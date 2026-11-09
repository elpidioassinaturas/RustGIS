//! A plugin command's dialog (`commands[].form` in `plugin.json`): the command's button opens a
//! window with its fields; **Run** runs the command with their values as parameters and shows
//! what it answers (its `report` text) under them, so each step of a plugin has its own
//! parameters and result instead of sharing one panel.
//!
//! The run waits two frames, as a plugin command from the ribbon does ([`crate::pluginstart`]),
//! so the window shows that it is running before the command holds the frame.

use crate::RustGisApp;
use egui::{Align2, RichText, Ui};
use rustgis_engine::analysis::ParamKind;
use rustgis_engine::plugins::ParamDecl;
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// Frames drawn between **Run** and the run, so the window shows it running first.
const FRAMES_SHOWN: u8 = 2;

#[derive(Default)]
pub struct State {
    open: Option<Dialog>,
    /// The values each dialog had when last closed: opened again, it starts from them.
    kept: HashMap<(String, String), Map<String, Value>>,
}

struct Dialog {
    plugin: String,
    command: String,
    values: Map<String, Value>,
    /// Frames drawn since **Run**, while the run waits to start.
    running: Option<u8>,
    /// The last run's report, or its error.
    result: Option<Result<String, String>>,
}

/// `ui.pluginForm`: `{"plugin": id, "command": id}` opens that plugin command's dialog.
pub fn open(app: &mut RustGisApp, p: &Value) -> Result<Value, String> {
    let plugin = p.get("plugin").and_then(Value::as_str).ok_or("plugin is required")?;
    let command = p.get("command").and_then(Value::as_str).ok_or("command is required")?;
    let loaded = rustgis_engine::plugins::get(plugin).filter(|p| p.enabled).ok_or_else(|| format!("no plugin '{plugin}'"))?;
    let c = loaded.manifest.commands.iter().find(|c| c.id == command).ok_or_else(|| format!("{plugin} has no command '{command}'"))?;
    if c.form.is_empty() {
        return Err(format!("{plugin}/{command} has no dialog"));
    }
    let key = (plugin.to_string(), command.to_string());
    let values = app
        .ui
        .plugin_dialog
        .kept
        .get(&key)
        .cloned()
        .unwrap_or_else(|| c.form.iter().filter_map(|f| f.default.clone().map(|d| (f.name.clone(), d))).collect());
    app.ui.plugin_dialog.open = Some(Dialog { plugin: key.0, command: key.1, values, running: None, result: None });
    Ok(Value::Null)
}

/// What a command answered, as the dialog shows it: its `report` text, or the answer itself
/// (without the panel state it may carry).
fn report(answer: &Value) -> String {
    if let Some(t) = answer.get("report").and_then(Value::as_str) {
        return t.to_string();
    }
    match answer {
        Value::Null => tl!("Done.").to_string(),
        Value::Object(o) => {
            let rest: Map<String, Value> = o.iter().filter(|(k, _)| *k != "panel").map(|(k, v)| (k.clone(), v.clone())).collect();
            serde_json::to_string_pretty(&rest).unwrap_or_default()
        }
        other => other.to_string(),
    }
}

pub fn show(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some(mut d) = app.ui.plugin_dialog.open.take() else { return };
    // A plugin turned off or removed takes its dialog with it.
    let Some(loaded) = rustgis_engine::plugins::get(&d.plugin).filter(|p| p.enabled) else { return };
    let m = &loaded.manifest;
    let Some(c) = m.commands.iter().find(|c| c.id == d.command) else { return };
    let lang = app.lang().code().to_string();

    if let Some(frames) = d.running {
        if frames >= FRAMES_SHOWN {
            let id = rustgis_engine::plugins::command_id(&d.plugin, &d.command);
            d.result = Some(app.run(&id, Value::Object(d.values.clone())).map(|v| report(&v)));
            d.running = None;
        } else {
            d.running = Some(frames + 1);
            ctx.request_repaint();
        }
    }

    let mut open = true;
    let title = format!("{} › {}", m.translate(&lang, &m.name), m.translate(&lang, &c.label).replace('\n', " "));
    egui::Window::new(title)
        .id(egui::Id::unique(("plugin_form", &d.plugin, &d.command)))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(460.0)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            if !c.help.is_empty() {
                ui.label(RichText::new(m.translate(&lang, &c.help)).weak());
                ui.separator();
            }
            egui::Grid::new(("plugin_form_grid", &d.plugin, &d.command)).num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                // A field lists the fields of the layer chosen above it.
                let mut layer: Option<String> = None;
                for f in &c.form {
                    let label = ui.label(m.translate(&lang, &f.label));
                    if !f.help.is_empty() {
                        label.on_hover_text(m.translate(&lang, &f.help));
                    }
                    field(app, ui, f, &mut d.values, layer.as_deref(), m, &lang);
                    if layer_kind(&f.kind).is_some() {
                        layer = d.values.get(&f.name).and_then(Value::as_str).map(str::to_string);
                    }
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let running = d.running.is_some();
                if ui.add_enabled(!running, egui::Button::new(RichText::new(tl!("Run")).strong())).clicked() {
                    d.running = Some(0);
                    d.result = None;
                }
                if running {
                    ui.spinner();
                    ui.label(tl!("Running…"));
                }
            });
            if let Some(r) = &d.result {
                ui.separator();
                ui.label(RichText::new(tl!("Result")).strong());
                egui::ScrollArea::vertical().min_scrolled_height(260.0).max_height(420.0).auto_shrink([false, true]).show(
                    ui,
                    |ui| match r {
                        Ok(t) => {
                            ui.add(egui::Label::new(RichText::new(t).monospace()).selectable(true));
                        }
                        Err(e) => {
                            ui.colored_label(ui.visuals().error_fg_color, e);
                        }
                    },
                );
            }
        });
    if open {
        app.ui.plugin_dialog.open = Some(d);
    } else {
        app.ui.plugin_dialog.kept.insert((d.plugin, d.command), d.values);
    }
}

