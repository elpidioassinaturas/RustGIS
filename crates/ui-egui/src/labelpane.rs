//! The Labeling pane (#617): every label setting of the selected feature layer in one place, opened
//! from a layer's right-click menu (Labeling Properties…) or View › Windows › Labeling. The ribbon
//! keeps its label groups; both run the same `labels.*` commands, so the pane holds no state of its
//! own (it reads the layer each frame, as the Symbology pane does) and every change undoes.

use egui::{RichText, Ui, vec2};
use rustgis_engine::map::symbology::{DEFAULT_LABEL_CLASS, LABEL_SIZE, MAX_LABEL_CLASSES};
use rustgis_engine::map::{LabelPlacement, Layer, Rgba};
use serde_json::{Value, json};

use crate::RustGisApp;
use crate::textform::{LABEL_W, section};
use crate::theme::{Tokens, c32, rgba, semibold};

/// The placements as they sit around a feature, row by row; Automatic sits apart, above them.
const GRID: [[LabelPlacement; 3]; 3] = [
    [LabelPlacement::AboveLeft, LabelPlacement::Above, LabelPlacement::AboveRight],
    [LabelPlacement::Left, LabelPlacement::Center, LabelPlacement::Right],
    [LabelPlacement::BelowLeft, LabelPlacement::Below, LabelPlacement::BelowRight],
];

/// The viewer distances the Closest and Farthest limits offer, in metres (as the ribbon's).
const DISTANCES: [f64; 5] = [1_000.0, 10_000.0, 100_000.0, 1_000_000.0, 10_000_000.0];

pub fn show(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let Some(l) = app.session.active_layer.and_then(|id| app.session.map().layer(id)).cloned() else {
        ui.label(RichText::new(tl!("Select a layer in the Layers pane.")).color(t.text_dim));
        return;
    };
    ui.label(RichText::new(&l.name).font(semibold(14.0)));
    if l.is_raster() {
        ui.add_space(6.0);
        ui.label(RichText::new(tl!("Labels are drawn for feature layers, not rasters.")).color(t.text_dim));
        return;
    }
    ui.add_space(6.0);
    let mut on = l.labels.enabled;
    if ui.checkbox(&mut on, tl!("Label features in this layer")).changed() {
        run(app, "labels.enable", json!({"on": on}));
    }
    class(app, ui, &l, t);
    text_symbol(app, ui, &l);
    position(app, ui, &l);
    visibility(app, ui, &l);
    map_labeling(app, ui);
}

/// Runs a label command from the pane on the active layer, the one it shows; a refusal shows as a
/// notice. The commands don't name the layer, as the ribbon's don't: a step recorded from them
/// (Record Task) then names it, so it replays on that layer by name (`RustGisApp::record_step`).
fn run(app: &mut RustGisApp, id: &str, params: Value) {
    if let Err(e) = app.run(id, params) {
        app.session.notify(e, true);
    }
}

fn grid(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    egui::Grid::new(("label_pane", id)).num_columns(2).min_col_width(LABEL_W).spacing(vec2(8.0, 6.0)).show(ui, add);
}

/// The tabs of the Class section, as ArcGIS Pro's Class tab: what the label shows, and the SQL
/// choosing the features it labels.
#[derive(Clone, Copy, PartialEq)]
enum ClassTab {
    Field,
    Sql,
}

/// One class for now, "Default": the field it labels with, and the SQL clause (#618) choosing the
/// features it labels (the other classes are made with commands for now).
fn class(app: &mut RustGisApp, ui: &mut Ui, l: &Layer, t: &Tokens) {
    section(ui, "Class", |ui| {
        // Its own name once renamed (`labels.class.rename`), else "Default" translated.
        let name = &l.labels.default.name;
        let name = if name == DEFAULT_LABEL_CLASS { tl!("Default").to_string() } else { name.clone() };
        ui.horizontal(|ui| {
            ui.label(tl!("Class"));
            ui.label(RichText::new(name).color(t.text_dim));
        });
        let key = egui::Id::new("label_pane_class_tab");
        let mut tab = ui.data(|d| d.get_temp::<ClassTab>(key)).unwrap_or(ClassTab::Field);
        ui.horizontal(|ui| {
            for (label, it) in [(tl!("Field"), ClassTab::Field), (tl!("SQL"), ClassTab::Sql)] {
                if ui.add(egui::Button::selectable(tab == it, label)).clicked() {
                    tab = it;
                }
            }
        });
        ui.data_mut(|d| d.insert_temp(key, tab));
        ui.add_space(4.0);
        match tab {
            ClassTab::Field => field(app, ui, l),
            ClassTab::Sql => sql(app, ui, l, t),
        }
    });
}

