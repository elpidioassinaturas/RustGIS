//! Edit tab: create and delete features, edit attributes and fields.

use rustgis_data::{FieldType, Geometry, GeometryType, Value};
use serde_json::{Value as Json, json};

use super::{need_layer, ps};
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("edit.calculateGeometry", "Calculate Geometry", "Attribute Table", crate::geometry_calc::run)
            .when(super::design::need_editable)
            .params(r#"{"layer"?, "field": string, "create": bool, "overwrite": bool, "property": string, "unit": string, "epsg"?: number, "scope": "all"|"selected"}"#),
        C::new("edit.addFeature", "Create Feature", "Edit › Features", add_feature)
            .when(super::design::need_editable)
            .params(r#"{"layer"?, "geometry": GeoJSON geometry (lon/lat), "attributes"?: {field: value}, "template"?: feature template}"#),
        C::new("edit.delete", "Delete", "Edit › Features", delete).key("Delete").when(super::design::need_editable_selection),
        C::new("edit.setValue", "Edit Attribute", "Attribute Table", set_value)
            .when(super::design::need_editable)
            .params(r#"{"layer"?, "row": number, "field": string, "value": any}"#),
        C::new("edit.addField", "Add Field", "Data › Data Design › Fields", add_field)
            .when(need_layer)
            .params(r#"{"layer"?, "name": string, "type": "long"|"double"|"text"|"boolean"}"#),
        C::new("edit.deleteField", "Delete Field", "Data › Data Design › Fields", delete_field)
            .when(need_layer)
            .params(r#"{"layer"?, "field": string}"#),
        C::new("edit.calculateField", "Calculate Field", "Attribute Table", calculate_field)
            .when(super::design::need_editable)
            .params(r#"{"layer"?, "field": string, "expression": "number, 'text', or field op field/number (+ - * /)"}"#),
    ]
}

pub(crate) fn add_feature(s: &mut Session, p: &Json) -> CmdResult {
    let id = s.layer_id(p)?;
    let g = p.get("geometry").and_then(rustgis_data::io::geometry_from_geojson).ok_or("geometry must be a GeoJSON geometry")?;
    let l = s.map_mut().layer_mut(id).ok_or("no layer")?;
    let want = l.geometry_type();
    let ok = want == g.kind() || want == GeometryType::None;
    if !ok {
        return Err(format!("{} can't go in a {} layer", g.kind().label(), want.label()));
    }
    validate(&g)?;
    // A feature template's defaults, then the given attributes.
    let template = match ps(p, "template") {
        Some(t) => {
            Some(l.design.templates.iter().find(|x| x.name.eq_ignore_ascii_case(t)).cloned().ok_or_else(|| format!("no template '{t}'"))?)
        }
        None => None,
    };
    let attrs: Vec<Value> = l
        .dataset
        .fields
        .iter()
        .map(|f| {
            let given = p.get("attributes").and_then(|a| a.get(&f.name)).map(|v| coerce(Value::from_json(v), f.ty));
            let default = template
                .as_ref()
                .and_then(|t| t.defaults.iter().find(|(k, _)| k.eq_ignore_ascii_case(&f.name)))
                .map(|(_, v)| coerce(v.clone(), f.ty));
            given.or(default).unwrap_or(Value::Null)
        })
        .collect();
    l.dataset.push(Some(g), attrs);
    l.touch_geometry();
    let row = l.dataset.features.len() - 1;
    l.selection = vec![row];
    super::design::apply(s, id, vec![super::design::Change { before: None, row: Some(row) }])?;
    Ok(json!({"row": row}))
}

/// Rejects empty or degenerate geometry, which would break extents, labels and rendering.
pub(crate) fn validate(g: &Geometry) -> Result<(), String> {
    match g {
        Geometry::Point(_) => Ok(()),
        Geometry::MultiPoint(v) if v.is_empty() => Err("a multipoint needs at least one point".into()),
        Geometry::MultiPoint(_) => Ok(()),
        Geometry::Line(parts) if parts.is_empty() || parts.iter().any(|p| p.len() < 2) => Err("a line needs at least two points".into()),
        Geometry::Line(_) => Ok(()),
        Geometry::Polygon(polys) if polys.is_empty() || polys.iter().any(|rings| rings.is_empty() || rings.iter().any(|r| r.len() < 4)) => {
            Err("a polygon needs at least three points".into())
        }
        Geometry::Polygon(_) => Ok(()),
    }
}

pub(crate) fn coerce(v: Value, ty: FieldType) -> Value {
    match v {
        Value::Null => Value::Null,
        Value::Text(t) => Value::parse(&t, ty),
        other => Value::parse(&other.to_string(), ty),
    }
}

fn delete(s: &mut Session, _: &Json) -> CmdResult {
    let mut n = 0;
    let mut removed: Vec<(u32, Vec<super::design::Change>)> = Vec::new();
    for l in &mut s.map_mut().layers {
        if l.selection.is_empty() {
            continue;
        }
        let mut sel = std::mem::take(&mut l.selection);
        sel.sort_unstable();
        sel.dedup();
        let mut changes = Vec::new();
        for r in sel.into_iter().rev() {
            if r < l.dataset.features.len() {
                changes.push(super::design::Change { before: Some(l.dataset.features.remove(r)), row: None });
                n += 1;
            }
        }
        l.touch_geometry();
        removed.push((l.id, changes));
    }
    for (id, changes) in removed {
        super::design::apply(s, id, changes)?;
    }
    s.notify(format!("Deleted {n} features"), false);
    Ok(json!({"deleted": n}))
}

fn field_index(s: &Session, id: u32, name: &str) -> Result<usize, String> {
    s.map().layer(id).and_then(|l| l.dataset.field_index(name)).ok_or_else(|| format!("no field named '{name}'"))
}

fn set_value(s: &mut Session, p: &Json) -> CmdResult {
    let id = s.layer_id(p)?;
    let row = p.get("row").and_then(Json::as_u64).ok_or("row is required")? as usize;
    let fi = field_index(s, id, ps(p, "field").ok_or("field is required")?)?;
    let l = s.map_mut().layer_mut(id).ok_or("no layer")?;
    let ty = l.dataset.fields.get(fi).map(|f| f.ty).ok_or("no field")?;
    let raw = p.get("value").cloned().unwrap_or(Json::Null);
    let v = match raw {
        Json::String(t) => Value::parse(&t, ty),
        other => coerce(Value::from_json(&other), ty),
    };
    let f = l.dataset.features.get_mut(row).ok_or("no such row")?;
    let before = f.clone();
    let cell = f.attrs.get_mut(fi).ok_or("no such row")?;
    *cell = v;
    super::design::apply(s, id, vec![super::design::Change { before: Some(before), row: Some(row) }])?;
    Ok(Json::Null)
}

fn add_field(s: &mut Session, p: &Json) -> CmdResult {
    let id = s.layer_id(p)?;
    let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or("name is required")?;
    if !name.chars().all(|c| c.is_alphanumeric() || c == '_') || name.chars().count() > 64 {
        return Err("field names use letters, digits and _ (at most 64)".into());
    }
    let ty = match ps(p, "type").unwrap_or("double").to_ascii_lowercase().as_str() {
        "long" | "integer" | "int" | "short" => FieldType::Integer,
        "double" | "float" | "number" => FieldType::Double,
        "text" | "string" => FieldType::Text,
        "boolean" | "bool" => FieldType::Boolean,
        other => return Err(format!("unknown field type '{other}'")),
    };
    let l = s.map_mut().layer_mut(id).ok_or("no layer")?;
    if l.dataset.field_index(name).is_some() {
        return Err(format!("a field named '{name}' already exists"));
    }
    l.dataset.ensure_field(name, ty);
    Ok(Json::Null)
}

fn delete_field(s: &mut Session, p: &Json) -> CmdResult {
    let id = s.layer_id(p)?;
    let fi = field_index(s, id, ps(p, "field").ok_or("field is required")?)?;
    let l = s.map_mut().layer_mut(id).ok_or("no layer")?;
    let removed = l.dataset.fields.remove(fi).name;
    for f in &mut l.dataset.features {
        if fi < f.attrs.len() {
            f.attrs.remove(fi);
        }
    }
    // Nothing may keep referring to the deleted field.
    if l.renderer.field().is_some_and(|f| f.eq_ignore_ascii_case(&removed)) {
        l.renderer = rustgis_map::Renderer::Single(l.renderer.base_symbol());
    }
    // The layer's labels turn off if the Default class was on it, which starts again from the
    // layer's first name-like field (keeping its name, clause and visibility). Another class on it
    // labels with Default's field instead. A class clause naming it no longer reads, and takes no
    // features, which go on to the next class: it is kept to edit, not dropped with its text symbol.
    if l.labels.default.field.eq_ignore_ascii_case(&removed) {
        l.labels.restart_default(&l.dataset);
    }
    l.labels.repoint_missing_fields(&l.dataset);
    if let Some(q) = &l.definition_query
        && rustgis_data::query::parse(q, &l.dataset).is_err()
    {
        l.definition_query = None;
    }
    // Nor its data design: domains, subtypes, contingent values, and rules that used it.
    let d = &mut l.design;
    d.domains.retain(|x| !x.field.eq_ignore_ascii_case(&removed));
    if d.subtypes.as_ref().is_some_and(|st| st.field.eq_ignore_ascii_case(&removed)) {
        d.subtypes = None;
    }
    if let Some(st) = &mut d.subtypes {
        for sub in &mut st.subtypes {
            sub.defaults.retain(|(f, _)| !f.eq_ignore_ascii_case(&removed));
        }
    }
    d.contingent.retain(|c| !c.fields.iter().any(|f| f.eq_ignore_ascii_case(&removed)));
    let ds = &l.dataset;
    let before = d.rules.len();
    d.rules.retain(|r| {
        !r.field.eq_ignore_ascii_case(&removed)
            && match r.kind {
                rustgis_map::design::RuleKind::Calculation => Calc::parse(&r.expression, ds).is_ok(),
                rustgis_map::design::RuleKind::Constraint => rustgis_data::query::parse(&r.expression, ds).is_ok(),
            }
    });
    Ok(json!({"rulesRemoved": before - d.rules.len()}))
}

/// A tiny calculator: `a`, `a op b` where a/b are numbers, 'text' or field names.
fn calculate_field(s: &mut Session, p: &Json) -> CmdResult {
    let id = s.layer_id(p)?;
    let fi = field_index(s, id, ps(p, "field").ok_or("field is required")?)?;
    let expr = ps(p, "expression").map(str::trim).filter(|e| !e.is_empty()).ok_or("expression is required")?.to_string();
    let l = s.map_mut().layer_mut(id).ok_or("no layer")?;
    let ty = l.dataset.fields.get(fi).map(|f| f.ty).ok_or("no field")?;
    let calc = Calc::parse(&expr, &l.dataset)?;
    let rows: Vec<usize> = if l.selection.is_empty() { (0..l.dataset.features.len()).collect() } else { l.selection.clone() };
    let mut out = Vec::new();
    for r in &rows {
        out.push((*r, coerce(calc.eval(&l.dataset, *r), ty)));
    }
    let mut changes = Vec::new();
    for (r, v) in out {
        if let Some(f) = l.dataset.features.get_mut(r) {
            let before = f.clone();
            if let Some(c) = f.attrs.get_mut(fi) {
                *c = v;
            }
            changes.push(super::design::Change { before: Some(before), row: Some(r) });
        }
    }
    super::design::apply(s, id, changes)?;
    Ok(json!({"rows": rows.len()}))
}

enum Term {
    Num(f64),
    Text(String),
    Field(usize),
}

/// A parsed calculator expression: `a` or `a op b`.
pub(crate) struct Calc {
    a: Term,
    op: Option<char>,
    b: Option<Term>,
}

impl Calc {
    pub(crate) fn parse(expr: &str, ds: &rustgis_data::Dataset) -> Result<Calc, String> {
        let (lhs, op, rhs) = split_op(expr.trim());
        let term = |t: &str| -> Result<Term, String> {
            let t = t.trim();
            if let Some(q) = t.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')) {
                return Ok(Term::Text(q.to_string()));
            }
            if let Ok(n) = t.parse::<f64>() {
                return Ok(Term::Num(n));
            }
            let name = t.trim_matches('!').trim_matches('"');
            ds.field_index(name).map(Term::Field).ok_or_else(|| format!("no field named '{name}'"))
        };
        Ok(Calc { a: term(lhs)?, op, b: rhs.map(term).transpose()? })
    }

    pub(crate) fn eval(&self, ds: &rustgis_data::Dataset, r: usize) -> Value {
        let get = |t: &Term| -> Value {
            match t {
                Term::Num(n) => Value::Float(*n),
                Term::Text(s) => Value::Text(s.clone()),
                Term::Field(i) => ds.value(r, *i).clone(),
            }
        };
        let va = get(&self.a);
        match (&self.b, self.op) {
            (Some(b), Some(op)) => {
                let vb = get(b);
                match (va.as_f64(), vb.as_f64(), op) {
                    (Some(x), Some(y), '+') => Value::Float(x + y),
                    (Some(x), Some(y), '-') => Value::Float(x - y),
                    (Some(x), Some(y), '*') => Value::Float(x * y),
                    (Some(x), Some(y), '/') if y != 0.0 => Value::Float(x / y),
                    (_, _, '+') if !va.is_null() && !vb.is_null() => Value::Text(format!("{va}{vb}")),
                    _ => Value::Null,
                }
            }
            _ => va,
        }
    }
}

/// Splits on the first top-level arithmetic operator outside quotes (not a leading minus).
fn split_op(e: &str) -> (&str, Option<char>, Option<&str>) {
    let mut q = false;
    for (i, c) in e.char_indices() {
        if c == '\'' {
            q = !q;
        }
        if !q && i > 0 && matches!(c, '+' | '-' | '*' | '/') {
            let before = e.get(..i).unwrap_or("").trim_end();
            if before.ends_with(['e', 'E']) && before.chars().rev().nth(1).is_some_and(|d| d.is_ascii_digit()) {
                continue;
            }
            return (e.get(..i).unwrap_or(""), Some(c), e.get(i + c.len_utf8()..));
        }
    }
    (e, None, None)
}
