//! The History pane's session log (opengeos/RustGIS#797, part 2): the commands run in the
//! session, newest first, filtered by command and to those that failed; saved as CSV or JSON
//! Lines; cleared or turned off. It shows the session's log (`Session::log`) as `project.log`
//! gives it, and changes it only through the log's commands, so what the pane does is logged as
//! a script's would be.

use egui::{CornerRadius, Sense, Stroke, Ui, vec2};
use rustgis_engine::project_log::LogEntry;
use serde_json::{Value, json};

use crate::RustGisApp;
use crate::theme::{Tokens, semibold};

/// Most entries the pane lists, the newest: it says how many more match.
pub const SHOWN: usize = 200;

/// What the listed rows were picked from: the log's [`changes`](rustgis_engine::project_log::SessionLog::changes)
/// and the filters, the query as typed (trimmed).
type Key = (u64, String, bool);

/// An entry as the pane draws it: its texts made once, when the rows are picked.
struct Row {
    e: LogEntry,
    /// Its parameters as one line of JSON (none for no parameters), and that line as listed.
    params: String,
    line: String,
    /// How long it took, and when it started.
    took: String,
    at: String,
}

impl Row {
    fn of(e: LogEntry) -> Row {
        let params = params_text(&e);
        // One line in the list; the whole of it (up to the log's 4 KB) on hover.
        let mut line: String = params.chars().take(120).collect();
        if line.len() < params.len() {
            line.push('…');
        }
        let took = crate::i18n::fmt("{ms} ms", &[("ms", &e.millis.to_string())]);
        let at = crate::i18n::fmt("{time} UTC", &[("time", &e.time_utc())]);
        Row { e, params, line, took, at }
    }
}

/// Which list the History pane shows, and the log's filters.
#[derive(Default)]
pub struct State {
    /// The session log rather than the tool runs.
    pub open: bool,
    /// Words the command's id holds (`layer.`, `export`), in any case.
    pub query: String,
    /// Only the commands that failed.
    pub failed: bool,
    /// The rows last listed and how many match and are in the log, kept while their [`Key`]
    /// holds: the pane is drawn each frame, and picking them goes through the whole log.
    cache: Option<(Key, Vec<Row>, usize, usize)>,
}

/// The switch at the top of the History pane, between its two records (named as Mavrick-F
/// proposed on opengeos/RustGIS#1216): the tool runs and the session log; whether the log is the
/// one shown.
pub fn tabs(app: &mut RustGisApp, ui: &mut Ui) -> bool {
    ui.horizontal(|ui| {
        let s = &mut app.ui.session_log;
        ui.selectable_value(&mut s.open, false, tl!("Tool runs"))
            .on_hover_text(tl!("The tools run on this project: saved with it, and each can run again"));
        ui.selectable_value(&mut s.open, true, tl!("Session log"))
            .on_hover_text(tl!("Everything done in this session, from the app, scripts, the AI Assistant or plugins"));
    });
    ui.add_space(4.0);
    app.ui.session_log.open
}

/// Whether entry `e` is one the filters keep: `query` already in lower case.
fn kept(e: &LogEntry, query: &str, failed: bool) -> bool {
    (!failed || !e.ok) && (query.is_empty() || e.command.to_lowercase().contains(query))
}