/// The field the class labels with.
fn field(app: &mut RustGisApp, ui: &mut Ui, l: &Layer) {
    grid(ui, "class", |ui| {
        ui.label(tl!("Field"));
        let cur = &l.labels.default.field;
        egui::ComboBox::from_id_salt("label_pane_field").selected_text(cur.as_str()).width(180.0).show_ui(ui, |ui| {
            for f in &l.dataset.fields {
                if ui.selectable_label(f.name == *cur, &f.name).clicked() {
                    run(app, "labels.field", json!({"field": f.name}));
                }
            }
        });
        ui.end_row();
    });
}

/// The Default class's SQL clause: its labels, and their style, go only to the features that
/// match it; none labels every feature.
fn sql(app: &mut RustGisApp, ui: &mut Ui, l: &Layer, t: &Tokens) {
    let labels = &l.labels;
    if !labels.enabled {
        ui.label(RichText::new(tl!("Labels are off for this layer: the clause applies once they are on.")).small().color(t.text_dim));
    }
    if !labels.default.visible {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tl!("This class is hidden, so it labels nothing.")).small().color(t.text_dim));
            if ui.small_button(tl!("Show labels")).clicked() {
                run(app, "labels.class.update", json!({"visible": true}));
            }
        });
    }
    if !labels.classes.is_empty() {
        let n = labels.classes.len().min(MAX_LABEL_CLASSES).to_string();
        let said = crate::i18n::fmt(
            "Other label classes on this layer (made with commands): {n}. They are checked first; this clause is for the features they leave.",
            &[("n", &n)],
        );
        ui.label(RichText::new(said).small().color(t.text_dim));
    }
    // Counted among the features the layer draws at the map's scale, as its labels are.
    let scale = app.session.map().scale();
    let count = |w: &str| -> Result<(usize, usize), String> {
        let e = rustgis_engine::data::query::parse(w, &l.dataset)?;
        let rows = l.drawn_rows(scale);
        let of = rows.iter().filter(|r| **r).count();
        let n = rows.iter().enumerate().filter(|(i, r)| **r && rustgis_engine::data::query::eval(&e, &l.dataset, *i)).count();
        Ok((n, of))
    };
    // A draft belongs to this layer of this project: another project may reuse the layer's id.
    let owner = egui::Id::new((app.session.opened, l.id));
    // A count holds for this project state at this scale (display filters follow the scale).
    let version = egui::Id::new((app.session.revision, scale.to_bits())).value();
    match crate::sqlbox::show(ui, "label_sql", owner, version, labels.default.where_.as_deref(), &l.dataset, count) {
        Some(crate::sqlbox::Apply::Clause(w)) => run(app, "labels.class.update", json!({"where": w})),
        Some(crate::sqlbox::Apply::Clear) => run(app, "labels.class.update", json!({"where": null})),
        None => {}
    }
}

fn text_symbol(app: &mut RustGisApp, ui: &mut Ui, l: &Layer) {
    section(ui, "Text Symbol", |ui| {
        grid(ui, "symbol", |ui| {
            ui.label(tl!("Size"));
            // The whole range `labels.style` accepts (#457); the map follows a drag and letting
            // go runs the command once, so a drag is one undo step.
            let mut size = f64::from(l.labels.default.size);
            let r = ui.add(egui::DragValue::new(&mut size).range(LABEL_SIZE).speed(0.1).suffix(" pt"));
            let id = l.id;
            crate::ribbon::live_drag(
                app,
                ui,
                &r,
                f64::from(l.labels.default.size),
                size,
                |app, x| {
                    if let Some(l) = app.session.map_mut().layer_mut(id) {
                        l.labels.default.size = x.clamp(*LABEL_SIZE.start(), *LABEL_SIZE.end()) as f32;
                    }
                },
                |app, x| run(app, "labels.style", json!({"size": x})),
            );
            ui.end_row();
            ui.label(tl!("Color"));
            let mut c = c32(l.labels.default.color);
            if crate::widgets::color_button(ui, &mut c).changed() {
                color_drag(app, ui, l, rgba(c));
            }
            ui.end_row();
            ui.label("");
            ui.horizontal(|ui| {
                let mut bold = l.labels.default.bold;
                if ui.checkbox(&mut bold, tl!("Bold")).changed() {
                    run(app, "labels.style", json!({"bold": bold}));
                }
                let mut halo = l.labels.default.halo;
                if ui.checkbox(&mut halo, tl!("Halo")).changed() {
                    run(app, "labels.style", json!({"halo": halo}));
                }
            });
            ui.end_row();
        });
    });
}

