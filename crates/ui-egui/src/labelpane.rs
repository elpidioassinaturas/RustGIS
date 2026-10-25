//! The Labeling pane (#617): every label setting of the selected feature layer in one place, opened
//! from a layer's right-click menu (Labeling Properties…) or View › Windows › Labeling. The ribbon
//! keeps its label groups; both run the same `labels.*` commands, so the pane holds no state of its
//! own (it reads the layer each frame, as the Symbology pane does) and every change undoes. It
//! edits one label class at a time, the one chosen in its header (#722); the ribbon's label groups
//! edit and choose the same one (#858).

use egui::{RichText, Ui, vec2};
use rustgis_engine::map::symbology::{DEFAULT_LABEL_CLASS, LABEL_SIZE, LabelClass, Labeling, MAX_LABEL_CLASSES, label_class_name};
use rustgis_engine::map::{LabelPlacement, Layer, Rgba};
use rustgis_engine::render::families;
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
    let c = chosen(ui, app, &l);
    classes(app, ui, &l, &c, t);
    class(app, ui, &l, &c, t);
    text_symbol(app, ui, &l, &c);
    position(app, ui, &l, &c);
    visibility(app, ui, &c);
    map_labeling(app, ui);
}

/// The label class the pane's sections and the ribbon's label groups show and edit (#722).
pub(crate) struct Class<'a> {
    pub(crate) it: &'a LabelClass,
    /// `None` for Default: the commands change it when they get no class, so a step recorded on
    /// it stays as it was before classes could be chosen.
    pub(crate) name: Option<&'a str>,
}

impl Class<'_> {
    /// A command's parameters for this class.
    pub(crate) fn params(&self, p: Value) -> Value {
        named(p, "class", self.name)
    }
}

/// `p` with `name` as its `key`, or as it is without one.
pub(crate) fn named(mut p: Value, key: &str, name: Option<&str>) -> Value {
    if let (Some(n), Some(o)) = (name, p.as_object_mut()) {
        o.insert(key.into(), json!(n));
    }
    p
}

/// The class `name` names on `labels` to change: Default when none.
pub(crate) fn class_mut<'a>(labels: &'a mut Labeling, name: Option<&str>) -> Option<&'a mut LabelClass> {
    match name {
        None => Some(&mut labels.default),
        Some(n) => labels.class_mut(n),
    }
}

/// The classes listed before Default, as many as count.
fn extras(l: &Layer) -> impl Iterator<Item = &LabelClass> {
    l.labels.classes.iter().take(MAX_LABEL_CLASSES)
}

/// A class's name as the pane shows it: Default's own once renamed, else "Default" translated.
fn shown(c: &Class) -> String {
    match c.name {
        None if c.it.name == DEFAULT_LABEL_CLASS => tl!("Default").to_string(),
        _ => c.it.name.clone(),
    }
}

/// `l` as a layer of this map of this project: another map, or another project, may reuse its id.
/// A map goes by its place and its name, as Define Projection's does: deleting a map moves the
/// next one into its place, and a copy keeps its layer ids.
fn layer_key(app: &RustGisApp, l: &Layer) -> egui::Id {
    egui::Id::unique((app.session.opened, app.session.project.active_map, app.session.map().name.as_str(), l.id))
}

/// Where the class chosen on a layer is kept.
fn chosen_key(app: &RustGisApp, l: &Layer) -> egui::Id {
    egui::Id::unique(("label_pane_class", layer_key(app, l)))
}

/// The class chosen on `l`, by the name it has now; Default when none is, or when the one chosen
/// went away (an undo, a script, the Assistant).
pub(crate) fn chosen<'a>(ui: &Ui, app: &RustGisApp, l: &'a Layer) -> Class<'a> {
    let name = ui.data(|d| d.get_temp::<String>(chosen_key(app, l)));
    match name.and_then(|n| extras(l).find(|c| c.name.eq_ignore_ascii_case(&n))) {
        Some(it) => Class { it, name: Some(&it.name) },
        None => Class { it: &l.labels.default, name: None },
    }
}

/// The class chooser, the pane's and the ribbon's: a class chosen in either is chosen in both. Its
/// id is made within the parent `Ui`, so the two don't clash.
pub(crate) fn class_combo(app: &RustGisApp, ui: &mut Ui, l: &Layer, c: &Class, width: f32) -> egui::Response {
    egui::ComboBox::from_id_salt("label_pane_class")
        .selected_text(shown(c))
        .width(width)
        .show_ui(ui, |ui| {
            for k in extras(l) {
                if ui.selectable_label(c.name == Some(k.name.as_str()), &k.name).clicked() {
                    choose(ui, app, l, Some(&k.name));
                }
            }
            let default = Class { it: &l.labels.default, name: None };
            if ui.selectable_label(c.name.is_none(), shown(&default)).clicked() {
                choose(ui, app, l, None);
            }
        })
        .response
}

fn choose(ui: &Ui, app: &RustGisApp, l: &Layer, name: Option<&str>) {
    let key = chosen_key(app, l);
    ui.data_mut(|d| match name {
        Some(n) => {
            d.insert_temp(key, n.to_string());
        }
        None => d.remove::<String>(key),
    });
}