/// The session log: its filters, its newest entries that match, and what can be done with it.
pub fn pane(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    ui.horizontal(|ui| {
        let s = &mut app.ui.session_log;
        ui.add(egui::TextEdit::singleline(&mut s.query).hint_text(tl!("Filter by command")).desired_width(170.0))
            .on_hover_text(tl!("Words in the command's id, such as layer. or export"));
        ui.checkbox(&mut s.failed, tl!("Failed only"));
    });
    // Compared in place, so a repaint that changes nothing picks, formats and clones nothing;
    // the rows (and their texts) are made again only when the key changed.
    let (log, State { query, failed, cache, .. }) = (&app.session.log, &mut app.ui.session_log);
    let raw = query.trim();
    let same = cache.as_ref().is_some_and(|(k, ..)| k.0 == log.changes() && k.1 == raw && k.2 == *failed);
    if !same {
        let lower = raw.to_lowercase();
        let (shown, count, total) = log.select(|e| kept(e, &lower, *failed), SHOWN);
        let rows = shown.into_iter().map(Row::of).collect();
        *cache = Some(((log.changes(), raw.to_string(), *failed), rows, count, total));
    }
    let (count, total, listed) = cache.as_ref().map_or((0, 0, 0), |(_, rows, matched, all)| (*matched, *all, rows.len()));
    let note = if total == 0 {
        tl!("Commands that change the project, or that name a file, folder or address, appear here.").to_string()
    } else if count == 0 {
        tl!("No command matches").to_string()
    } else if count > listed {
        crate::i18n::fmt("The newest {shown} of {count} entries", &[("shown", &listed.to_string()), ("count", &count.to_string())])
    } else if count < total {
        crate::i18n::fmt("{count} of {total} entries", &[("count", &count.to_string()), ("total", &total.to_string())])
    } else {
        crate::i18n::fmt("{count} entries", &[("count", &count.to_string())])
    };
    ui.label(egui::RichText::new(note).size(11.0).color(t.text_dim));
    // Entries dropped past the log's limits aren't listed, nor saved: said.
    let dropped = app.session.log.dropped();
    if dropped > 0 {
        let said = crate::i18n::fmt("The {n} oldest entries were dropped past the log's limits", &[("n", &dropped.to_string())]);
        ui.label(egui::RichText::new(said).size(11.0).color(t.text_dim));
    }
    if !app.session.log.on() {
        ui.label(egui::RichText::new(tl!("Logging is off: commands run now aren't recorded.")).size(11.0).color(t.red));
    }
    ui.add_space(4.0);
    if let Some((_, rows, ..)) = &app.ui.session_log.cache {
        for r in rows.iter().rev() {
            row(ui, r, t);
            ui.add_space(4.0);
        }
    }
    ui.horizontal_wrapped(|ui| {
        let tip = tl!("The whole log, as CSV or JSON Lines (.jsonl): it may name files and addresses, so look at it before sharing it");
        if ui.add_enabled(total > 0, egui::Button::new(tl!("Save Log As…"))).on_hover_text(tip).clicked() {
            save(app);
        }
        if ui.add_enabled(total > 0, egui::Button::new(tl!("Clear Log"))).clicked()
            && let Err(e) = app.run("project.clearLog", Value::Null)
        {
            app.session.notify(e, true);
        }
        let mut on = app.session.log.on();
        let tip = tl!("Off, the commands run aren't recorded; what is in the log stays");
        if ui.checkbox(&mut on, tl!("Log Commands")).on_hover_text(tip).changed()
            && let Err(e) = app.run("project.logCommands", json!({"on": on}))
        {
            app.session.notify(e, true);
        }
    });
}

/// An entry's parameters as one line of JSON: none for no parameters.
fn params_text(e: &LogEntry) -> String {
    match &e.params {
        Value::Null => String::new(),
        Value::Object(m) if m.is_empty() => String::new(),
        p => p.to_string(),
    }
}

/// One entry: whether it worked, its command, how long it took, when it started, its error and
/// its parameters (all of them on hover; right-click copies them).
fn row(ui: &mut Ui, item: &Row, t: &Tokens) {
    let (e, params) = (&item.e, item.params.as_str());
    let frame =
        egui::Frame::NONE.stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::same(8));
    let r = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            // Worked or failed, said in words too (on hover, and to a screen reader), not by its
            // colour alone.
            let (r, dot) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.5, if e.ok { t.green } else { t.red });
            let worked = if e.ok { tl!("Worked") } else { tl!("Failed") };
            dot.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, worked));
            dot.on_hover_text(worked);
            ui.label(egui::RichText::new(&e.command).font(semibold(12.5)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(&item.took).size(11.0).color(t.text_dim));
            });
        });
        ui.label(egui::RichText::new(&item.at).size(11.0).color(t.text_dim));
        if let Some(err) = &e.error {
            ui.label(egui::RichText::new(err).size(11.0).color(t.red));
        }
        if !params.is_empty() {
            ui.add(egui::Label::new(egui::RichText::new(&item.line).monospace().size(10.5).color(t.text_dim)).truncate())
                .on_hover_text(egui::RichText::new(params).monospace());
        }
    });
    r.response.interact(Sense::click()).context_menu(|ui| {
        if ui.add_enabled(!params.is_empty(), egui::Button::new(tl!("Copy Parameters"))).clicked() {
            ui.ctx().copy_text(params.to_string());
            ui.close();
        }
        if ui.button(tl!("Copy Command")).clicked() {
            ui.ctx().copy_text(e.command.clone());
            ui.close();
        }
    });
}