fn position(app: &mut RustGisApp, ui: &mut Ui, l: &Layer) {
    section(ui, "Position", |ui| {
        let cur = l.labels.default.placement;
        let pick = |app: &mut RustGisApp, ui: &mut Ui, pl: LabelPlacement| {
            if ui.add_sized(vec2(92.0, 22.0), egui::Button::selectable(cur == pl, tl!(pl.label())).truncate()).clicked() && cur != pl {
                run(app, "labels.placement", json!({"position": pl.code()}));
            }
        };
        ui.horizontal(|ui| pick(app, ui, LabelPlacement::Auto));
        ui.add_space(2.0);
        egui::Grid::new(("label_pane", "position")).spacing(vec2(4.0, 4.0)).show(ui, |ui| {
            for row in GRID {
                for pl in row {
                    pick(app, ui, pl);
                }
                ui.end_row();
            }
        });
    });
}

fn visibility(app: &mut RustGisApp, ui: &mut Ui, l: &Layer) {
    section(ui, "Visibility Range", |ui| {
        grid(ui, "range", |ui| {
            for (label, cur, key) in [("Out Beyond", l.labels.default.min_scale, "min"), ("In Beyond", l.labels.default.max_scale, "max")] {
                ui.label(tl!(label));
                egui::ComboBox::from_id_salt(("label_pane_scale", key))
                    .selected_text(crate::ribbon::scale_label(cur))
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for c in std::iter::once(None).chain(crate::ribbon::SCALES.iter().map(|s| Some(*s))) {
                            if ui.selectable_label(c == cur, crate::ribbon::scale_label(c)).clicked() {
                                run(app, "labels.scaleRange", json!({key: c}));
                            }
                        }
                    });
                ui.end_row();
            }
            for (label, cur, key) in [("Closest", l.labels.default.closest, "closest"), ("Farthest", l.labels.default.farthest, "farthest")]
            {
                ui.label(tl!(label));
                let text = |d: Option<f64>| d.map(rustgis_engine::geom::format_distance).unwrap_or_else(|| tl!("<None>").to_string());
                egui::ComboBox::from_id_salt(("label_pane_distance", key)).selected_text(text(cur)).width(140.0).show_ui(ui, |ui| {
                    for d in std::iter::once(None).chain(DISTANCES.iter().map(|d| Some(*d))) {
                        if ui.selectable_label(d == cur, text(d)).clicked() {
                            run(app, "labels.distanceRange", json!({key: d}));
                        }
                    }
                });
                ui.end_row();
            }
        });
        let limited = [l.labels.default.min_scale, l.labels.default.max_scale, l.labels.default.closest, l.labels.default.farthest]
            .iter()
            .any(Option::is_some);
        if ui
            .add_enabled(limited, egui::Button::new(tl!("Clear Limits")))
            .on_hover_text(tl!("Draw labels at every scale and distance."))
            .clicked()
        {
            run(app, "labels.scaleRange", json!({"clear": true}));
        }
    });
}

/// The settings for every layer's labels on the map, and turning this layer's into annotation.
fn map_labeling(app: &mut RustGisApp, ui: &mut Ui) {
    section(ui, "Map Labeling", |ui| {
        let d = app.session.map().label_display;
        for (label, desc, id, cur) in [
            ("Lock", "Keep the labelled features fixed as the map moves.", "labels.lock", d.locked),
            ("Pause", "Stop drawing labels, for faster navigation.", "labels.pause", d.paused),
            ("View Unplaced", "Show the labels that don't fit, in red.", "labels.viewUnplaced", d.show_unplaced),
        ] {
            let mut on = cur;
            if ui.checkbox(&mut on, tl!(label)).on_hover_text(tl!(desc)).changed() {
                run(app, id, json!({"on": on}));
            }
        }
        ui.add_space(4.0);
        if ui.button(tl!("Convert to Annotation")).on_hover_text(tl!("Convert this layer's labels into an annotation layer.")).clicked() {
            run(app, "labels.convert", json!({}));
        }
    });
}