/// Runs a label command from the pane on the active layer, the one it shows; a refusal shows as a
/// notice. The commands don't name the layer, as the ribbon's don't: a step recorded from them
/// (Record Task) then names it, so it replays on that layer by name (`RustGisApp::record_step`).
fn run(app: &mut RustGisApp, id: &str, params: Value) -> Option<Value> {
    app.run(id, params).map_err(|e| app.session.notify(e, true)).ok()
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

/// The chosen class's field and SQL clause (#618) choosing the features it labels.
fn class(app: &mut RustGisApp, ui: &mut Ui, l: &Layer, c: &Class, t: &Tokens) {
    section(ui, "Class", |ui| {
        let key = egui::Id::unique("label_pane_class_tab");
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
            ClassTab::Field => field(app, ui, l, c),
            ClassTab::Sql => sql(app, ui, l, c, t),
        }
    });
}

/// A class being renamed: the name typed so far, for the class it was started on.
#[derive(Clone)]
struct Renaming {
    /// The project, map, layer and class it was started on.
    owner: egui::Id,
    text: String,
    focused: bool,
    /// The pass it was last shown on: one the pane skipped (closed, or behind another tab) ends it.
    pass: u64,
}

/// The pane's header for the chosen class (#858): its chooser, a row with New, Rename, Move Up,
/// Move Down and Remove for it (#722), and Visible. Default, last in the list, can be renamed or
/// hidden, but not removed or moved.
fn classes(app: &mut RustGisApp, ui: &mut Ui, l: &Layer, c: &Class, t: &Tokens) {
    let rename_key = egui::Id::unique("label_pane_rename");
    let owner = egui::Id::unique((layer_key(app, l), c.name));
    // A rename started on another class or layer (chosen since, or by an undo), or left when the
    // pane was last hidden, is dropped.
    let pass = ui.ctx().cumulative_pass_nr();
    let renaming = ui.data(|d| d.get_temp::<Renaming>(rename_key)).filter(|r| r.owner == owner && r.pass + 1 >= pass);
    ui.data_mut(|d| d.remove::<Renaming>(rename_key));
    let editing = renaming.is_some();
    grid(ui, "class_pick", |ui| {
        let input_label = ui.label(tl!("Class"));
        match renaming {
            Some(r) => rename_box(app, ui, l, c, rename_key, input_label.id, r),
            None => {
                class_combo(app, ui, l, c, 180.0).labelled_by(input_label.id);
            }
        }
        ui.end_row();
    });
    // Off while the class is renamed: a click on them ends the rename (the box loses focus), so
    // they would act on the class by the name it had before.
    ui.add_enabled_ui(!editing, |ui| {
        let count = extras(l).count();
        let at = c.name.and_then(|n| extras(l).position(|k| k.name == n));
        ui.horizontal(|ui| {
            let full = l.labels.classes.len() >= MAX_LABEL_CLASSES;
            let max = MAX_LABEL_CLASSES.to_string();
            let new = ui
                .add_enabled(!full, egui::Button::new(tl!("New")))
                .on_hover_text(tl!("Add a class with this one's field, text symbol, placement and range."))
                .on_disabled_hover_text(crate::i18n::fmt("A layer keeps at most {max} label classes besides Default.", &[("max", &max)]));
            if new.clicked() {
                let p = named(json!({"name": new_name(&l.labels)}), "from", c.name);
                if let Some(added) = run(app, "labels.class.add", p) {
                    choose(ui, app, l, added.get("class").and_then(Value::as_str));
                }
            }
            if ui.button(tl!("Rename")).clicked() {
                let r = Renaming { owner, text: c.it.name.clone(), focused: false, pass };
                ui.data_mut(|d| d.insert_temp(rename_key, r));
            }
            let up = at.filter(|i| *i > 0).map(|i| i - 1);
            let down = at.filter(|i| i + 1 < count).map(|i| i + 1);
            let moves = [
                ("up", "Move Up", "Move up: labels of the classes listed first are placed first when they compete for room.", up),
                ("down", "Move Down", "Move down: labels of the classes listed first are placed first when they compete for room.", down),
            ];
            for (icon, label, tip, to) in moves {
                let moved = ui
                    .add_enabled_ui(to.is_some(), |ui| {
                        // Named for screen readers (and tests) by its label, as the Bookmarks
                        // pane's icons; its one tooltip says why the order matters.
                        let enabled = ui.is_enabled();
                        let r = crate::widgets::icon_button(ui, icon, tl!(tip));
                        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, tl!(label)));
                        r
                    })
                    .inner;
                if moved.clicked() {
                    run(app, "labels.class.move", c.params(json!({"to": to})));
                }
            }
            // At the far end and in red: it is the one that deletes, and its name is close to
            // Rename's.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let remove = ui
                    .add_enabled(at.is_some(), egui::Button::new(RichText::new(tl!("Remove")).color(t.red)))
                    .on_disabled_hover_text(tl!("Default can't be removed. You can rename or hide it."));
                if remove.clicked() {
                    // Default chosen outright: a class added later by the removed one's name (an
                    // undo, a script) isn't chosen again by the name left behind.
                    if run(app, "labels.class.remove", c.params(json!({}))).is_some() {
                        choose(ui, app, l, None);
                    }
                }
            });
        });
        let mut visible = c.it.visible;
        if ui.checkbox(&mut visible, tl!("Visible")).on_hover_text(tl!("A hidden class leaves its features unlabelled.")).changed() {
            run(app, "labels.class.update", c.params(json!({"visible": visible})));
        }
    });
}