/// Saves the log: downloaded as CSV in the browser, where `project.exportLog` can't write a
/// file (so the download isn't itself an entry in the log, said in the user guide); else to the
/// CSV or JSON Lines file the user picks, through `project.exportLog`, which the log records.
fn save(app: &mut RustGisApp) {
    if app.services.download.is_some() {
        let mut bytes = Vec::new();
        match app.session.log.write(&mut bytes, true) {
            Ok(()) => {
                app.download_bytes("session-log.csv", bytes);
                // Entries dropped past the log's limits aren't in the file: said, as an export
                // says it on the desktop.
                let dropped = app.session.log.dropped();
                if dropped > 0 {
                    let said = crate::i18n::fmt(
                        "session-log.csv: the {n} oldest entries were dropped past the log's limits before it",
                        &[("n", &dropped.to_string())],
                    );
                    app.session.notify(said, false);
                }
            }
            Err(e) => app.session.notify(e.to_string(), true),
        }
        return;
    }
    let Some(path) = crate::dialogs::pick_save("session-log.csv", &["csv", "jsonl"], tl!("Session log")) else {
        // Cancelled: nothing was saved.
        return;
    };
    // A name typed without an extension is saved as CSV, the first kind offered.
    let mut params = json!({"path": path});
    if std::path::Path::new(&path).extension().is_none() {
        params["format"] = "csv".into();
    }
    if let Err(e) = app.run("project.exportLog", params) {
        app.session.notify(e, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    fn harness(app: RustGisApp) -> egui_kittest::Harness<'static, RustGisApp> {
        let mut fonts = false;
        egui_kittest::Harness::builder().with_size(vec2(520.0, 2400.0)).build_ui_state(
            move |ui, app: &mut RustGisApp| {
                if !fonts {
                    crate::theme::install_fonts(ui.ctx(), &[], None);
                    fonts = true;
                    return;
                }
                let t = crate::theme::Tokens::light();
                if tabs(app, ui) {
                    pane(app, ui, &t);
                }
            },
            app,
        )
    }

    /// The History pane's session log lists the session's commands, newest first, filtered by
    /// command and to those that failed, and says how many match; reading is left out, as in
    /// the log.
    #[test]
    fn the_session_log_lists_the_commands_run_newest_first() {
        let mut app = RustGisApp::with_sample();
        app.session.log.clear();
        app.run("map.new", json!({"name": "Second"})).unwrap();
        assert!(app.run("layer.remove", json!({"layer": 999_999})).is_err());
        app.run("info.project", Value::Null).unwrap();
        app.ui.session_log.open = true;
        let mut h = harness(app);
        h.run();
        assert!(h.query_by_label("Tool runs").is_some() && h.query_by_label("Session log").is_some(), "the two views, by name");
        assert!(h.query_by_label("2 entries").is_some());
        let y = |h: &egui_kittest::Harness<'_, RustGisApp>, name: &str| h.get_by_label(name).rect().center().y;
        assert!(y(&h, "layer.remove") < y(&h, "map.new"), "newest first");
        assert!(h.query_by_label("info.project").is_none(), "reading isn't logged");
        // Its parameters, on one line.
        assert!(h.query_by_label(r#"{"name":"Second"}"#).is_some());
        h.state_mut().ui.session_log.failed = true;
        h.run();
        assert!(h.query_by_label("1 of 2 entries").is_some());
        assert!(h.query_by_label("map.new").is_none());
        let s = &mut h.state_mut().ui.session_log;
        (s.failed, s.query) = (false, "MAP.".into());
        h.run();
        assert!(h.query_by_label("map.new").is_some() && h.query_by_label("layer.remove").is_none());
        h.state_mut().ui.session_log.query = "no such command".into();
        h.run();
        assert!(h.query_by_label("No command matches").is_some());
        // Tool runs shows the tools run instead.
        h.state_mut().ui.session_log.open = false;
        h.run();
        assert!(h.query_by_label("Filter by command").is_none());
    }

    /// Past [`SHOWN`] entries the pane lists the newest and says how many match.
    #[test]
    fn a_long_log_shows_its_newest_entries() {
        let mut app = RustGisApp::with_sample();
        app.session.log.clear();
        for i in 0..SHOWN + 5 {
            app.run("map.new", json!({"name": format!("Map {i}")})).unwrap();
        }
        app.ui.session_log.open = true;
        let mut h = harness(app);
        h.run();
        assert!(h.query_by_label(&format!("The newest {SHOWN} of {} entries", SHOWN + 5)).is_some());
        assert!(h.query_by_label(&format!(r#"{{"name":"Map {}"}}"#, SHOWN + 4)).is_some());
        assert!(h.query_by_label(r#"{"name":"Map 4"}"#).is_none());
        // Nothing dropped, nothing said; each entry says whether it worked, in words.
        assert!(h.query_by_label_contains("dropped past the log's limits").is_none());
        assert!(h.query_all_by_label("Worked").next().is_some());
    }

    /// Past the log's limit, the pane says how many of the oldest entries it dropped.
    #[test]
    fn a_log_past_its_limit_says_what_it_dropped() {
        let mut app = RustGisApp::with_sample();
        app.session.log.clear();
        for i in 0..rustgis_engine::project_log::LOG_LIMIT + 7 {
            app.session.log.push(LogEntry { at: i as u64, command: "x".into(), params: Value::Null, ok: true, error: None, millis: 0 });
        }
        app.ui.session_log.open = true;
        let mut h = harness(app);
        h.run();
        assert!(h.query_by_label("The 7 oldest entries were dropped past the log's limits").is_some());
    }

    /// Save Log As… writes the log where the user picks, as CSV or JSON Lines by the name's
    /// extension (CSV without one), through `project.exportLog`; Clear Log empties it, leaving its
    /// own entry; Log Commands turns it off, saying so.
    #[test]
    fn the_session_log_is_saved_cleared_and_turned_off() {
        let mut app = RustGisApp::with_sample();
        app.session.log.clear();
        app.run("map.new", json!({"name": "Second"})).unwrap();
        let dir = tempfile::tempdir().unwrap();
        app.ui.session_log.open = true;
        let mut h = harness(app);
        h.run();
        for (name, first) in [("log.csv", "time_utc,"), ("log.jsonl", "{\"at\":"), ("log", "time_utc,")] {
            let path = dir.path().join(name);
            crate::dialogs::PICKED.with(|p| *p.borrow_mut() = Some(path.display().to_string()));
            h.get_by_label("Save Log As…").click();
            h.run();
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(text.starts_with(first), "{name}: {text}");
            assert!(text.contains("map.new"), "{name}");
        }
        // Each save is in the log, as a script's would be.
        assert_eq!(h.state().session.log.entries().filter(|e| e.command == "project.exportLog").count(), 3);
        h.get_by_label("Clear Log").click();
        h.run();
        let commands: Vec<String> = h.state().session.log.entries().map(|e| e.command.clone()).collect();
        assert_eq!(commands, ["project.clearLog"]);
        h.get_by_label("Log Commands").click();
        h.run();
        assert!(!h.state().session.log.on());
        assert!(h.query_by_label("Logging is off: commands run now aren't recorded.").is_some());
        h.state_mut().run("map.new", json!({"name": "Third"})).unwrap();
        assert!(h.state().session.log.entries().all(|e| e.command != "map.new"));
    }

    /// In the browser, where `project.exportLog` can't write a file, Save Log As… downloads the
    /// log as CSV.
    #[test]
    fn in_the_browser_the_session_log_is_downloaded_as_csv() {
        let mut app = RustGisApp::with_sample();
        app.session.log.clear();
        app.run("map.new", json!({"name": "Second"})).unwrap();
        let got = std::rc::Rc::new(std::cell::RefCell::new(None));
        let sink = got.clone();
        app.services.download = Some(Box::new(move |name, bytes| {
            *sink.borrow_mut() = Some((name.to_string(), String::from_utf8(bytes).unwrap()));
            Ok(())
        }));
        save(&mut app);
        let (name, text) = got.borrow_mut().take().expect("downloaded");
        assert_eq!(name, "session-log.csv");
        assert!(
            text.starts_with(
                "time_utc,at,command,ok,millis,error,params
"
            ),
            "{text}"
        );
        assert!(text.contains("\"map.new\",true"), "{text}");
        let dropped = |app: &RustGisApp| app.session.notices.iter().any(|n| n.text.contains("dropped past the log's limits"));
        assert!(!dropped(&app), "nothing dropped, nothing said");
        // Past the log's limit, the download says how many it lacks.
        for i in 0..rustgis_engine::project_log::LOG_LIMIT + 3 {
            app.session.log.push(LogEntry { at: i as u64, command: "x".into(), params: Value::Null, ok: true, error: None, millis: 0 });
        }
        save(&mut app);
        assert!(got.borrow_mut().take().is_some());
        assert!(dropped(&app), "{:?}", app.session.notices);
    }
}