/// Where a label colour being dragged in the pane's picker waits.
const COLOR_DRAG: &str = "label_pane_color_drag";

/// A label colour being dragged: it previews in the project, and when the pointer is let go
/// `finish_color_drag` puts `before` back and runs labels.style once, so a pick is one undo step
/// holding the pre-drag colour (as the Symbology pane's class colours do, #314).
#[derive(Clone)]
struct ColorDrag {
    layer: u32,
    before: Rgba,
    color: Rgba,
}

fn color_drag(app: &mut RustGisApp, ui: &Ui, l: &Layer, color: Rgba) {
    let key = egui::Id::new(COLOR_DRAG);
    let mut pending = ui.data(|d| d.get_temp::<ColorDrag>(key));
    // One left pending on another layer (the selection changed mid-pick, from the Assistant or a
    // shortcut) is finished first rather than dropped with its preview in place.
    if let Some(other) = pending.take_if(|d| d.layer != l.id) {
        ui.data_mut(|d| d.remove::<ColorDrag>(key));
        apply_color_drag(app, other);
    }
    // The colour before the first frame of this pick, kept while it goes on.
    let before = pending.map_or(l.labels.default.color, |d| d.before);
    ui.data_mut(|d| d.insert_temp(key, ColorDrag { layer: l.id, before, color }));
    if let Some(layer) = app.session.map_mut().layer_mut(l.id) {
        layer.labels.default.color = color;
    }
    app.session.revision = app.session.revision.wrapping_add(1);
}

/// Ends a colour pick once the pointer is let go. Called after the panes (from `panes::show`), so
/// it still ends if the pane closes meanwhile. A change made with no pointer down (typed in the
/// picker's hex field, or with the keyboard) ends on the frame it is made: each is its own undo
/// step, and none is lost, as in the Symbology pane's class colours.
pub(crate) fn finish_color_drag(app: &mut RustGisApp, ui: &Ui) {
    // Still dragging: the pointer is down and the window has focus (a release outside a window
    // that lost focus may never arrive).
    if ui.input(|i| i.pointer.primary_down() && i.focused) {
        return;
    }
    let key = egui::Id::new(COLOR_DRAG);
    let Some(drag) = ui.data_mut(|d| {
        let drag = d.get_temp::<ColorDrag>(key);
        d.remove::<ColorDrag>(key);
        drag
    }) else {
        return;
    };
    apply_color_drag(app, drag);
}