/// The box the chosen class is renamed in: Enter (or clicking away) renames it, Escape leaves it as
/// it was. A name refused (empty, or another class's) keeps the box open, with the notice saying why.
fn rename_box(app: &mut RustGisApp, ui: &mut Ui, l: &Layer, c: &Class, key: egui::Id, label: egui::Id, mut r: Renaming) {
    let keys = egui::EventFilter { escape: true, ..Default::default() };
    let edit = ui.add(egui::TextEdit::singleline(&mut r.text).desired_width(180.0).event_filter(keys)).labelled_by(label);
    if !r.focused {
        edit.request_focus();
        r.focused = true;
    }
    if edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        return;
    }
    if edit.lost_focus() {
        let name = label_class_name(&r.text);
        if name == c.it.name {
            return;
        }
        if run(app, "labels.class.rename", json!({"class": c.it.name, "name": name})).is_some() {
            // Default stays chosen by no name; another class goes by its new one.
            if c.name.is_some() {
                choose(ui, app, l, Some(&name));
            }
            return;
        }
        r.focused = false;
    }
    r.pass = ui.ctx().cumulative_pass_nr();
    ui.data_mut(|d| d.insert_temp(key, r));
}

/// The first "Class N" no class on `labels` goes by.
fn new_name(labels: &Labeling) -> String {
    (1..=MAX_LABEL_CLASSES + 2)
        .map(|n| crate::i18n::fmt("Class {n}", &[("n", &n.to_string())]))
        .find(|n| labels.class(n).is_none())
        .unwrap_or_default()
}

/// The field the class labels with.
fn field(app: &mut RustGisApp, ui: &mut Ui, l: &Layer, c: &Class) {
    grid(ui, "class", |ui| {
        let input_label = ui.label(tl!("Field"));
        let cur = &c.it.field;
        egui::ComboBox::from_id_salt("label_pane_field")
            .selected_text(cur.as_str())
            .width(180.0)
            .show_ui(ui, |ui| {
                for f in &l.dataset.fields {
                    if ui.selectable_label(f.name == *cur, &f.name).clicked() {
                        run(app, "labels.field", c.params(json!({"field": f.name})));
                    }
                }
            })
            .response
            .labelled_by(input_label.id);
        ui.end_row();
    });
}

/// The class's SQL clause: its labels, and their style, go only to the features that match it;
/// none takes every feature.
fn sql(app: &mut RustGisApp, ui: &mut Ui, l: &Layer, c: &Class, t: &Tokens) {
    let labels = &l.labels;
    if !labels.enabled {
        ui.label(RichText::new(tl!("Labels are off for this layer: the clause applies once they are on.")).small().color(t.text_dim));
    }
    if !c.it.visible {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tl!("This class is hidden, so it labels nothing.")).small().color(t.text_dim));
            if ui.small_button(tl!("Show labels")).clicked() {
                run(app, "labels.class.update", c.params(json!({"visible": true})));
            }
        });
    }
    if !labels.classes.is_empty() {
        let said = tl!("Each class labels the features its own clause matches, so a feature can get a label from more than one.");
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
    // A draft belongs to this class of this layer.
    let owner = egui::Id::unique((layer_key(app, l), c.name));
    // A count holds for this project state at this scale (display filters follow the scale).
    let version = egui::Id::unique((app.session.revision, scale.to_bits())).value();
    let applied = match crate::sqlbox::show(ui, ("label_sql", owner), owner, version, c.it.where_.as_deref(), &l.dataset, count) {
        Some(crate::sqlbox::Apply::Clause(w)) => json!({"where": w}),
        Some(crate::sqlbox::Apply::Clear) => json!({"where": null}),
        None => return,
    };
    run(app, "labels.class.update", c.params(applied));
}

fn text_symbol(app: &mut RustGisApp, ui: &mut Ui, l: &Layer, c: &Class) {
    section(ui, "Text Symbol", |ui| {
        grid(ui, "symbol", |ui| {
            font(app, ui, l, c);
            ui.end_row();
            let input_label = ui.label(tl!("Size"));
            // The whole range `labels.style` accepts (#457); the map follows a drag and letting
            // go runs the command once, so a drag is one undo step.
            let mut size = f64::from(c.it.size);
            let r = ui.add(egui::DragValue::new(&mut size).range(LABEL_SIZE).speed(0.1).suffix(" pt")).labelled_by(input_label.id);
            let id = l.id;
            crate::ribbon::live_drag(
                app,
                ui,
                &r,
                f64::from(c.it.size),
                size,
                |app, x| {
                    if let Some(k) = app.session.map_mut().layer_mut(id).and_then(|l| class_mut(&mut l.labels, c.name)) {
                        k.size = x.clamp(*LABEL_SIZE.start(), *LABEL_SIZE.end()) as f32;
                    }
                },
                |app, x| {
                    run(app, "labels.style", c.params(json!({"size": x})));
                },
            );
            ui.end_row();
            ui.label(tl!("Color"));
            let mut color = c32(c.it.color);
            if crate::widgets::color_button(ui, &mut color).changed() {
                color_drag(app, ui, l, c.name, rgba(color));
            }
            ui.end_row();
            ui.label("");
            ui.horizontal(|ui| {
                let mut bold = c.it.bold;
                if ui.checkbox(&mut bold, tl!("Bold")).changed() {
                    run(app, "labels.style", c.params(json!({"bold": bold})));
                }
                let mut italic = c.it.italic;
                if ui.checkbox(&mut italic, tl!("Italic")).changed() {
                    run(app, "labels.style", c.params(json!({"italic": italic})));
                }
                let mut halo = c.it.halo;
                if ui.checkbox(&mut halo, tl!("Halo")).changed() {
                    run(app, "labels.style", c.params(json!({"halo": halo})));
                }
            });
            ui.end_row();
        });
    });
}

