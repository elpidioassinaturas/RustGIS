//! View › Labeling (lock, pause, unplaced labels, convert to annotation), the Style tab's label
//! visibility range and placement, and a layer's label classes (#618).

use rustgis_data::{Dataset, Field, FieldType, Geometry, GeometryType, Value as Attr};
use rustgis_map::symbology::{MAX_LABEL_CLASSES, label_class_name};
use rustgis_map::{LabelClass, LabelMatcher, LabelPlacement, Labeling, Renderer, Rgba, Symbol};
use serde_json::{Value, json};

use super::layer::s_req;
use super::{b, f, need_feature_layer, ps};
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("labels.lock", "Lock", "View › Labeling", |s, p| {
            let d = &mut s.map_mut().label_display;
            d.locked = b(p, "on").unwrap_or(!d.locked);
            Ok(json!({"locked": d.locked}))
        })
        .view()
        .params(r#"{"on"?: bool}"#),
        C::new("labels.pause", "Pause", "View › Labeling", |s, p| {
            let d = &mut s.map_mut().label_display;
            d.paused = b(p, "on").unwrap_or(!d.paused);
            Ok(json!({"paused": d.paused}))
        })
        .view()
        .params(r#"{"on"?: bool}"#),
        C::new("labels.viewUnplaced", "View Unplaced", "View › Labeling", |s, p| {
            let d = &mut s.map_mut().label_display;
            d.show_unplaced = b(p, "on").unwrap_or(!d.show_unplaced);
            Ok(json!({"showUnplaced": d.show_unplaced}))
        })
        .view()
        .params(r#"{"on"?: bool}"#),
        C::new("labels.convert", "Convert To Annotation", "View › Labeling", convert)
            .when(need_feature_layer)
            .params(r#"{"layer"?, "name"?: string}"#),
        C::new("labels.scaleRange", "Label Visibility Range", "Style › Label Range", scale_range)
            .when(need_feature_layer)
            .params(r#"{"layer"?, "class"?: name (none: the Default class, by whatever name it has), "min"?: scale|null (out beyond), "max"?: scale|null (in beyond), "clear"?: bool (clears the scale and distance limits, as Clear Limits does)}"#),
        C::new("labels.distanceRange", "Label Distance Range", "Style › Label Range", distance_range)
            .when(need_feature_layer)
            .params(r#"{"layer"?, "class"?: name (none: the Default class, by whatever name it has), "closest"?: metres|null, "farthest"?: metres|null, "clear"?: bool}"#),
        C::new("labels.placement", "Label Placement", "Style › Label Placement", |s, p| {
            let code = ps(p, "position").ok_or("position is required")?;
            let pos = LabelPlacement::from_code(code).ok_or_else(|| format!("unknown placement '{code}'"))?;
            with_class(s, p, |c| c.placement = pos)
        })
        .when(need_feature_layer)
        .params(
            r#"{"layer"?, "class"?: name (none: the Default class, by whatever name it has), "position": "auto"|"center"|"above"|"below"|"left"|"right"|"aboveRight"|"aboveLeft"|"belowRight"|"belowLeft"}"#,
        ),
        // Label classes: a feature is labelled by the first class whose clause takes it, Default
        // last (`Labeling`).
        C::new("labels.class.add", "Add Label Class", "Style › Label Class", add_class)
            .when(need_feature_layer)
            .params(r#"{"layer"?, "name": string, "where"?: sql (a definition query's language; none takes every feature that reaches it), "from"?: class (whose field and text symbol it starts with; none: the Default class)}"#),
        C::new("labels.class.remove", "Remove Label Class", "Style › Label Class", |s, p| {
            let name = s_req(p, "class")?.trim().to_string();
            with_labels(s, p, |labels| {
                let i = extra_class(labels, &name)?;
                labels.classes.remove(i);
                Ok(())
            })
        })
        .when(need_feature_layer)
        .params(r#"{"layer"?, "class": name (not Default)}"#),
        // Every class but Default; none to remove is no error, so a script can start with it.
        C::new("labels.class.clear", "Remove All Label Classes", "Style › Label Class", |s, p| {
            let mut removed = 0;
            with_labels(s, p, |labels| {
                removed = labels.classes.len();
                labels.classes.clear();
                Ok(())
            })?;
            Ok(json!({"removed": removed}))
        })
        .when(need_feature_layer)
        .params(r#"{"layer"?}"#),
        C::new("labels.class.rename", "Rename Label Class", "Style › Label Class", |s, p| {
            let class = s_req(p, "class")?.trim().to_string();
            let name = s_req(p, "name")?.to_string();
            with_labels(s, p, |labels| {
                let name = class_name(labels, &name, Some(&class))?;
                labels.class_mut(&class).ok_or_else(|| format!("no label class named '{class}'"))?.name = name;
                Ok(())
            })
        })
        .when(need_feature_layer)
        .params(r#"{"layer"?, "class": name, "name": string}"#),
        C::new("labels.class.update", "Update Label Class", "Style › Label Class", |s, p| {
            let id = s.layer_id(p)?;
            let clause = s.map().layer(id).map(|l| clause(p, &l.dataset)).transpose()?.flatten();
            let visible = b(p, "visible");
            with_class(s, p, |c| {
                if let Some(w) = clause {
                    c.where_ = w;
                }
                if let Some(v) = visible {
                    c.visible = v;
                }
            })
        })
        .when(need_feature_layer)
        .params(r#"{"layer"?, "class"?: name (none: the Default class, by whatever name it has), "where"?: sql|null, "visible"?: bool}"#),
        C::new("labels.class.move", "Move Label Class", "Style › Label Class", |s, p| {
            let name = s_req(p, "class")?.trim().to_string();
            let to = f(p, "to").filter(|v| v.is_finite()).ok_or("to is required: the class's place, 0 checked first")?;
            with_labels(s, p, |labels| {
                let i = extra_class(labels, &name)?;
                let c = labels.classes.remove(i);
                let to = (to.max(0.0) as usize).min(labels.classes.len());
                labels.classes.insert(to, c);
                Ok(())
            })
        })
        .when(need_feature_layer)
        .params(r#"{"layer"?, "class": name (not Default), "to": number (its place among the classes checked before Default, 0 first)}"#),
    ]
}

/// Changes the label class `p` names (`"class"`; Default when none) of the layer it names.
pub(crate) fn with_class(s: &mut Session, p: &Value, f: impl FnOnce(&mut LabelClass)) -> CmdResult {
    with_labels(s, p, |labels| {
        f(class_in(labels, p)?);
        Ok(())
    })
}

/// The label class `p` names (`"class"`; Default when none).
pub(crate) fn class_in<'a>(labels: &'a mut Labeling, p: &Value) -> Result<&'a mut LabelClass, String> {
    match class_param(p, "class")? {
        None => Ok(&mut labels.default),
        Some(n) => labels.class_mut(n).ok_or_else(|| format!("no label class named '{n}'")),
    }
}

/// The class name `p` gives as `key`: none when it's missing or null (the Default class), and a
/// refusal when it isn't a name, rather than changing Default by mistake.
fn class_param<'a>(p: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            v.as_str().map(str::trim).filter(|n| !n.is_empty()).map(Some).ok_or_else(|| format!("{key} must be a label class's name"))
        }
    }
}

/// Changes the labels of the layer `p` names; nothing changes when `f` refuses.
pub(crate) fn with_labels(s: &mut Session, p: &Value, f: impl FnOnce(&mut Labeling) -> Result<(), String>) -> CmdResult {
    let id = s.layer_id(p)?;
    let l = s.map_mut().layer_mut(id).ok_or("no such layer")?;
    let mut labels = l.labels.clone();
    f(&mut labels)?;
    l.labels = labels;
    Ok(Value::Null)
}

/// The place of the class named `name` among those checked before Default (of the
/// `MAX_LABEL_CLASSES` that count, as `Labeling::class` finds them).
fn extra_class(labels: &Labeling, name: &str) -> Result<usize, String> {
    if labels.default.name.eq_ignore_ascii_case(name) {
        return Err(format!("{name} is the Default class: it labels the rest of the features, and stays last"));
    }
    let mut counted = labels.classes.iter().take(MAX_LABEL_CLASSES);
    counted.position(|c| c.name.eq_ignore_ascii_case(name)).ok_or_else(|| format!("no label class named '{name}'"))
}

/// A class name: trimmed, kept short, and not another class's (`renaming` is the class being
/// renamed, which may keep its own).
fn class_name(labels: &Labeling, name: &str, renaming: Option<&str>) -> Result<String, String> {
    let name = label_class_name(name);
    if name.is_empty() {
        return Err("a label class needs a name".into());
    }
    let own = |c: &LabelClass| renaming.is_some_and(|r| c.name.eq_ignore_ascii_case(r.trim()));
    if labels.in_order().any(|c| c.name.eq_ignore_ascii_case(&name) && !own(c)) {
        return Err(format!("there is already a label class named '{name}'"));
    }
    Ok(name)
}

/// A class clause parameter (`"where"`): `None` leaves it, `Some(None)` clears it, and a clause is
/// kept only if it reads against `ds`.
fn clause(p: &Value, ds: &Dataset) -> Result<Option<Option<String>>, String> {
    match p.get("where") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(w)) if w.trim().is_empty() => Ok(Some(None)),
        Some(Value::String(w)) => {
            rustgis_data::query::parse(w, ds).map_err(|e| format!("the class's clause doesn't read: {e}"))?;
            Ok(Some(Some(w.trim().to_string())))
        }
        Some(_) => Err("where must be a SQL clause".into()),
    }
}

fn add_class(s: &mut Session, p: &Value) -> CmdResult {
    let name = s_req(p, "name")?.to_string();
    let from = class_param(p, "from")?.map(String::from);
    let id = s.layer_id(p)?;
    let clause = s.map().layer(id).map(|l| clause(p, &l.dataset)).transpose()?.flatten().flatten();
    let mut added = String::new();
    with_labels(s, p, |labels| {
        if labels.classes.len() >= MAX_LABEL_CLASSES {
            return Err(format!("a layer keeps at most {MAX_LABEL_CLASSES} label classes besides Default"));
        }
        let name = class_name(labels, &name, None)?;
        let base = match &from {
            None => &labels.default,
            Some(n) => labels.class(n).ok_or_else(|| format!("no label class named '{n}'"))?,
        };
        let class = LabelClass { name: name.clone(), visible: true, where_: clause, ..base.clone() };
        labels.classes.push(class);
        added = name;
        Ok(())
    })?;
    Ok(json!({"class": added}))
}

/// A limit parameter: `Some(None)` clears it, `Some(Some(v))` sets it, `None` leaves it.
fn limit(p: &Value, k: &str) -> Result<Option<Option<f64>>, String> {
    match p.get(k) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(t)) if t.trim().is_empty() => Ok(Some(None)),
        Some(_) => match f(p, k) {
            Some(v) if v > 0.0 => Ok(Some(Some(v.min(1e12)))),
            Some(v) if v.abs() < f64::EPSILON => Ok(Some(None)),
            _ => Err(format!("{k} must be a positive number")),
        },
    }
}

fn scale_range(s: &mut Session, p: &Value) -> CmdResult {
    let clear = b(p, "clear").unwrap_or(false);
    let (min, max) = (limit(p, "min")?, limit(p, "max")?);
    with_class(s, p, |c| {
        if clear {
            c.min_scale = None;
            c.max_scale = None;
            c.closest = None;
            c.farthest = None;
        }
        if let Some(v) = min {
            c.min_scale = v;
        }
        if let Some(v) = max {
            c.max_scale = v;
        }
    })
}

fn distance_range(s: &mut Session, p: &Value) -> CmdResult {
    let clear = b(p, "clear").unwrap_or(false);
    let (closest, farthest) = (limit(p, "closest")?, limit(p, "farthest")?);
    with_class(s, p, |c| {
        if clear {
            c.closest = None;
            c.farthest = None;
        }
        if let Some(v) = closest {
            c.closest = v;
        }
        if let Some(v) = farthest {
            c.farthest = v;
        }
    })
}

/// Converts a layer's labels into an annotation layer: one point per label, at the label point,
/// carrying the label text and drawn with the same text symbol. Each feature takes the text of
/// the class labelling it (a hidden class's take none); with classes besides Default, a CLASS
/// field keeps which, and the annotation has the same classes, picking their points by it. The
/// layer's own labels turn off.
fn convert(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let l = s.map().layer(id).ok_or("no such layer")?;
    let fields: Vec<Option<usize>> = l.labels.in_order().map(|c| l.dataset.field_index(&c.field)).collect();
    if fields.iter().all(Option::is_none) {
        return Err("the layer has no label field".into());
    }
    let classes = !l.labels.classes.is_empty();
    let rows = l.visible_rows();
    let matcher = LabelMatcher::new(&l.labels, &l.dataset);
    let mut out_fields = vec![Field::new("TEXT", FieldType::Text)];
    if classes {
        out_fields.push(Field::new("CLASS", FieldType::Text));
    }
    let mut ds = Dataset::new(
        ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(String::from).unwrap_or_else(|| format!("{} Annotation", l.name)),
        GeometryType::Point,
        out_fields,
    );
    for (row, ft) in l.dataset.features.iter().enumerate() {
        if rows.get(row) != Some(&true) {
            continue;
        }
        let Some((i, class)) = matcher.class_of(&l.dataset, row).filter(|(_, c)| c.visible) else { continue };
        let Some(fi) = fields.get(i).copied().flatten() else { continue };
        let v = l.dataset.value(row, fi);
        if v.is_null() {
            continue;
        }
        if let Some(c) = ft.geometry.as_ref().and_then(Geometry::label_point) {
            let mut attrs = vec![Attr::Text(v.to_string())];
            if classes {
                attrs.push(Attr::Text(class.name.clone()));
            }
            ds.push(Some(Geometry::Point(c)), attrs);
        }
    }
    if ds.features.is_empty() {
        return Err("the layer has no labels to convert".into());
    }
    let n = ds.features.len();
    let mut labels = l.labels.clone();
    labels.enabled = true;
    // The classes that count, as for the layer's own labels.
    labels.classes.truncate(MAX_LABEL_CLASSES);
    for c in std::iter::once(&mut labels.default).chain(labels.classes.iter_mut()) {
        c.field = "TEXT".into();
        c.placement = LabelPlacement::Center;
    }
    // Each class picks the points it labelled; Default, checked last, takes the rest.
    for c in &mut labels.classes {
        c.where_ = Some(format!("\"CLASS\" = '{}'", c.name.replace('\'', "''")));
    }
    labels.default.where_ = None;
    let (min_scale, max_scale) = (l.min_scale, l.max_scale);
    if let Some(src) = s.map_mut().layer_mut(id) {
        src.labels.enabled = false;
    }
    let new_id = super::layer::add_dataset(s, ds);
    if let Some(a) = s.map_mut().layer_mut(new_id) {
        // The annotation is all text: the points themselves are invisible.
        let clear = Rgba::new(0, 0, 0, 0);
        a.renderer = Renderer::Single(Symbol { fill: clear, stroke: clear, stroke_width: 0.0, size: 0.0, marker: Default::default() });
        a.labels = labels;
        a.min_scale = min_scale;
        a.max_scale = max_scale;
    }
    Ok(json!({"layer": new_id, "annotations": n}))
}
