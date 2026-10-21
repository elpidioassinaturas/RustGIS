//! Navigation benchmark for the desktop app's tile maps (#635): the real window, egui painting
//! and GPU submission included, with the same scripted pans and wheel zooms as the browser's
//! `apps/rustgis-web/bench/navigate.mjs`.
//!
//! `cargo run --release -p rustgis --example navigate_bench -- [--gpu] [--case liberty|imagery]`
//!
//! Needs the network and a display. Reports the intervals between frames and the time the app's
//! own frame (`RustGisApp::ui`) took on the UI thread while navigating.
use std::time::Instant;

use rustgis_ui_egui::RustGisApp;
use serde_json::json;

/// Seconds: tiles load, then drags, then wheel zooms, then the report.
const LOAD: f64 = 8.0;
const PAN: f64 = 6.0;
const ZOOM: f64 = 6.0;

struct Bench {
    app: RustGisApp,
    start: Instant,
    last: Instant,
    dragging: bool,
    intervals: Vec<f64>,
    /// When the last wheel turn was sent: the wheel turns at a rate, whatever the frame rate.
    last_wheel: f64,
    ui_ms: Vec<f64>,
    reported: bool,
}

fn stats(name: &str, samples: &mut [f64]) {
    samples.sort_by(f64::total_cmp);
    let n = samples.len();
    let at = |p: usize| samples.get((n * p / 100).min(n.saturating_sub(1))).copied().unwrap_or(0.0);
    println!(
        "{name}: n={n} p50={:.1} p95={:.1} p99={:.1} max={:.1} ms, >33 ms: {}, >100 ms: {}",
        at(50),
        at(95),
        at(99),
        samples.last().copied().unwrap_or(0.0),
        samples.iter().filter(|&&x| x > 33.0).count(),
        samples.iter().filter(|&&x| x > 100.0).count()
    );
}

impl eframe::App for Bench {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, input: &mut egui::RawInput) {
        let t = self.start.elapsed().as_secs_f64();
        let from = egui::pos2(740.0, 520.0);
        if (LOAD..LOAD + PAN).contains(&t) {
            if !self.dragging {
                input.events.push(egui::Event::PointerMoved(from));
                input.events.push(egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                });
                self.dragging = true;
            }
            // Back and forth, and down and up, across several tiles.
            let s = (t - LOAD) * 1.6;
            input.events.push(egui::Event::PointerMoved(from + egui::vec2(s.sin() as f32 * 400.0, (s * 0.7).sin() as f32 * 220.0)));
        } else if self.dragging {
            input.events.push(egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            });
            self.dragging = false;
        }
        let z = t - LOAD - PAN;
        if (0.0..ZOOM).contains(&z) {
            input.events.push(egui::Event::PointerMoved(from));
            // Out for a third, in for a third, out again, at 600 points a second, so a run at any
            // frame rate zooms the same.
            // From the zoom's start, all of it, however long a frame took.
            let dt = (t - self.last_wheel.max(LOAD + PAN)).max(0.0);
            self.last_wheel = t;
            let dir = if z < ZOOM / 3.0 || z >= 2.0 * ZOOM / 3.0 { -1.0 } else { 1.0 };
            input.events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, (dir * 600.0 * dt) as f32),
                phase: egui::TouchPhase::Move,
                modifiers: Default::default(),
            });
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.set_render_state(frame.wgpu_render_state().cloned());
        self.app.logic(ctx);
        ctx.request_repaint();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let t = self.start.elapsed().as_secs_f64();
        let navigating = (LOAD..LOAD + PAN + ZOOM).contains(&t);
        if navigating {
            self.intervals.push(now.duration_since(self.last).as_secs_f64() * 1000.0);
        }
        self.last = now;
        self.app.ui(ui);
        if navigating {
            self.ui_ms.push(now.elapsed().as_secs_f64() * 1000.0);
        }
        if t >= LOAD + PAN + ZOOM + 1.0 && !self.reported {
            stats("frame intervals", &mut self.intervals);
            stats("UI thread per frame", &mut self.ui_ms);
            self.reported = true;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    let gpu = args.iter().any(|a| a == "--gpu");
    let case = args.iter().position(|a| a == "--case").and_then(|i| args.get(i + 1)).map_or("liberty", String::as_str);
    let fail = |e: String| eframe::Error::AppCreation(e.into());
    let (tiles, at) = match case {
        "imagery" => (
            json!({"url": "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}"}),
            "#13/48.8566/2.3522",
        ),
        "liberty" => (json!({"url": "https://tiles.openfreemap.org/styles/liberty", "kind": "style"}), "#14/40.7549/-73.9840"),
        other => return Err(fail(format!("--case {other}: liberty or imagery"))),
    };
    let mut app = RustGisApp::with_sample();
    app.run("project.new", json!({"name": "Navigation benchmark"})).map_err(fail)?;
    app.run("ui.map.gpu", json!({"on": gpu})).map_err(fail)?;
    app.run("map.addTiles", tiles).map_err(fail)?;
    app.run("map.goToLink", json!({"url": at})).map_err(fail)?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("RustGIS navigation benchmark").with_inner_size([1480.0, 900.0]),
        ..Default::default()
    };
    println!("navigate_bench case={case} gpu={gpu}");
    eframe::run_native(
        "RustGIS navigation benchmark",
        options,
        Box::new(move |_| {
            let now = Instant::now();
            Ok(Box::new(Bench {
                app,
                start: now,
                last: now,
                dragging: false,
                intervals: Vec::new(),
                last_wheel: 0.0,
                ui_ms: Vec::new(),
                reported: false,
            }))
        }),
    )
}