/// The most installed families the font list shows at once (a machine can have thousands); the
/// rest are found by typing part of a name.
const MAX_FONTS_SHOWN: usize = 200;

/// The installed family the labels are drawn in: the label font, or one picked from the
/// installed families (found by typing part of the name). A family this machine lacks, from a
/// project made on another, is shown as missing and drawn in the label font.
fn font(app: &mut RustGisApp, ui: &mut Ui, l: &Layer, c: &Class) {
    let input_label = ui.label(tl!("Font"));
    // The installed fonts are indexed on a thread of their own; until then there's none to pick.
    let Some(installed) = families::installed_now() else {
        families::warm_up();
        ui.add_enabled(false, egui::Button::new(tl!("Reading the installed fonts…"))).labelled_by(input_label.id);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        return;
    };
    let cur = c.it.font.as_deref();
    let shown = match cur {
        None => tl!("Label font").to_string(),
        Some(name) if families::find(name).is_some() => name.to_string(),
        Some(name) => crate::i18n::fmt("{name} (not installed)", &[("name", name)]),
    };
    let search_id = egui::Id::unique(("label_pane_font_search", l.id));
    egui::ComboBox::from_id_salt("label_pane_font")
        .selected_text(shown)
        .width(180.0)
        .height(320.0)
        .show_ui(ui, |ui| {
            let mut search = ui.data_mut(|d| d.get_temp::<String>(search_id)).unwrap_or_default();
            if ui.add(egui::TextEdit::singleline(&mut search).hint_text(tl!("Search fonts"))).changed() {
                ui.data_mut(|d| d.insert_temp(search_id, search.clone()));
            }
            if ui.selectable_label(cur.is_none(), tl!("Label font")).clicked() {
                run(app, "labels.style", c.params(json!({"font": ""})));
            }
            let search = search.to_lowercase();
            let mut matching = installed.iter().filter(|f| f.listed && (search.is_empty() || f.name.to_lowercase().contains(&search)));
            for f in matching.by_ref().take(MAX_FONTS_SHOWN) {
                let on = cur.is_some_and(|c| c.eq_ignore_ascii_case(&f.name));
                if ui.selectable_label(on, &f.name).clicked() && !on {
                    run(app, "labels.style", c.params(json!({"font": f.name})));
                }
            }
            let more = matching.count();
            if more > 0 {
                ui.weak(crate::i18n::fmt("{count} more: type part of a name", &[("count", &more.to_string())]));
            }
        })
        .response
        .labelled_by(input_label.id);
}