/// The layer kind a field kind picks, if it picks a layer.
fn layer_kind(kind: &str) -> Option<ParamKind> {
    Some(match kind {
        "layer" => ParamKind::Layer,
        "vectorLayer" => ParamKind::VectorLayer,
        "polygonLayer" => ParamKind::PolygonLayer,
        "pointLayer" => ParamKind::PointLayer,
        "rasterLayer" => ParamKind::RasterLayer,
        _ => return None,
    })
}

/// A drop-down of `(value, label)` options for `values[key]`; an unset or vanished value
/// becomes the first option.
fn combo(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    values: &mut Map<String, Value>,
    key: &str,
    options: &[(String, String)],
) {
    let cur = values.get(key).and_then(Value::as_str).map(str::to_string);
    if (cur.is_none() || !options.iter().any(|o| Some(&o.0) == cur.as_ref()))
        && let Some(first) = options.first()
    {
        values.insert(key.to_string(), json!(first.0));
    }
    let cur = values.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let shown = options.iter().find(|o| o.0 == cur).map(|o| o.1.clone()).unwrap_or_else(|| tl!("(none)").to_string());
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(280.0).show_ui(ui, |ui| {
        for (v, label) in options {
            if ui.selectable_label(*v == cur, label).clicked() {
                values.insert(key.to_string(), json!(v));
            }
        }
    });
}

fn field(
    app: &RustGisApp,
    ui: &mut Ui,
    f: &ParamDecl,
    values: &mut Map<String, Value>,
    layer: Option<&str>,
    m: &rustgis_engine::plugins::Manifest,
    lang: &str,
) {
    let id = ("plugin_form_field", f.name.as_str());
    if let Some(kind) = layer_kind(&f.kind) {
        let options: Vec<(String, String)> = app
            .session
            .map()
            .layers
            .iter()
            .filter(|l| crate::panes::takes_layer(kind, l))
            .map(|l| (l.name.clone(), l.name.clone()))
            .collect();
        combo(ui, id, values, &f.name, &options);
        return;
    }
    match f.kind.as_str() {
        "field" | "numericField" => {
            let kind = if f.kind == "field" { ParamKind::Field } else { ParamKind::NumericField };
            let options: Vec<(String, String)> = layer
                .and_then(|n| crate::panes::input_dataset(&app.session, n))
                .map(|d| {
                    (0..d.fields.len())
                        .filter(|i| crate::panes::takes_field(kind, d, *i))
                        .map(|i| (d.fields[i].name.clone(), d.fields[i].name.clone()))
                        .collect()
                })
                .unwrap_or_default();
            // The field named like the default, when the layer has one.
            if !values.contains_key(&f.name)
                && let Some(d) = f.default.as_ref().and_then(Value::as_str)
                && options.iter().any(|o| o.0 == d)
            {
                values.insert(f.name.clone(), json!(d));
            }
            combo(ui, id, values, &f.name, &options);
        }
        "number" => {
            let mut x = values.get(&f.name).and_then(Value::as_f64).unwrap_or(0.0);
            if ui.add(egui::DragValue::new(&mut x).speed(1.0)).changed() || !values.contains_key(&f.name) {
                // A whole number goes as one, for commands that read integers.
                let v = if x.fract() == 0.0 && x.abs() < 1e15 { json!(x as i64) } else { json!(x) };
                values.insert(f.name.clone(), v);
            }
        }
        "choice" => {
            let options: Vec<(String, String)> = f.choices.iter().map(|c| (c.clone(), m.translate(lang, c).to_string())).collect();
            combo(ui, id, values, &f.name, &options);
        }
        "bool" => {
            let mut on = values.get(&f.name).and_then(Value::as_bool).unwrap_or(false);
            if ui.checkbox(&mut on, "").changed() || !values.contains_key(&f.name) {
                values.insert(f.name.clone(), json!(on));
            }
        }
        // `text`.
        _ => {
            let mut s = values.get(&f.name).and_then(Value::as_str).unwrap_or_default().to_string();
            if ui.add(egui::TextEdit::singleline(&mut s).desired_width(280.0)).changed() {
                values.insert(f.name.clone(), json!(s));
            }
        }
    }
}