/// Puts the pre-pick colour back and runs the pick's labels.style: one undo step.
fn apply_color_drag(app: &mut RustGisApp, drag: ColorDrag) {
    // A layer removed mid-pick took its preview with it: there is nothing to restore or style.
    let Some(layer) = app.session.map_mut().layer_mut(drag.layer) else { return };
    layer.labels.default.color = drag.before;
    if drag.before != drag.color {
        // On the layer the pick was made on, as the active one: the command reaches it by id
        // (a name two layers share would reach the first), and a recorded step names it, as
        // for the pane's other commands. The selection, if it moved mid-pick, is put back (as
        // `Session::run` does to check a named layer's conditions).
        let selected = app.session.active_layer.replace(drag.layer);
        run(app, "labels.style", json!({"color": drag.color.hex()}));
        app.session.active_layer = selected;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use egui_kittest::kittest::Queryable;
    use rustgis_engine::map::Rgba;
    use serde_json::json;

    use crate::{RightPane, RustGisApp};

    fn layer_id(app: &RustGisApp, name: &str) -> u32 {
        app.session.map().layers.iter().find(|l| l.name == name).unwrap().id
    }

    fn with_cities() -> RustGisApp {
        let mut app = RustGisApp::with_sample();
        let id = layer_id(&app, "Cities");
        app.session.run("layer.select", &json!({"layer": id})).unwrap();
        app
    }

    fn harness(app: RustGisApp) -> egui_kittest::Harness<'static, RustGisApp> {
        // The pane's fonts (its semibold heading) are installed on the first frame, as the app does.
        let mut fonts = false;
        egui_kittest::Harness::builder().with_size(egui::vec2(360.0, 900.0)).build_ui_state(
            move |ui, app: &mut RustGisApp| {
                if !fonts {
                    crate::theme::install_fonts(ui.ctx(), &[], None);
                    fonts = true;
                    return;
                }
                super::show(app, ui, &crate::theme::Tokens::light());
                super::finish_color_drag(app, ui);
            },
            app,
        )
    }

    #[test]
    fn the_pane_command_needs_a_feature_layer() {
        let mut app = RustGisApp::with_sample();
        app.session.active_layer = None;
        assert!(app.run("ui.pane.labeling", json!(null)).is_err(), "no layer selected");
        let raster = app.session.map().layers.iter().find(|l| l.is_raster()).map(|l| l.id);
        let cities = layer_id(&app, "Cities");
        if let Some(raster) = raster {
            app.session.run("layer.select", &json!({"layer": raster})).unwrap();
            assert!(app.run("ui.pane.labeling", json!(null)).is_err(), "a raster has no labels");
            // Refused for a raster given, the selection stays as it was.
            app.session.run("layer.select", &json!({"layer": cities})).unwrap();
            assert!(app.run("ui.pane.labeling", json!({"layer": raster})).is_err());
            assert_eq!(app.session.active_layer, Some(cities), "a refusal doesn't select the raster");
        }
        app.session.run("layer.select", &json!({"layer": cities})).unwrap();
        assert!(app.run("ui.pane.labeling", json!({"layer": "No Such Layer"})).is_err());
        assert_eq!(app.session.active_layer, Some(cities), "nor an unknown layer");
        app.session.active_layer = None;
        app.run("ui.pane.labeling", json!({"layer": cities})).unwrap();
        assert_eq!(app.session.active_layer, Some(cities), "the layer given is selected");
        assert_eq!(app.ui.right, Some(RightPane::Labeling));
        app.run("ui.pane.labeling", json!({"close": true})).unwrap();
        assert!(!app.ui.right_open.contains(&RightPane::Labeling));
    }

    #[test]
    fn the_pane_runs_the_label_commands_on_the_selected_layer() {
        let app = with_cities();
        let before = app.session.map().layer(app.session.active_layer.unwrap()).unwrap().labels.clone();
        let mut h = harness(app);
        h.run();
        h.get_by_label("Label features in this layer").click();
        h.run();
        h.get_by_label("Below Right").click();
        h.run();
        h.get_by_label("Bold").click();
        h.run();
        let app = h.state();
        let l = app.session.map().layer(app.session.active_layer.unwrap()).unwrap();
        assert_ne!(l.labels.enabled, before.enabled, "the checkbox toggles labels");
        assert_eq!(l.labels.default.placement, rustgis_engine::map::LabelPlacement::BelowRight);
        assert_ne!(l.labels.default.bold, before.default.bold, "bold toggles");
        // Each change is its own command, so each undoes on its own.
        let app = h.state_mut();
        app.run("edit.undo", json!(null)).unwrap();
        let l = app.session.map().layer(app.session.active_layer.unwrap()).unwrap();
        assert_eq!(l.labels.default.bold, before.default.bold, "undo takes back the last change only");
        assert_eq!(l.labels.default.placement, rustgis_engine::map::LabelPlacement::BelowRight);
    }

    /// Record Task names the layer in a step made from the pane, as it does for the ribbon, so the
    /// step replays on that layer by name (#617 review).
    #[test]
    fn a_step_recorded_from_the_pane_names_the_layer() {
        let mut app = with_cities();
        app.ws.recording = Some(Vec::new());
        let mut h = harness(app);
        h.run();
        h.get_by_label("Below Right").click();
        h.run();
        h.get_by_label("Bold").click();
        h.run();
        let steps = h.state().ws.recording.clone().unwrap();
        let recorded: Vec<(&str, Option<&serde_json::Value>)> = steps.iter().map(|s| (s.command.as_str(), s.params.get("layer"))).collect();
        assert_eq!(
            recorded,
            [("labels.placement", Some(&json!("Cities"))), ("labels.style", Some(&json!("Cities")))],
            "each step names the layer, not its id"
        );
    }

    /// A pick left pending on one layer when the selection moves to another mid-pick (the
    /// Assistant, a shortcut) is finished as its own step, not left as an unrecorded preview.
    #[test]
    fn a_pick_pending_on_another_layer_is_finished_first() {
        let mut app = with_cities();
        app.ws.recording = Some(Vec::new());
        let cities = app.session.active_layer.unwrap();
        let countries = layer_id(&app, "Countries");
        let color = |app: &RustGisApp, id| app.session.map().layer(id).unwrap().labels.default.color;
        let (cities_before, countries_before) = (color(&app, cities), color(&app, countries));
        let steps = app.session.undo_depth();
        let mut h = egui_kittest::Harness::builder().build_ui_state(
            |ui, (app, pick): &mut (RustGisApp, Option<Rgba>)| {
                if let Some(c) = pick.take() {
                    let l = app.session.map().layer(app.session.active_layer.unwrap()).unwrap().clone();
                    super::color_drag(app, ui, &l, c);
                }
                super::finish_color_drag(app, ui);
            },
            (app, None),
        );
        let button = |pressed| egui::Event::PointerButton {
            pos: egui::pos2(4.0, 4.0),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let (red, blue) = (Rgba::new(200, 10, 10, 255), Rgba::new(10, 10, 200, 255));
        h.event(button(true));
        h.step();
        h.state_mut().1 = Some(red);
        h.step();
        // The pointer still down, the selection moves and the picker reports a colour for it.
        h.state_mut().0.session.run("layer.select", &json!({"layer": countries})).unwrap();
        h.state_mut().1 = Some(blue);
        h.step();
        let app = &h.state().0;
        assert_eq!(color(app, cities), red, "the first pick is kept on its layer");
        assert_eq!(app.session.undo_depth(), steps + 1, "and run as a step of its own");
        assert_eq!(color(app, countries), blue, "the new pick previews");
        assert_eq!(app.session.active_layer, Some(countries), "the selection stays where it moved");
        h.event(button(false));
        h.step();
        let app = &mut h.state_mut().0;
        assert_eq!(app.session.undo_depth(), steps + 2);
        // Both steps name their layer, the first one too, though it was no longer selected.
        let steps_recorded: Vec<_> =
            app.ws.recording.iter().flatten().map(|s| (s.command.clone(), s.params.get("layer").cloned())).collect();
        assert_eq!(
            steps_recorded,
            [("labels.style".to_string(), Some(json!("Cities"))), ("labels.style".to_string(), Some(json!("Countries")))]
        );
        app.run("edit.undo", json!(null)).unwrap();
        assert_eq!(color(app, countries), countries_before);
        app.run("edit.undo", json!(null)).unwrap();
        assert_eq!(color(app, cities), cities_before, "each pick undoes on its own");
    }

    #[test]
    fn a_colour_pick_is_one_undo_step() {
        let app = with_cities();
        let id = app.session.active_layer.unwrap();
        let before = app.session.map().layer(id).unwrap().labels.default.color;
        let steps = app.session.undo_depth();
        // The picker's colour each frame, if it reports one, as `text_symbol` hands it on.
        let mut h = egui_kittest::Harness::builder().build_ui_state(
            |ui, (app, pick): &mut (RustGisApp, Option<Rgba>)| {
                if let Some(c) = pick.take() {
                    let l = app.session.map().layer(app.session.active_layer.unwrap()).unwrap().clone();
                    super::color_drag(app, ui, &l, c);
                }
                super::finish_color_drag(app, ui);
            },
            (app, None),
        );
        let button = |pressed| egui::Event::PointerButton {
            pos: egui::pos2(4.0, 4.0),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        h.event(button(true));
        h.step();
        let mut picked = before;
        for c in [[200u8, 10, 10], [10, 200, 10], [10, 10, 200]] {
            picked = Rgba::new(c[0], c[1], c[2], 255);
            h.state_mut().1 = Some(picked);
            h.step();
            let app = &h.state().0;
            assert_eq!(app.session.map().layer(id).unwrap().labels.default.color, picked, "the map previews the pick");
            assert_eq!(app.session.undo_depth(), steps, "no command while the pointer is down");
        }
        h.event(button(false));
        h.step();
        let app = &mut h.state_mut().0;
        assert_eq!(app.session.map().layer(id).unwrap().labels.default.color, picked);
        assert_eq!(app.session.undo_depth(), steps + 1, "letting go runs labels.style once");
        app.run("edit.undo", json!(null)).unwrap();
        assert_eq!(app.session.map().layer(id).unwrap().labels.default.color, before, "one undo takes the whole pick back");
    }

    fn clause(app: &RustGisApp) -> Option<String> {
        app.session.map().layer(app.session.active_layer.unwrap()).unwrap().labels.default.where_.clone()
    }

    /// Opens the Class section's SQL tab.
    fn sql_tab(app: RustGisApp) -> egui_kittest::Harness<'static, RustGisApp> {
        let mut h = harness(app);
        h.run();
        h.get_by_label("SQL").click();
        h.run();
        h
    }

    fn type_clause(h: &mut egui_kittest::Harness<'static, RustGisApp>, text: &str) {
        h.get_by_role(egui::accesskit::Role::MultilineTextInput).focus();
        h.run();
        h.get_by_role(egui::accesskit::Role::MultilineTextInput).type_text(text);
        h.run();
    }

    /// The SQL tab writes a draft: typing applies nothing, Check counts, Apply sets the clause in
    /// one undo step, and Clear takes it away in another (#618).
    #[test]
    fn the_sql_tab_applies_a_clause_in_one_step() {
        let app = with_cities();
        let steps = app.session.undo_depth();
        let mut h = sql_tab(app);
        h.get_by_label("There is no expression defined.");
        h.get_by_label("+ New expression").click();
        h.run();
        type_clause(&mut h, "pop_max > 5000000");
        assert_eq!((clause(h.state()), h.state().session.undo_depth()), (None, steps), "typing applies nothing");
        h.get_by_label("Not applied yet.");
        h.get_by_label("Check").click();
        h.run();
        let l = h.state().session.map().layer(h.state().session.active_layer.unwrap()).unwrap().clone();
        let n = rustgis_engine::data::query::select("pop_max > 5000000", &l.dataset).unwrap().len();
        let all = l.drawn_rows(h.state().session.map().scale()).iter().filter(|r| **r).count();
        h.get_by_label(&format!("Valid: {n} of {all} features match"));
        h.get_by_label("Apply").click();
        h.run();
        assert_eq!(clause(h.state()).as_deref(), Some("pop_max > 5000000"));
        assert_eq!(h.state().session.undo_depth(), steps + 1, "one step");
        assert!(h.query_by_label("Not applied yet.").is_none());
        h.get_by_label("Clear").click();
        h.run();
        assert_eq!((clause(h.state()), h.state().session.undo_depth()), (None, steps + 2));
        h.get_by_label("There is no expression defined.");
        let app = h.state_mut();
        app.run("edit.undo", json!(null)).unwrap();
        assert_eq!(clause(app).as_deref(), Some("pop_max > 5000000"), "undo brings the clause back");
    }

    /// A clause that doesn't read says why, and Apply leaves the layer as it was.
    #[test]
    fn a_clause_that_doesnt_read_is_not_applied() {
        let app = with_cities();
        let steps = app.session.undo_depth();
        let mut h = sql_tab(app);
        h.get_by_label("+ New expression").click();
        h.run();
        type_clause(&mut h, "pop_max >");
        assert!(h.query_by_label_contains("Invalid:").is_some());
        h.get_by_label("Apply").click();
        h.run();
        assert_eq!((clause(h.state()), h.state().session.undo_depth()), (None, steps), "Apply is off");
    }

    /// A draft follows the clause when it changes underneath (undo, a script), until it is edited;
    /// another layer has its own clause, and the draft is dropped.
    #[test]
    fn the_draft_follows_the_layer_and_its_clause() {
        let app = with_cities();
        let mut h = sql_tab(app);
        h.state_mut().run("labels.class.update", json!({"where": "pop_max > 1"})).unwrap();
        h.run();
        assert_eq!(h.get_by_role(egui::accesskit::Role::MultilineTextInput).value().as_deref(), Some("pop_max > 1"));
        type_clause(&mut h, " AND pop_max < 9");
        h.state_mut().run("labels.class.update", json!({"where": "pop_max > 2"})).unwrap();
        h.run();
        let text = h.get_by_role(egui::accesskit::Role::MultilineTextInput).value().unwrap_or_default();
        assert!(text.contains("AND pop_max < 9") && text != "pop_max > 2", "an edited draft is kept: {text}");
        h.get_by_label("Not applied yet.");
        let countries = layer_id(h.state(), "Countries");
        h.state_mut().session.run("layer.select", &json!({"layer": countries})).unwrap();
        h.run();
        h.get_by_label("There is no expression defined.");
    }

    /// Classes made with commands, and a hidden Default, are said in the SQL tab; Show labels
    /// shows Default again.
    #[test]
    fn the_sql_tab_says_what_else_decides_the_labels() {
        let mut app = with_cities();
        app.run("labels.class.add", json!({"name": "Big", "where": "pop_max > 5000000"})).unwrap();
        app.run("labels.class.update", json!({"visible": false})).unwrap();
        let mut h = sql_tab(app);
        assert!(h.query_by_label_contains("Other label classes on this layer (made with commands): 1.").is_some());
        h.get_by_label("Show labels").click();
        h.run();
        let l = &h.state().session.map().layer(h.state().session.active_layer.unwrap()).unwrap().labels;
        assert!(l.default.visible);
    }

    /// A draft belongs to its layer in its project: another project, though it reuses the
    /// layer's id, starts without it.
    #[test]
    fn a_draft_is_dropped_with_its_project() {
        let app = with_cities();
        let mut h = sql_tab(app);
        h.get_by_label("+ New expression").click();
        h.run();
        type_clause(&mut h, "pop_max > 5000000");
        h.get_by_label("Not applied yet.");
        let app = h.state_mut();
        app.run("project.sample", json!(null)).unwrap();
        let cities = layer_id(app, "Cities");
        app.session.run("layer.select", &json!({"layer": cities})).unwrap();
        h.run();
        h.get_by_label("There is no expression defined.");
    }

    /// A Check's count shows while nothing has changed since: after a change, it is asked again.
    #[test]
    fn a_check_is_dropped_after_a_change() {
        let app = with_cities();
        let mut h = sql_tab(app);
        h.get_by_label("+ New expression").click();
        h.run();
        type_clause(&mut h, "pop_max > 5000000");
        h.get_by_label("Check").click();
        h.run();
        assert!(h.query_by_label_contains("features match").is_some());
        h.state_mut().run("labels.enable", json!({"on": true})).unwrap();
        h.run();
        assert!(h.query_by_label_contains("features match").is_none(), "the count may be stale");
        // A zoom changes no revision, but the scale: display filters may draw other features.
        h.get_by_label("Check").click();
        h.run();
        assert!(h.query_by_label_contains("features match").is_some());
        h.state_mut().session.map_mut().camera.res *= 2.0;
        h.run();
        assert!(h.query_by_label_contains("features match").is_none(), "the count was for another scale");
    }

    /// A clause whose field was deleted since says so: it no longer reads, and labels nothing.
    #[test]
    fn a_clause_that_no_longer_reads_says_so() {
        let mut app = with_cities();
        app.run("labels.class.update", json!({"where": "pop_max > 1"})).unwrap();
        app.run("edit.deleteField", json!({"field": "pop_max"})).unwrap();
        let h = sql_tab(app);
        assert!(h.query_by_label_contains("This clause no longer reads").is_some());
    }

    /// A clause applied from the pane is recorded on the layer by name, for Default (no class).
    #[test]
    fn a_clause_recorded_from_the_pane_names_the_layer() {
        let mut app = with_cities();
        app.ws.recording = Some(Vec::new());
        let mut h = sql_tab(app);
        h.get_by_label("+ New expression").click();
        h.run();
        type_clause(&mut h, "pop_max > 5000000");
        h.get_by_label("Apply").click();
        h.run();
        let steps = h.state().ws.recording.clone().unwrap();
        let last = steps.last().unwrap();
        assert_eq!(last.command, "labels.class.update");
        assert_eq!(last.params.get("layer"), Some(&json!("Cities")));
        assert_eq!(last.params.get("class"), None);
    }
}