fn position(app: &mut RustGisApp, ui: &mut Ui, l: &Layer, c: &Class) {
    section(ui, "Position", |ui| {
        let cur = c.it.placement;
        let pick = |app: &mut RustGisApp, ui: &mut Ui, pl: LabelPlacement| {
            if ui.add_sized(vec2(92.0, 22.0), egui::Button::selectable(cur == pl, tl!(pl.label())).truncate()).clicked() && cur != pl {
                run(app, "labels.placement", c.params(json!({"position": pl.code()})));
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
        // Lines: the text along them, straight or bending with them.
        if l.geometry_type() == rustgis_engine::data::GeometryType::Polyline {
            ui.add_space(4.0);
            ui.label(RichText::new(tl!("Along lines")).size(11.5));
            ui.horizontal(|ui| {
                pick(app, ui, LabelPlacement::Parallel);
                pick(app, ui, LabelPlacement::Curved);
            });
        }
    });
}

fn visibility(app: &mut RustGisApp, ui: &mut Ui, c: &Class) {
    section(ui, "Visibility Range", |ui| {
        grid(ui, "range", |ui| {
            for (label, cur, key) in [("Out Beyond", c.it.min_scale, "min"), ("In Beyond", c.it.max_scale, "max")] {
                let input_label = ui.label(tl!(label));
                egui::ComboBox::from_id_salt(("label_pane_scale", key))
                    .selected_text(crate::ribbon::scale_label(cur))
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for s in std::iter::once(None).chain(crate::ribbon::SCALES.iter().map(|s| Some(*s))) {
                            if ui.selectable_label(s == cur, crate::ribbon::scale_label(s)).clicked() {
                                run(app, "labels.scaleRange", c.params(json!({key: s})));
                            }
                        }
                    })
                    .response
                    .labelled_by(input_label.id);
                ui.end_row();
            }
            for (label, cur, key) in [("Closest", c.it.closest, "closest"), ("Farthest", c.it.farthest, "farthest")] {
                let input_label = ui.label(tl!(label));
                let imperial = app.ui.measurement_imperial;
                let text =
                    |d: Option<f64>| d.map(|d| crate::measure_units::distance(d, imperial)).unwrap_or_else(|| tl!("<None>").to_string());
                egui::ComboBox::from_id_salt(("label_pane_distance", key))
                    .selected_text(text(cur))
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for d in std::iter::once(None).chain(DISTANCES.iter().map(|d| Some(*d))) {
                            if ui.selectable_label(d == cur, text(d)).clicked() {
                                run(app, "labels.distanceRange", c.params(json!({key: d})));
                            }
                        }
                    })
                    .response
                    .labelled_by(input_label.id);
                ui.end_row();
            }
        });
        let limited = [c.it.min_scale, c.it.max_scale, c.it.closest, c.it.farthest].iter().any(Option::is_some);
        if ui
            .add_enabled(limited, egui::Button::new(tl!("Clear Limits")))
            .on_hover_text(tl!("Draw labels at every scale and distance."))
            .clicked()
        {
            run(app, "labels.scaleRange", c.params(json!({"clear": true})));
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

/// Where a label colour being dragged in the pane or ribbon's picker waits.
const COLOR_DRAG: &str = "label_pane_color_drag";

/// A label colour being dragged: it previews in the project, and when the pointer is let go
/// `finish_color_drag` puts `before` back and runs labels.style once, so a pick is one undo step
/// holding the pre-drag colour (as the Symbology pane's class colours do, #314).
#[derive(Clone)]
struct ColorDrag {
    layer: u32,
    /// The class picked on: `None` for Default.
    class: Option<String>,
    before: Rgba,
    color: Rgba,
}

/// Previews a colour pick on its layer's class (`None`: Default) until `finish_color_drag` commits
/// the completed gesture.
pub(crate) fn color_drag(app: &mut RustGisApp, ui: &Ui, l: &Layer, class: Option<&str>, color: Rgba) {
    let key = egui::Id::unique(COLOR_DRAG);
    let mut pending = ui.data(|d| d.get_temp::<ColorDrag>(key));
    // One left pending on another layer or class (the selection changed mid-pick, from the
    // Assistant or a shortcut) is finished first rather than dropped with its preview in place.
    if let Some(other) = pending.take_if(|d| d.layer != l.id || d.class.as_deref() != class) {
        ui.data_mut(|d| d.remove::<ColorDrag>(key));
        apply_color_drag(app, other);
    }
    let Some(now) = class.map_or(Some(&l.labels.default), |n| l.labels.class(n)).map(|k| k.color) else { return };
    // The colour before the first frame of this pick, kept while it goes on.
    let before = pending.map_or(now, |d| d.before);
    ui.data_mut(|d| d.insert_temp(key, ColorDrag { layer: l.id, class: class.map(String::from), before, color }));
    if let Some(k) = app.session.map_mut().layer_mut(l.id).and_then(|l| class_mut(&mut l.labels, class)) {
        k.color = color;
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
    let key = egui::Id::unique(COLOR_DRAG);
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
    // A layer or class removed mid-pick took its preview with it: there is nothing to restore or
    // style.
    let Some(k) = app.session.map_mut().layer_mut(drag.layer).and_then(|l| class_mut(&mut l.labels, drag.class.as_deref())) else {
        return;
    };
    k.color = drag.before;
    if drag.before != drag.color {
        // On the layer the pick was made on, as the active one: the command reaches it by id
        // (a name two layers share would reach the first), and a recorded step names it, as
        // for the pane's other commands. The selection, if it moved mid-pick, is put back (as
        // `Session::run` does to check a named layer's conditions).
        let selected = app.session.active_layer.replace(drag.layer);
        run(app, "labels.style", named(json!({"color": drag.color.hex()}), "class", drag.class.as_deref()));
        app.session.active_layer = selected;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use egui_kittest::kittest::{NodeT, Queryable};
    use rustgis_engine::map::Rgba;
    use serde_json::json;

    use crate::{RightPane, RustGisApp};

    fn layer_id(app: &RustGisApp, name: &str) -> u32 {
        app.session.map().layers.iter().find(|l| l.name == name).unwrap().id
    }

    fn with_cities() -> RustGisApp {
        // Keep the font picker ready before the harness checks its role or settles the layout.
        super::families::installed();
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

    /// Inputs remain reachable by their purpose, independently of the selected value (#790).
    #[test]
    fn inputs_are_reachable_by_their_row_labels() {
        use egui::accesskit::Role;
        let mut h = harness(with_cities());
        h.run();
        for label in ["Class", "Field", "Font", "Out Beyond", "In Beyond", "Closest", "Farthest"] {
            assert!(h.query_by_role_and_label(Role::ComboBox, label).is_some(), "missing input: {label}");
        }
        assert!(h.query_by_role_and_label(Role::SpinButton, "Size").is_some());
        h.get_by_role_and_label(Role::ComboBox, "Field").click();
        h.run();
        h.get_by_label("name").click();
        h.run();
        assert_eq!(h.state().session.map().layer_by_name("Cities").unwrap().labels.default.field, "name");
        assert!(h.query_by_role_and_label(Role::ComboBox, "Field").is_some());
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
                    super::color_drag(app, ui, &l, None, c);
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
                    super::color_drag(app, ui, &l, None, c);
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

    /// The SQL tab says that every class labels what its clause matches, and that a hidden Default
    /// labels nothing; Show labels shows Default again.
    #[test]
    fn the_sql_tab_says_what_else_decides_the_labels() {
        let mut app = with_cities();
        app.run("labels.class.add", json!({"name": "Big", "where": "pop_max > 5000000"})).unwrap();
        app.run("labels.class.update", json!({"visible": false})).unwrap();
        let mut h = sql_tab(app);
        assert!(h.query_by_label_contains("Each class labels the features its own clause matches").is_some());
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

    fn labels(app: &RustGisApp) -> rustgis_engine::map::symbology::Labeling {
        app.session.map().layer(app.session.active_layer.unwrap()).unwrap().labels.clone()
    }

    /// The layer's class names, in the order they are listed.
    fn names(app: &RustGisApp) -> Vec<String> {
        labels(app).in_order().map(|c| c.name.clone()).collect()
    }

    /// The class the pane's chooser shows.
    fn shown(h: &egui_kittest::Harness<'static, RustGisApp>, name: &str) -> bool {
        h.query_all_by_role(egui::accesskit::Role::ComboBox).any(|n| n.value().as_deref() == Some(name))
    }

    /// Chooses class `to` in the pane's chooser, which shows `from`.
    fn pick_class(h: &mut egui_kittest::Harness<'static, RustGisApp>, from: &str, to: &str) {
        h.query_all_by_role(egui::accesskit::Role::ComboBox).find(|n| n.value().as_deref() == Some(from)).unwrap().click();
        h.run();
        h.get_by_label(to).click();
        h.run();
    }

    /// The pane edits the class chosen, and only it; its steps name it, and Default's name none
    /// (#722).
    #[test]
    fn the_chosen_class_is_the_one_edited() {
        let mut app = with_cities();
        app.run("labels.class.add", json!({"name": "Big", "where": "pop_max > 5000000"})).unwrap();
        app.ws.recording = Some(Vec::new());
        let before = labels(&app).default;
        let mut h = harness(app);
        h.run();
        pick_class(&mut h, "Default", "Big");
        h.get_by_label("Below Right").click();
        h.run();
        h.get_by_label("Bold").click();
        h.run();
        let l = labels(h.state());
        let big = l.class("Big").unwrap();
        assert_eq!((big.placement, big.bold), (rustgis_engine::map::LabelPlacement::BelowRight, !before.bold));
        assert_eq!((l.default.placement, l.default.bold), (before.placement, before.bold), "Default is left as it was");
        pick_class(&mut h, "Big", "Default");
        h.get_by_label("Halo").click();
        h.run();
        assert_ne!(labels(h.state()).default.halo, before.halo);
        let steps = h.state().ws.recording.clone().unwrap();
        let classes: Vec<_> = steps.iter().map(|s| (s.command.as_str(), s.params.get("class").cloned())).collect();
        assert_eq!(
            classes,
            [("labels.placement", Some(json!("Big"))), ("labels.style", Some(json!("Big"))), ("labels.style", None)],
            "Default's step names no class"
        );
    }

    /// Font, italic and line placement edits affect the chosen class and leave Default unchanged.
    #[test]
    fn font_italic_and_line_placement_edit_the_chosen_class() {
        let mut app = with_cities();
        let rivers = layer_id(&app, "Rivers");
        app.session.run("layer.select", &json!({"layer": rivers})).unwrap();
        app.run("labels.class.add", json!({"name": "Named"})).unwrap();
        app.run("labels.style", json!({"class": "Named", "font": "Missing test font"})).unwrap();
        let before = labels(&app).default;
        let steps = app.session.undo_depth();
        app.ws.recording = Some(Vec::new());
        let mut h = harness(app);
        h.run();
        pick_class(&mut h, "Default", "Named");
        h.get_by_label("Italic").click();
        h.run();
        h.get_by_label("Curved").click();
        h.run();
        h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Font").click();
        h.run();
        h.get_by_label("Label font").click();
        h.run();
        let l = labels(h.state());
        let named = l.class("Named").unwrap();
        assert_eq!(named.italic, !before.italic);
        assert_eq!(named.placement, rustgis_engine::map::LabelPlacement::Curved);
        assert_eq!(named.font, None);
        assert_eq!((l.default.italic, l.default.placement, l.default.font), (before.italic, before.placement, before.font));
        assert_eq!(h.state().session.undo_depth(), steps + 3);
        for step in h.state().ws.recording.as_ref().unwrap() {
            assert_eq!(step.params.get("class"), Some(&json!("Named")));
        }
        h.state_mut().run("edit.undo", json!(null)).unwrap();
        assert_eq!(labels(h.state()).class("Named").unwrap().font.as_deref(), Some("Missing test font"));
    }

    /// A class chosen that goes away (an undo, a script, the Assistant) leaves Default chosen.
    #[test]
    fn the_pane_goes_back_to_default_when_its_class_goes_away() {
        let mut app = with_cities();
        app.run("labels.class.add", json!({"name": "Big"})).unwrap();
        let before = labels(&app).default.bold;
        let mut h = harness(app);
        h.run();
        pick_class(&mut h, "Default", "Big");
        h.state_mut().run("labels.class.remove", json!({"class": "Big"})).unwrap();
        h.run();
        assert!(shown(&h, "Default"));
        h.get_by_label("Bold").click();
        h.run();
        assert_ne!(labels(h.state()).default.bold, before, "the pane edits Default again");
    }

    /// The class chosen belongs to its map: a copy of the map has the same layer ids, and starts on
    /// Default.
    #[test]
    fn the_chosen_class_stays_with_its_map() {
        let mut app = with_cities();
        app.run("labels.class.add", json!({"name": "Big"})).unwrap();
        let map = app.session.map().name.clone();
        let mut h = harness(app);
        h.run();
        pick_class(&mut h, "Default", "Big");
        let copy = h.state_mut().run("map.duplicate", json!({})).unwrap()["name"].as_str().unwrap().to_string();
        h.run();
        assert_eq!(h.state().session.map().name, copy, "the copy is in view");
        assert!(shown(&h, "Default"));
        h.state_mut().run("map.activate", json!({"name": map})).unwrap();
        let id = layer_id(h.state(), "Cities");
        h.state_mut().run("layer.select", json!({"layer": id})).unwrap();
        h.run();
        assert!(shown(&h, "Big"), "back on its map");
        // Deleted, its copy moves into its place, and still starts on Default.
        let at = h.state().session.project.active_map;
        h.state_mut().run("map.remove", json!({"name": map})).unwrap();
        h.state_mut().run("map.activate", json!({"name": copy})).unwrap();
        assert_eq!(h.state().session.project.active_map, at, "the copy is where its map was");
        let id = layer_id(h.state(), "Cities");
        h.state_mut().run("layer.select", json!({"layer": id})).unwrap();
        h.run();
        assert!(shown(&h, "Default"));
    }

    /// New, Move Up, Rename and Remove, under the header's chooser (#858), each run their command
    /// once: one undo step each.
    #[test]
    fn classes_are_made_renamed_ordered_and_removed_from_the_pane() {
        let app = with_cities();
        let steps = app.session.undo_depth();
        let mut h = harness(app);
        h.run();
        assert!(h.get_by_label("Remove").accesskit_node().is_disabled(), "Default stays");
        h.get_by_label("New").click();
        h.run();
        assert_eq!(names(h.state()), ["Class 1", "Default"]);
        assert!(shown(&h, "Class 1"), "the new class is chosen");
        h.get_by_label("New").click();
        h.run();
        assert_eq!(names(h.state()), ["Class 1", "Class 2", "Default"]);
        assert!(h.get_by_label("Move Down").accesskit_node().is_disabled(), "the last before Default");
        h.get_by_label("Move Up").click();
        h.run();
        assert_eq!(names(h.state()), ["Class 2", "Class 1", "Default"]);
        h.get_by_label("Rename").click();
        h.run();
        h.get_by_role(egui::accesskit::Role::TextInput).type_text(" big");
        h.run();
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(names(h.state()), ["Class 2 big", "Class 1", "Default"]);
        assert!(shown(&h, "Class 2 big"), "still chosen by its new name");
        h.get_by_label("Remove").click();
        h.run();
        assert_eq!(names(h.state()), ["Class 1", "Default"]);
        assert!(shown(&h, "Default"));
        assert_eq!(h.state().session.undo_depth(), steps + 5);
        // Undone, the class is back, but the pane doesn't choose it again by its name.
        h.state_mut().run("edit.undo", json!(null)).unwrap();
        h.run();
        assert_eq!(names(h.state()), ["Class 2 big", "Class 1", "Default"]);
        assert!(shown(&h, "Default"));
    }

    /// A name refused keeps the box open, saying why; Escape leaves the name as it was.
    #[test]
    fn a_refused_class_name_keeps_the_box_open() {
        let mut app = with_cities();
        app.run("labels.class.add", json!({"name": "Big"})).unwrap();
        app.run("labels.class.add", json!({"name": "Big 2"})).unwrap();
        let mut h = harness(app);
        h.run();
        pick_class(&mut h, "Default", "Big");
        h.get_by_label("Rename").click();
        h.run();
        h.get_by_role(egui::accesskit::Role::TextInput).type_text(" 2");
        h.run();
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(names(h.state()), ["Big", "Big 2", "Default"]);
        assert!(h.state().session.notices.iter().any(|n| n.error && n.text.contains("already a label class")));
        assert!(h.query_by_role(egui::accesskit::Role::TextInput).is_some(), "the box stays open");
        for off in ["New", "Rename", "Move Up", "Move Down", "Remove", "Visible"] {
            assert!(h.get_by_label(off).accesskit_node().is_disabled(), "{off} waits for the rename");
        }
        h.key_press(egui::Key::Escape);
        h.run();
        assert!(h.query_by_role(egui::accesskit::Role::TextInput).is_none());
        assert_eq!(names(h.state()), ["Big", "Big 2", "Default"]);
        assert!(shown(&h, "Big"));
    }

    /// A name that changes only in case is a rename, not another class's name.
    #[test]
    fn a_class_is_renamed_to_another_case() {
        let mut app = with_cities();
        app.run("labels.class.add", json!({"name": "Big"})).unwrap();
        let steps = app.session.undo_depth();
        let mut h = harness(app);
        h.run();
        pick_class(&mut h, "Default", "Big");
        h.get_by_label("Rename").click();
        h.run();
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.key_press(egui::Key::Backspace);
        h.run();
        h.get_by_role(egui::accesskit::Role::TextInput).type_text("big");
        h.run();
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(names(h.state()), ["big", "Default"]);
        assert!(shown(&h, "big"), "still chosen by its new name");
        assert_eq!(h.state().session.undo_depth(), steps + 1);
    }

    /// A rename left open when the pane is hidden is gone when it shows again.
    #[test]
    fn a_rename_ends_with_the_pane() {
        let mut app = with_cities();
        app.ui.right = Some(RightPane::Labeling);
        let mut fonts = false;
        // Drawn only while it is the right pane in view, as the app does.
        let mut h = egui_kittest::Harness::builder().with_accessibility_check(false).with_size(egui::vec2(360.0, 900.0)).build_ui_state(
            move |ui, app: &mut RustGisApp| {
                if !fonts {
                    crate::theme::install_fonts(ui.ctx(), &[], None);
                    fonts = true;
                    return;
                }
                if app.ui.right == Some(RightPane::Labeling) {
                    super::show(app, ui, &crate::theme::Tokens::light());
                }
            },
            app,
        );
        h.run();
        h.get_by_label("Rename").click();
        h.run();
        assert!(h.query_by_role(egui::accesskit::Role::TextInput).is_some());
        h.state_mut().ui.right = None;
        h.run();
        h.state_mut().ui.right = Some(RightPane::Labeling);
        h.run();
        assert!(h.query_by_role(egui::accesskit::Role::TextInput).is_none(), "the box is closed");
        assert!(shown(&h, "Default"));
    }

    /// New is off once the layer has as many classes as it keeps.
    #[test]
    fn new_is_off_at_the_class_limit() {
        let mut app = with_cities();
        for i in 0..rustgis_engine::map::symbology::MAX_LABEL_CLASSES {
            app.run("labels.class.add", json!({"name": format!("C{i}")})).unwrap();
        }
        let steps = app.session.undo_depth();
        let mut h = harness(app);
        h.run();
        assert!(h.get_by_label("New").accesskit_node().is_disabled());
        h.get_by_label("New").click();
        h.run();
        assert_eq!(h.state().session.undo_depth(), steps);
    }

    #[test]
    fn sql_drafts_survive_switching_label_classes() {
        let mut app = with_cities();
        app.run("labels.class.add", json!({"name": "Big"})).unwrap();
        let mut h = sql_tab(app);
        h.get_by_label("+ New expression").click();
        h.run();
        type_clause(&mut h, "pop_max > 123");
        pick_class(&mut h, "Default", "Big");
        h.get_by_label("+ New expression").click();
        h.run();
        type_clause(&mut h, "pop_max > 456");
        pick_class(&mut h, "Big", "Default");
        assert_eq!(h.get_by_role(egui::accesskit::Role::MultilineTextInput).value().as_deref(), Some("pop_max > 123"));
        pick_class(&mut h, "Default", "Big");
        assert_eq!(h.get_by_role(egui::accesskit::Role::MultilineTextInput).value().as_deref(), Some("pop_max > 456"));
    }

    /// The SQL tab shows and sets the chosen class's clause.
    #[test]
    fn the_sql_tab_edits_the_chosen_class() {
        let mut app = with_cities();
        app.run("labels.class.add", json!({"name": "Big", "where": "pop_max > 5000000"})).unwrap();
        let mut h = harness(app);
        h.run();
        pick_class(&mut h, "Default", "Big");
        h.get_by_label("SQL").click();
        h.run();
        assert_eq!(h.get_by_role(egui::accesskit::Role::MultilineTextInput).value().as_deref(), Some("pop_max > 5000000"));
        h.get_by_label("Clear").click();
        h.run();
        let l = labels(h.state());
        assert_eq!((l.class("Big").unwrap().where_.clone(), l.default.where_), (None, None));
    }

    /// A colour pick is kept on the class it was made on: one left pending when another class is
    /// chosen mid-pick is finished first, as its own step naming its class.
    #[test]
    fn a_colour_pick_stays_on_its_class() {
        let mut app = with_cities();
        app.run("labels.class.add", json!({"name": "Big"})).unwrap();
        app.ws.recording = Some(Vec::new());
        let before = labels(&app).default.color;
        let mut h = egui_kittest::Harness::builder().build_ui_state(
            |ui, (app, pick): &mut (RustGisApp, Option<(Option<&'static str>, Rgba)>)| {
                if let Some((class, c)) = pick.take() {
                    let l = app.session.map().layer(app.session.active_layer.unwrap()).unwrap().clone();
                    super::color_drag(app, ui, &l, class, c);
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
        h.state_mut().1 = Some((Some("Big"), red));
        h.step();
        let l = labels(&h.state().0);
        assert_eq!((l.class("Big").unwrap().color, l.default.color), (red, before), "the pick previews on Big only");
        h.state_mut().1 = Some((None, blue));
        h.step();
        h.event(button(false));
        h.step();
        let app = &h.state().0;
        let l = labels(app);
        assert_eq!((l.class("Big").unwrap().color, l.default.color), (red, blue));
        let classes: Vec<_> = app.ws.recording.iter().flatten().map(|s| s.params.get("class").cloned()).collect();
        assert_eq!(classes, [Some(json!("Big")), None]);
    }
}
