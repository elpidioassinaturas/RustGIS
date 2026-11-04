//! A plugin command the user asks for in the interface starts a frame or two later, so the window
//! first shows that it is running (#1319): the command may wait for a server on this thread, and
//! nothing is drawn until it has finished. Clicks made meanwhile, which the window system holds
//! until then, are dropped, so a second click out of impatience doesn't run it again. Scripts, the
//! AI Assistant and attached agents run commands at once ([`RustGisApp::run`]).

use crate::{RustGisApp, i18n};
use serde_json::Value;
use std::collections::VecDeque;
use std::time::Duration;

/// Frames drawn before a command runs: the one it was asked in may have drawn the status bar
/// already, so the next is the one that shows it.
const FRAMES_SHOWN: u8 = 2;

/// A run this long has clicks held for it that were meant for a window that wasn't answering.
const STALE_AFTER: Duration = Duration::from_millis(150);

struct Start {
    id: String,
    params: Value,
    frames: u8,
}

#[derive(Default)]
pub struct State {
    waiting: VecDeque<Start>,
    /// A command that took long has just run: the clicks arriving next were made during it.
    clicks_stale: bool,
}

/// Leaves plugin command `id` to start once the window has shown it. The same command with the
/// same parameters, already waiting, isn't asked twice.
pub(crate) fn ask(app: &mut RustGisApp, id: &str, params: Value) {
    let waiting = &mut app.ui.plugin_start.waiting;
    if !waiting.iter().any(|s| s.id == id && s.params == params) {
        waiting.push_back(Start { id: id.to_string(), params, frames: 0 });
    }
}

/// Runs the command that has been shown. Its errors are notices, as for any command.
pub(crate) fn run_next(app: &mut RustGisApp) {
    if app.ui.plugin_start.waiting.front().is_none_or(|s| s.frames < FRAMES_SHOWN) {
        return;
    }
    let Some(start) = app.ui.plugin_start.waiting.pop_front() else { return };
    let began = web_time::Instant::now();
    let _ = app.run(&start.id, start.params);
    app.ui.plugin_start.clicks_stale = began.elapsed() >= STALE_AFTER;
}

/// At the end of a frame: a command waiting has been shown once more, with a busy cursor, and
/// another frame is wanted (nothing else may ask for one).
pub(crate) fn frame_drawn(app: &mut RustGisApp, ctx: &egui::Context) {
    if let Some(start) = app.ui.plugin_start.waiting.front_mut() {
        start.frames = start.frames.saturating_add(1);
        ctx.set_cursor_icon(egui::CursorIcon::Wait);
        ctx.request_repaint();
    }
}

/// What the status bar says while a command waits to start: `Running BrasilData: Solos…`.
pub(crate) fn running(app: &RustGisApp) -> Option<String> {
    let id = &app.ui.plugin_start.waiting.front()?.id;
    let plugin = rustgis_engine::plugins::split_id(id).and_then(|(plugin, item)| Some((rustgis_engine::plugins::get(plugin)?, item)));
    let title = plugin.map_or_else(
        || id.clone(),
        |(p, item)| {
            let m = &p.manifest;
            let label = m.commands.iter().find(|c| c.id == item).map_or(item, |c| m.translate(app.lang().code(), &c.label));
            format!("{}: {}", m.name, label.replace('\n', " "))
        },
    );
    Some(i18n::fmt("Running {command}…", &[("command", &title)]))
}

impl RustGisApp {
    /// Before a frame, from the window: drops the clicks made during a long plugin command, press
    /// and release alike, so none is half taken.
    pub fn drop_stale_clicks(&mut self, input: &mut egui::RawInput) {
        if std::mem::take(&mut self.ui.plugin_start.clicks_stale) {
            input.events.retain(|e| !matches!(e, egui::Event::PointerButton { .. }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, pos2, vec2};
    use egui_kittest::kittest::Queryable;
    use rustgis_engine::plugins;
    use serde_json::json;

    fn launch(plugin: &str) -> egui_kittest::Harness<'static, RustGisApp> {
        let pkg = plugins::Package::from_zip(&plugins::testing::package(plugin, &["edit"])).unwrap();
        plugins::load(pkg, "test").unwrap();
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 860.0)).with_max_steps(64).build_ui_state(
            |ui, app: &mut RustGisApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            RustGisApp::with_sample(),
        );
        h.run();
        h
    }

    fn rename(from: &str, to: &str) -> Value {
        json!({"op": "layer.rename", "params": {"layer": from, "name": to}})
    }

    /// Asked for by the user, a command runs only after a frame has said so; asked twice, once.
    #[test]
    fn a_command_is_shown_running_before_it_runs() {
        let mut h = launch("ui-start");
        let id = "plugin:ui-start/f";
        let mark = h.state().session.notice_mark();
        h.state_mut().run_by_user(id, rename("Rivers", "Streams")).unwrap();
        h.state_mut().run_by_user(id, rename("Rivers", "Streams")).unwrap();
        h.step();
        assert!(h.state().session.map().layer_by_name("Rivers").is_some(), "not yet");
        h.step();
        assert!(h.state().session.map().layer_by_name("Rivers").is_some(), "not before it is shown");
        assert!(running(h.state()).is_some_and(|t| t.starts_with("Running") && t.contains("ui-start")), "{:?}", running(h.state()));
        let _ = h.get_by_label_contains("Running");
        h.run();
        assert!(h.state().session.map().layer_by_name("Streams").is_some());
        // Run again, it would find no Rivers.
        assert!(!h.state().session.notices_since(mark).iter().any(|n| n.error), "run once");
        assert!(running(h.state()).is_none());
        plugins::unload("ui-start");
    }

    /// Another command asked for meanwhile runs after it, in turn; `run` (scripts, the assistant)
    /// doesn't wait.
    #[test]
    fn commands_asked_for_run_in_turn() {
        let mut h = launch("ui-turn");
        let id = "plugin:ui-turn/f";
        h.state_mut().run_by_user(id, rename("Rivers", "Streams")).unwrap();
        h.state_mut().run_by_user(id, rename("Streams", "Brooks")).unwrap();
        h.run();
        assert!(h.state().session.map().layer_by_name("Brooks").is_some());
        h.state_mut().run(id, rename("Brooks", "Creeks")).unwrap();
        assert!(h.state().session.map().layer_by_name("Creeks").is_some());
        plugins::unload("ui-turn");
    }

    /// Clicks are dropped once, after a long run; what the pointer did otherwise stays.
    #[test]
    fn clicks_made_during_a_long_run_are_dropped_once() {
        let mut app = RustGisApp::with_sample();
        let at = pos2(10.0, 10.0);
        let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        let events = || vec![Event::PointerMoved(at), button(true), button(false)];
        let mut input = egui::RawInput { events: events(), ..Default::default() };
        app.drop_stale_clicks(&mut input);
        assert_eq!(input.events.len(), 3, "none stale");
        app.ui.plugin_start.clicks_stale = true;
        app.drop_stale_clicks(&mut input);
        assert_eq!(input.events, vec![Event::PointerMoved(at)]);
        let mut input = egui::RawInput { events: events(), ..Default::default() };
        app.drop_stale_clicks(&mut input);
        assert_eq!(input.events.len(), 3, "only those of that run");
    }
}
