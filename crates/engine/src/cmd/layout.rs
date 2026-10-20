//! Home › Project (New Layout, Import Layout), Publish › Map Frames, Map Surrounds and Text, and
//! Publish › Output (Export Layout) and Save As (Layout File): page layouts.

use std::path::Path;

use rustgis_geom::{Coord, Extent};
use rustgis_map::Rgba;
use rustgis_map::layout::{
    ArrowStyle, CoordFormat, ElementKind, FrameShape, Grid, GridEdges, GridStyle, Guides, Layout, LineDir, LineHead, MAX_COLORBAR_TICKS,
    MAX_GRID_WIDTH, MAX_GUIDES, MAX_SCALE_DIVISIONS, MAX_TICK_MM, MIN_ELEMENT_MM, MapSeries, Orientation, PAGES, ScaleBarStyle, ScaleUnits,
    TextAlign, TickSide, fit_scale, line_angle, line_at_angle, line_length,
};
use serde_json::{Value, json};

use super::{b, f, numbered, ps};
use crate::{CmdResult, CommandSpec as C, Session};

/// Most compressed page data a PDF export holds (less in the browser).
const PDF_BUDGET: usize = if cfg!(target_arch = "wasm32") { 256 << 20 } else { 1 << 30 };

/// Layout files: JSON with this extension.
pub const LAYOUT_EXTENSION: &str = "rgislayout";
/// Largest layout file read (64 MiB, pictures included).
const MAX_LAYOUT: u64 = 64 << 20;

pub fn commands() -> Vec<C> {
    vec![
        C::new("layout.new", "New Layout", "Home › Project", new_layout).params(
            r#"{"name"?: string, "page"?: "letter"|"legal"|"tabloid"|"a5"|"a4"|"a3"|"a2"|"a1"|"a0", "orientation"?: "landscape"|"portrait", "width"?: mm, "height"?: mm, "template"?: "map" (frame, title, legend, north arrow, scale bar; the default)|"blank"}"#,
        ),
        C::new("layout.activate", "Open Layout", "Browser", |s, p| {
            let i = index(s, &by_name(p))?;
            s.active_layout = Some(i);
            Ok(json!({"layout": i}))
        })
        .view()
        .params(r#"{"name": string | "layout": name|index}"#),
        C::new("layout.close", "Show Map", "Browser", |s, _| {
            s.active_layout = None;
            Ok(Value::Null)
        })
        .view(),
        C::new("layout.remove", "Delete Layout", "Browser", |s, p| {
            let i = index(s, &by_name(p))?;
            s.project.layouts.remove(i);
            // Deleting another layout keeps the one open (a layout tab's close button).
            s.active_layout = match s.active_layout {
                Some(a) if a > i => Some(a - 1),
                Some(a) if a < i => Some(a),
                _ => None,
            };
            Ok(Value::Null)
        })
        .params(r#"{"name"?: string, "layout"?: name|index (instead of name; default: the open layout)}"#),
        C::new("layout.rename", "Rename Layout", "Browser", |s, p| {
            let name = ps(p, "newName").map(str::trim).filter(|n| !n.is_empty()).ok_or("newName is required")?;
            let i = index(s, &by_name(p))?;
            // Unique among the other layouts, so names keep finding one layout.
            let unique = numbered(name, |n| s.project.layouts.iter().enumerate().any(|(j, l)| j != i && l.name.eq_ignore_ascii_case(n)));
            if let Some(l) = s.project.layouts.get_mut(i) {
                l.name = unique.clone();
            }
            Ok(json!({"name": unique}))
        })
        .params(r#"{"name"?: string, "layout"?: name|index (instead of name; default: the open layout), "newName": string}"#),
        // A copy of a layout with all its elements (#288), right after it and open. Element ids
        // are per layout, so legends and scale bars stay on their frames, which show the same maps.
        C::new("layout.duplicate", "Duplicate Layout", "Browser", |s, p| {
            let i = index(s, &by_name(p))?;
            let mut l = s.project.layouts.get(i).ok_or("open a layout first")?.clone();
            l.name = super::copy_name(ps(p, "newName"), &l.name, |n| s.project.layouts.iter().any(|x| x.name.eq_ignore_ascii_case(n)));
            let name = l.name.clone();
            s.project.layouts.insert(i + 1, l);
            s.active_layout = Some(i + 1);
            Ok(json!({"name": name, "layout": i + 1}))
        })
        .params(r#"{"name"?: string, "layout"?: name|index (instead of name; default: the open layout), "newName"?: string (default: "<layout> copy")}"#),
        C::new("layout.page", "Page Size", "Publish › Page Setup", page).params(
            r#"{"layout"?, "page"?: page id, "orientation"?: "landscape"|"portrait", "width"?: mm, "height"?: mm}"#,
        ),
        C::new("layout.addFrame", "Map Frame", "Publish › Map Frames", add_frame).params(
            r#"{"layout"?, "shape"?: "rectangle"|"circle"|"ellipse"|"polygon"|"lasso", "points"?: [[u, v], …] (polygon/lasso, 0..1 across the frame), "map"?: name, "x"?, "y"?, "w"?, "h"?: mm}"#,
        ),
        C::new("layout.addElement", "Add Element", "Publish › Map Surrounds", add_element).params(
            r##"{"layout"?, "kind": "northArrow"|"scaleBar"|"legend"|"colorbar"|"text"|"dynamicText"|"picture"|"table"|"chart"|"rectangle" (a neatline framing the page unless any of x, y, w, h is given)|"ellipse"|"circle"|"line"|"arrow"|"extentIndicator"|"grid", "frame"?: element id (default: the first map frame), "of"?: frame id (extentIndicator, which also takes "color" and "width" in points, default red and 2), "text"?, "size"?, "color"?: "#RRGGBB"|"#RRGGBBAA", "bold"?: bool (text), "italic"?: bool, "align"?: "left"|"center"|"right", "letterSpacing"?: points, -5 to 50 (text, dynamicText), "title"?: legend title (default "Legend"), "allClasses"?: bool (legend: every class, not only those the frame draws), "hideOther"?: bool (legend: leave out <all other values>), "exclude"?: [layer names] (legend: layers it leaves out though the map draws them), "orientation"?: "horizontal"|"vertical" (colorbar), "ticks"?: 2..21 (colorbar: labelled values, ends included), "tickSide"?: "below"|"above"|"right"|"left" (colorbar), "reverse"?: bool (colorbar: high values first), "fill"?: "#RRGGBBAA" (rectangle, ellipse, circle, northArrow), "width"?: outline width in points (rectangle, ellipse, circle; 0 for none) or line width in points (line, grid), "dir"?: "horizontal"|"vertical"|"falling"|"rising" (line), "head"?: "none"|"end"|"start"|"both" (line; default "end" for an arrow), "angle"?: degrees anticlockwise from pointing right, centred on the page unless x or y is given (line, arrow; it sets the box and the direction, so w, h and dir are not used with it), "length"?: mm (line, arrow), "image"?: base64, "path"?: picture file, "layer"?: (table, chart; colorbar: a raster layer, default the top visible one), "field"?, "label"?, "rows"?, "step"?: degrees (0 picks one), "labels"?: bool, "style"?: "lines"|"ticks"|"linesAndTicks" (grid) or "classic"|"compass"|"triangle"|"star" (northArrow) or "alternating"|"stepped"|"line" (scaleBar), "tickLength"?: mm, "format"?: "decimal"|"dms", "hemisphere"?: bool (N/S/E/W rather than a sign), "edges"?: ["top","bottom","left","right"] (labelled edges), "labelOrientation"?: "horizontal"|"vertical", "off"?: bool (grid; options not given keep the grid's), "outline"?: "#RRGGBB" (northArrow), "units"?: "auto"|"kilometers"|"meters"|"miles"|"feet", "divisions"?: 1..10 (scaleBar), "x"?, "y"?, "w"?, "h"?: mm}"##,
        ),
        C::new("layout.update", "Element Properties", "Publish › Element", update).params(
            r##"{"layout"?, "element": id, "x"?, "y"?, "w"?, "h"?: mm, "name"?, "visible"?, "locked"?: bool (a locked element takes no other change until unlocked; "locked": false unlocks it and may come with changes), "text"?, "size"?, "color"? (text, line), "bold"?, "italic"?, "align"?, "letterSpacing"? (text), "title"?, "allClasses"?: bool (legend), "hideOther"?: bool (legend), "exclude"?: [layer names] (legend; [] lists every layer again), "orientation"?, "ticks"?, "tickSide"?, "reverse"?: (colorbar), "style"?: (northArrow, scaleBar), "fill"?, "outline"? (northArrow, rectangle, ellipse; for a rectangle or ellipse, color sets it when outline is not given), "width"?: points (rectangle, ellipse, line), "dir"?, "head"?, "angle"?, "length"? (line: turned or sized about its centre, then moved by any x or y; dir does not come with angle or length), "units"?, "divisions"? (scaleBar), "map"?, "scale"?, "center"?: [lon, lat], "bearing"?: degrees clockwise from north, "pitch"?: degrees of tilt (0 to 85), "grid"?: {grid options as in layout.addElement} | false (map frame), "extentIndicator"?: {"color"?, "width"?: points} | false (map frame: its extent indicator's style, or removes it), "layer"?: (colorbar: "" for the top visible raster), "field"?, "rows"?}"##,
        ),
        C::new("layout.guides", "Guides", "Publish › Page Setup", guides).params(
            r#"{"layout"?, "x"?: [mm] (vertical guides, from the page's left edge), "y"?: [mm] (horizontal guides, from its top edge), "clear"?: bool (remove every guide first)}"#,
        ),
        C::new("layout.duplicateElement", "Duplicate Element", "Publish › Element", duplicate_element).params(
            r#"{"layout"?, "element": id, "dx"?, "dy"?: mm (offset of the copy, default 5)}"#,
        ),
        C::new("layout.removeElement", "Delete Element", "Publish › Element", |s, p| {
            let id = element_id(p)?;
            let l = active(s, p)?;
            // A frame's surrounds go with it: a locked one keeps the frame too.
            if let Some(locked) = l.locked_in_removal(id).map(|e| e.id) {
                unlocked(l, locked)?;
            }
            if l.remove(id) { Ok(Value::Null) } else { Err(format!("no element {id}")) }
        })
        .params(r#"{"layout"?, "element": id}"#),
        C::new("layout.frameZoom", "Zoom Frame", "Publish › Map Frame", frame_zoom).params(
            r#"{"layout"?, "element"?: frame id, "frame"?: (as element), "to"?: "view" (the map view's extent, bearing and pitch; the default)|"full"|"selection"|"layer", "layer"?}"#,
        ),
        C::new("layout.series", "Map Series", "Publish › Map Frames", series).params(
            r#"{"layout"?, "layer": index layer, "nameField"?: string, "frame"?: id, "margin"?: 0..1, "off"?: bool}"#,
        ),
        C::new("layout.seriesPage", "Map Series Page", "Publish › Map Frames", series_page).params(r#"{"layout"?, "page": number|"next"|"previous"}"#),
        C::new("layout.seriesRefresh", "Refresh Map Series", "Publish › Map Frames", |s, p| series_page(s, &json!({"layout": p.get("layout"), "page": "current"})))
            .params(r#"{"layout"?}"#),
        C::new("layout.export", "Export Layout", "Publish › Output", export)
            .view()
            .params(r#"{"layout"?, "path": "*.png|*.pdf", "dpi"?: 72..600, "allPages"?: bool (default true: every map series page, up to 500; false: the current page; PDF)}"#),
        C::new("layout.save", "Save Layout File", "Publish › Save As", |s, p| {
            let path = ps(p, "path").ok_or("path is required")?;
            let l = active(s, p)?.clone();
            let text = serde_json::to_string_pretty(&l).map_err(|e| e.to_string())?;
            rustgis_data::io::write_atomic(Path::new(path), text.as_bytes()).map_err(|e| format!("{path}: {e}"))?;
            Ok(json!({"path": path}))
        })
        .view()
        .params(r#"{"layout"?, "path": "*.rgislayout"}"#),
        C::new("layout.import", "Import Layout", "Home › Project", |s, p| {
            let text = match (ps(p, "text"), ps(p, "path")) {
                (Some(t), _) => t.to_string(),
                (None, Some(path)) => {
                    use std::io::Read;
                    // Read at most one byte past the limit, so a file growing meanwhile is still refused.
                    let mut text = String::new();
                    std::fs::File::open(path)
                        .and_then(|f| f.take(MAX_LAYOUT + 1).read_to_string(&mut text))
                        .map_err(|e| format!("{path}: {e}"))?;
                    if text.len() as u64 > MAX_LAYOUT {
                        return Err("the layout file is too large".into());
                    }
                    text
                }
                (None, None) => return Err("path or text is required".into()),
            };
            import(s, &text)
        })
        .params(r#"{"path"?: "*.rgislayout", "text"?: layout JSON}"#),
    ]
}

/// The layout named or numbered by `layout`, else the active one, else the only one.
fn index(s: &Session, p: &Value) -> Result<usize, String> {
    let ls = &s.project.layouts;
    // A `layout` that is neither a name nor an index is an error, never the open layout.
    match p.get("layout") {
        None | Some(Value::Null) => {}
        Some(Value::String(n)) => {
            return ls.iter().position(|l| l.name.eq_ignore_ascii_case(n)).ok_or_else(|| format!("no layout '{n}'"));
        }
        Some(_) => {
            let i = super::uint(p, "layout").ok_or("layout must be a layout's name or index")?;
            return usize::try_from(i).ok().filter(|i| *i < ls.len()).ok_or_else(|| format!("no layout {i}"));
        }
    }
    s.active_layout
        .filter(|i| *i < ls.len())
        .or(if ls.len() == 1 { Some(0) } else { None })
        .ok_or_else(|| "open a layout first".to_string())
}

/// Parameters where `name` names the layout (Open, Delete and Rename Layout) as `layout`.
fn by_name(p: &Value) -> Value {
    match (ps(p, "name"), p.get("layout")) {
        (Some(n), None) => json!({"layout": n}),
        _ => p.clone(),
    }
}

fn active<'a>(s: &'a mut Session, p: &Value) -> Result<&'a mut Layout, String> {
    let i = index(s, p)?;
    s.project.layouts.get_mut(i).ok_or_else(|| "open a layout first".to_string())
}

fn element_id(p: &Value) -> Result<u32, String> {
    p.get("element").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()).ok_or_else(|| "element is required".to_string())
}

/// Refuses to change a locked element (#536).
fn unlocked(l: &Layout, id: u32) -> Result<(), String> {
    match l.element(id) {
        Some(e) if e.locked => Err(format!("'{}' is locked: unlock it first", e.name)),
        _ => Ok(()),
    }
}

fn unique_name(s: &Session, base: &str) -> String {
    let base = base.trim();
    let base = if base.is_empty() { "Layout" } else { base };
    numbered(base, |n| s.project.layouts.iter().any(|l| l.name.eq_ignore_ascii_case(n)))
}

/// (width, height) in mm from page/orientation/width/height parameters.
pub(crate) fn page_size(p: &Value, current: Option<(f64, f64)>) -> Result<(f64, f64), String> {
    let (mut w, mut h) = match ps(p, "page") {
        Some(id) => PAGES
            .iter()
            .find(|(pid, label, ..)| pid.eq_ignore_ascii_case(id) || label.eq_ignore_ascii_case(id))
            .map(|(_, _, w, h)| (*w, *h))
            .ok_or_else(|| format!("unknown page size '{id}'"))?,
        None => current.unwrap_or((215.9, 279.4)),
    };
    if let Some(v) = f(p, "width") {
        w = v;
    }
    if let Some(v) = f(p, "height") {
        h = v;
    }
    match ps(p, "orientation") {
        Some("landscape") if h > w => std::mem::swap(&mut w, &mut h),
        Some("portrait") if w > h => std::mem::swap(&mut w, &mut h),
        Some("landscape" | "portrait") | None => {}
        Some(o) => return Err(format!("unknown orientation '{o}'")),
    }
    if !(20.0..=5000.0).contains(&w) || !(20.0..=5000.0).contains(&h) {
        return Err("pages are 20 to 5000 mm on a side".into());
    }
    Ok((w, h))
}

/// The active map's view as a frame centre and scale for a frame `w_mm` wide.
fn view_frame(s: &Session, w_mm: f64) -> (Coord, f64) {
    let m = s.map();
    let ground_w = s.view_size.0 * m.ground_res();
    (m.center_lonlat(), (ground_w / (w_mm / 1000.0)).clamp(100.0, 1e10))
}

/// The active map's bearing and pitch, for a frame showing its view.
fn view_turn(s: &Session) -> (f64, f64) {
    let m = s.map();
    (m.camera.bearing, m.camera.pitch)
}

fn frame_kind(s: &Session, map: &str, w: f64, shape: FrameShape) -> ElementKind {
    let (center, scale) = view_frame(s, w);
    let (bearing, pitch) = view_turn(s);
    ElementKind::MapFrame {
        map: map.to_string(),
        center,
        scale,
        shape,
        grid: None,
        extent_of: None,
        extent_color: rustgis_map::layout::INDICATOR_COLOR,
        extent_width: rustgis_map::layout::INDICATOR_WIDTH,
        bearing,
        pitch,
    }
}

pub(crate) fn new_layout(s: &mut Session, p: &Value) -> CmdResult {
    let (w, h) = page_size(
        &json!({"page": ps(p, "page").unwrap_or("letter"), "orientation": ps(p, "orientation").unwrap_or("landscape"), "width": p.get("width"), "height": p.get("height")}),
        None,
    )?;
    let name = unique_name(s, ps(p, "name").unwrap_or("Layout"));
    let mut l = Layout::new(name.clone(), w, h);
    match ps(p, "template").unwrap_or("map") {
        "blank" => {}
        "map" => {
            let map = s.map().name.clone();
            let m = 10.0;
            let fw = (w - 2.0 * m) * 0.72;
            let fh = h - 2.0 * m - 18.0;
            let frame = l.add("Map Frame", (m, m + 16.0, fw, fh), frame_kind(s, &map, fw, FrameShape::Rectangle));
            let side = m + fw + 6.0;
            let sw = w - side - m;
            l.add("Title", (m, m, w - 2.0 * m, 14.0), ElementKind::text(map.clone(), 22.0, Rgba::rgb(0x1F, 0x29, 0x37), true));
            l.add(
                "Legend",
                (side, m + 16.0, sw, fh * 0.6),
                ElementKind::Legend { frame, title: "Legend".into(), all_classes: false, hide_other: false, exclude: Vec::new() },
            );
            l.add("North Arrow", (side + sw / 2.0 - 8.0, m + 16.0 + fh * 0.62, 16.0, 24.0), ElementKind::north_arrow(frame));
            l.add("Scale Bar", (side, m + 16.0 + fh * 0.62 + 28.0, sw, 12.0), ElementKind::scale_bar(frame));
            l.add(
                "Credits",
                (side, h - m - 10.0, sw, 10.0),
                ElementKind::text("{scale} · {date}", 8.0, Rgba::rgb(0x55, 0x5B, 0x66), false),
            );
        }
        t => return Err(format!("unknown template '{t}'")),
    }
    s.project.layouts.push(l);
    let i = s.project.layouts.len() - 1;
    s.active_layout = Some(i);
    Ok(json!({"layout": i, "name": name}))
}

fn page(s: &mut Session, p: &Value) -> CmdResult {
    let i = index(s, p)?;
    let cur = s.project.layouts.get(i).map(|l| (l.width, l.height));
    let (w, h) = page_size(p, cur)?;
    let l = s.project.layouts.get_mut(i).ok_or("no layout")?;
    l.width = w;
    l.height = h;
    Ok(json!({"width": w, "height": h}))
}

/// Sets a layout's ruler guides: the lists given replace those there.
fn guides(s: &mut Session, p: &Value) -> CmdResult {
    let read = |k: &str| -> Result<Option<Vec<f64>>, String> {
        let bad = || format!("{k} is a list of positions in mm");
        match p.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(a)) if a.len() <= MAX_GUIDES => a
                .iter()
                .map(|v| v.as_f64().filter(|v| v.is_finite() && v.abs() <= 5000.0).ok_or_else(bad))
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            Some(Value::Array(_)) => Err(format!("a layout has at most {MAX_GUIDES} guides each way")),
            Some(_) => Err(bad()),
        }
    };
    // Checked before anything changes.
    let (x, y) = (read("x")?, read("y")?);
    let l = active(s, p)?;
    if b(p, "clear") == Some(true) {
        l.guides = Guides::default();
    }
    if let Some(x) = x {
        l.guides.x = x;
    }
    if let Some(y) = y {
        l.guides.y = y;
    }
    Ok(json!({"x": l.guides.x, "y": l.guides.y}))
}

/// Copies an element, offset a little so the copy shows, on top of the others.
fn duplicate_element(s: &mut Session, p: &Value) -> CmdResult {
    let id = element_id(p)?;
    let (dx, dy) = (f(p, "dx").unwrap_or(5.0), f(p, "dy").unwrap_or(5.0));
    if dx.abs() > 5000.0 || dy.abs() > 5000.0 {
        return Err("the copy's offset is up to 5000 mm".into());
    }
    let l = active(s, p)?;
    let e = l.element(id).cloned().ok_or_else(|| format!("no element {id}"))?;
    let name = numbered(&e.name, |n| l.elements.iter().any(|x| x.name == n));
    let copy = l.add(name.clone(), (e.x + dx, e.y + dy, e.w, e.h), e.kind);
    if let Some(c) = l.element_mut(copy) {
        c.visible = e.visible;
    }
    Ok(json!({"element": copy, "name": name}))
}

/// A shape's "width" in points: `default` when it's not given (or null), an error when it's not a
/// number.
fn shape_width(p: &Value, default: f64) -> Result<f64, String> {
    match p.get("width").filter(|v| !v.is_null()) {
        Some(_) => f(p, "width").ok_or_else(|| "width is a number of points".to_string()),
        None => Ok(default),
    }
}

/// A line's "head": `default` when it's not given (or null), an error when it's not an
/// arrowhead's name.
fn line_head(p: &Value, default: LineHead) -> Result<LineHead, String> {
    match p.get("head").filter(|v| !v.is_null()) {
        Some(v) => v.as_str().and_then(LineHead::from_name).ok_or_else(|| format!("unknown arrowhead {v} (none, end, start or both)")),
        None => Ok(default),
    }
}

/// A number given as `k`, when it's given (and not null): an error when it's not a number.
fn given_num(p: &Value, k: &str, what: &str) -> Result<Option<f64>, String> {
    match p.get(k).filter(|v| !v.is_null()) {
        Some(_) => f(p, k).map(Some).ok_or_else(|| format!("{k} is {what}")),
        None => Ok(None),
    }
}

/// A line's "dir": `default` when it's not given (or null), an error when it's not a direction's
/// name.
fn line_dir(p: &Value, default: LineDir) -> Result<LineDir, String> {
    match p.get("dir").filter(|v| !v.is_null()) {
        Some(v) => v
            .as_str()
            .and_then(LineDir::from_name)
            .ok_or_else(|| format!("unknown line direction {v} (horizontal, vertical, falling or rising)")),
        None => Ok(default),
    }
}

/// The element's box: given values, else a default inside the page.
fn place(p: &Value, l: &Layout, (dx, dy, dw, dh): (f64, f64, f64, f64)) -> Result<(f64, f64, f64, f64), String> {
    let v = |k: &str, d: f64| f(p, k).unwrap_or(d);
    let (x, y, w, h) = (v("x", dx), v("y", dy), v("w", dw.min(l.width - 10.0).max(5.0)), v("h", dh.min(l.height - 10.0).max(5.0)));
    if w <= 0.0 || h <= 0.0 || w > 5000.0 || h > 5000.0 || x.abs() > 5000.0 || y.abs() > 5000.0 {
        return Err("element sizes are more than 0 and up to 5000 mm".into());
    }
    Ok((x, y, w, h))
}

fn add_frame(s: &mut Session, p: &Value) -> CmdResult {
    let shape = match ps(p, "shape").unwrap_or("rectangle") {
        "rectangle" => FrameShape::Rectangle,
        "circle" => FrameShape::Circle,
        "ellipse" => FrameShape::Ellipse,
        "polygon" | "lasso" => {
            let pts: Vec<(f64, f64)> = p
                .get("points")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .take(10_000)
                        .filter_map(|v| Some((v.get(0)?.as_f64()?, v.get(1)?.as_f64()?)))
                        .filter(|(u, v)| u.is_finite() && v.is_finite())
                        .map(|(u, v)| (u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)))
                        .collect()
                })
                .unwrap_or_else(|| vec![(0.5, 0.0), (1.0, 0.38), (0.81, 1.0), (0.19, 1.0), (0.0, 0.38)]);
            if pts.len() < 3 {
                return Err("a polygon frame needs at least three points".into());
            }
            FrameShape::Polygon(pts)
        }
        o => return Err(format!("unknown frame shape '{o}'")),
    };
    let map = match ps(p, "map") {
        Some(n) => {
            s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(n)).map(|m| m.name.clone()).ok_or_else(|| format!("no map '{n}'"))?
        }
        None => s.map().name.clone(),
    };
    let i = index(s, p)?;
    let l = s.project.layouts.get(i).ok_or("no layout")?;
    let n = l.elements.len() as f64;
    let rect = place(p, l, (15.0 + n * 5.0, 25.0 + n * 5.0, l.width * 0.5, l.height * 0.5))?;
    let kind = frame_kind(s, &map, rect.2, shape);
    let l = s.project.layouts.get_mut(i).ok_or("no layout")?;
    let id = l.add("Map Frame", rect, kind);
    Ok(json!({"element": id}))
}

fn frame_of(l: &Layout, p: &Value) -> Result<u32, String> {
    match p.get("frame").and_then(Value::as_u64) {
        Some(id) => {
            let id = u32::try_from(id).map_err(|_| "no such frame")?;
            match l.element(id).map(|e| &e.kind) {
                Some(ElementKind::MapFrame { .. }) => Ok(id),
                _ => Err(format!("element {id} isn't a map frame")),
            }
        }
        None => l.first_frame().ok_or_else(|| "add a map frame first".to_string()),
    }
}

pub(crate) fn color(p: &Value, k: &str, d: Rgba) -> Result<Rgba, String> {
    match ps(p, k) {
        Some(c) => Rgba::from_hex(c).ok_or_else(|| format!("{k} must be #RRGGBB or #RRGGBBAA")),
        None => Ok(d),
    }
}

fn add_element(s: &mut Session, p: &Value) -> CmdResult {
    let kind = ps(p, "kind").ok_or("kind is required")?;
    if kind != "legend" && p.get("exclude").is_some_and(|v| !v.is_null()) {
        return Err("only legends leave layers out".into());
    }
    let i = index(s, p)?;
    let image = if kind == "picture" { Some(super::graphics::picture(p)?) } else { None };
    let layer_name = |s: &Session, l: &Layout, frame: u32| -> Result<String, String> {
        let map = match l.element(frame).map(|e| &e.kind) {
            Some(ElementKind::MapFrame { map, .. }) => map.clone(),
            _ => return Err("no such frame".into()),
        };
        let m = s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(&map)).ok_or_else(|| format!("no map '{map}'"))?;
        let found = match ps(p, "layer") {
            Some(n) => m.layer_by_name(n),
            None => m.layers.iter().find(|l| !l.is_raster() && !l.graphics),
        };
        found.map(|l| l.name.clone()).ok_or_else(|| "no such layer in the frame's map".to_string())
    };
    let l = s.project.layouts.get(i).ok_or("no layout")?;
    let (pw, ph) = (l.width, l.height);
    let (name, rect, k) = match kind {
        "northArrow" => {
            let frame = frame_of(l, p)?;
            let k = restyle(&ElementKind::north_arrow(frame), p)?.unwrap_or(ElementKind::north_arrow(frame));
            ("North Arrow", place(p, l, (pw - 30.0, 15.0, 16.0, 24.0))?, k)
        }
        "scaleBar" => {
            let frame = frame_of(l, p)?;
            let k = restyle(&ElementKind::scale_bar(frame), p)?.unwrap_or(ElementKind::scale_bar(frame));
            ("Scale Bar", place(p, l, (15.0, ph - 25.0, 70.0, 12.0))?, k)
        }
        "legend" => {
            let frame = frame_of(l, p)?;
            let title = ps(p, "title").unwrap_or("Legend").chars().take(200).collect();
            let all_classes = b(p, "allClasses").unwrap_or(false);
            let hide_other = b(p, "hideOther").unwrap_or(false);
            let exclude = legend_exclude(s, l, frame, p)?.unwrap_or_default();
            ("Legend", place(p, l, (pw - 70.0, 45.0, 60.0, 80.0))?, ElementKind::Legend { frame, title, all_classes, hide_other, exclude })
        }
        "colorbar" => {
            let frame = frame_of(l, p)?;
            let layer = colorbar_layer(s, l, frame, ps(p, "layer"))?;
            let title = ps(p, "title").map(|t| t.chars().take(200).collect()).unwrap_or_else(|| layer.clone().unwrap_or_default());
            let orientation = orientation(p)?.unwrap_or_default();
            let rect = match orientation {
                Orientation::Horizontal => (15.0, ph - 45.0, 80.0, 16.0),
                Orientation::Vertical => (pw - 40.0, 45.0, 25.0, 80.0),
            };
            let k = ElementKind::Colorbar {
                frame,
                // Kept only when named: otherwise (none, or "") it follows the map's top visible raster.
                layer: ps(p, "layer").filter(|n| !n.trim().is_empty()).and(layer),
                title,
                orientation,
                ticks: ticks(p).unwrap_or(5),
                tick_side: tick_side(p)?.unwrap_or_default(),
                reverse: b(p, "reverse").unwrap_or(false),
            };
            ("Colorbar", place(p, l, rect)?, k)
        }
        "text" | "dynamicText" => {
            let default = if kind == "text" { "Text" } else { "{map} · {scale} · {date}" };
            let text: String = ps(p, "text").unwrap_or(default).chars().take(5000).collect();
            let fonts = rustgis_map::graphics::FONT_SIZE;
            let size = f(p, "size").unwrap_or(12.0).clamp(*fonts.start(), *fonts.end());
            let (italic, align, letter_spacing) = text_style(p, false, TextAlign::Left, 0.0)?;
            let k = ElementKind::Text {
                text,
                size,
                color: color(p, "color", Rgba::rgb(0x1F, 0x29, 0x37))?,
                bold: b(p, "bold").unwrap_or(false),
                italic,
                align,
                letter_spacing,
            };
            (if kind == "text" { "Text" } else { "Dynamic Text" }, place(p, l, (15.0, 12.0, 120.0, (size * 0.5).max(8.0)))?, k)
        }
        "picture" => {
            ("Picture", place(p, l, (pw - 60.0, ph - 50.0, 45.0, 35.0))?, ElementKind::Picture { image: image.unwrap_or_default() })
        }
        "table" => {
            let frame = frame_of(l, p)?;
            let layer = layer_name(s, l, frame)?;
            let rows = p.get("rows").and_then(Value::as_u64).unwrap_or(12).clamp(1, 500) as usize;
            ("Table Frame", place(p, l, (15.0, ph * 0.6, pw * 0.5, ph * 0.35))?, ElementKind::Table { frame, layer, rows })
        }
        "chart" => {
            let frame = frame_of(l, p)?;
            let layer = layer_name(s, l, frame)?;
            let m = s.project.maps.iter().find_map(|m| m.layer_by_name(&layer)).ok_or("no such layer")?;
            let field = match ps(p, "field") {
                Some(fl) => m
                    .dataset
                    .field_index(fl)
                    .and_then(|i| m.dataset.fields.get(i))
                    .map(|f| f.name.clone())
                    .ok_or_else(|| format!("no field '{fl}'"))?,
                None => {
                    m.dataset.fields.iter().find(|f| f.ty.is_numeric()).map(|f| f.name.clone()).ok_or("the layer has no numeric field")?
                }
            };
            let label = ps(p, "label").and_then(|n| m.dataset.field_index(n)).and_then(|i| m.dataset.fields.get(i)).map(|f| f.name.clone());
            ("Chart Frame", place(p, l, (pw * 0.55, ph * 0.6, pw * 0.4, ph * 0.35))?, ElementKind::Chart { frame, layer, field, label })
        }
        "rectangle" => {
            let k = ElementKind::Rectangle {
                fill: color(p, "fill", Rgba::new(0, 0, 0, 0))?,
                outline: color(p, "color", Rgba::rgb(0x22, 0x22, 0x22))?,
                width: shape_width(p, 1.0)?.clamp(0.0, 50.0),
            };
            // Without a box it frames the page, as a neatline does; with any part of one it's a shape.
            if ["x", "y", "w", "h"].iter().any(|k| p.get(k).is_some_and(|v| !v.is_null())) {
                ("Rectangle", place(p, l, (pw * 0.4, ph * 0.4, pw * 0.2, ph * 0.2))?, k)
            } else {
                ("Neatline", place(p, l, (5.0, 5.0, pw - 10.0, ph - 10.0))?, k)
            }
        }
        "ellipse" | "circle" => {
            let k = ElementKind::Ellipse {
                fill: color(p, "fill", Rgba::new(0, 0, 0, 0))?,
                outline: color(p, "color", Rgba::rgb(0x22, 0x22, 0x22))?,
                width: shape_width(p, 1.0)?.clamp(0.0, 50.0),
            };
            // A circle is an ellipse in a square box.
            let d = (pw.min(ph) * 0.25).max(5.0);
            let (w, h) = if kind == "circle" { (d, d) } else { (d * 1.5, d) };
            let name = if kind == "circle" { "Circle" } else { "Ellipse" };
            (name, place(p, l, ((pw - w) / 2.0, (ph - h) / 2.0, w, h))?, k)
        }
        "line" | "arrow" => {
            let arrow = kind == "arrow";
            let head = line_head(p, if arrow { LineHead::End } else { LineHead::None })?;
            let color = color(p, "color", Rgba::rgb(0x22, 0x22, 0x22))?;
            let width = shape_width(p, if arrow { 1.5 } else { 1.0 })?.clamp(0.1, 50.0);
            let len = (pw.min(ph) * 0.4).max(10.0);
            let name = if head == LineHead::None { "Line" } else { "Arrow" };
            // At an angle, centred on the page unless x or y moves it; else the box is the
            // direction's, with room either side of a level or upright line to pick it up by.
            let (rect, dir, head) = match given_num(p, "angle", "a number of degrees")? {
                Some(angle) => {
                    // The angle sets the direction, so a "dir" as well would be overruled.
                    if p.get("dir").is_some_and(|v| !v.is_null()) {
                        return Err("give a line a dir or an angle, not both".into());
                    }
                    let length = given_num(p, "length", "a number of mm")?.unwrap_or(len);
                    if length <= 0.0 {
                        return Err("length is more than 0 mm".into());
                    }
                    // The box is exact for the angle and length: place() would resize it.
                    let ((x, y, w, h), dir, head) = line_at_angle((pw / 2.0, ph / 2.0), length, angle, head);
                    let (x, y) = (f(p, "x").unwrap_or(x), f(p, "y").unwrap_or(y));
                    if x.abs() > 5000.0 || y.abs() > 5000.0 {
                        return Err("element positions are up to 5000 mm".into());
                    }
                    ((x, y, w, h), dir, head)
                }
                None => {
                    let dir = line_dir(p, LineDir::Horizontal)?;
                    let (w, h) = match dir {
                        LineDir::Horizontal => (len, 6.0),
                        LineDir::Vertical => (6.0, len),
                        LineDir::Falling | LineDir::Rising => (len, len * 0.6),
                    };
                    (place(p, l, ((pw - w) / 2.0, (ph - h) / 2.0, w, h))?, dir, head)
                }
            };
            (name, rect, ElementKind::Line { color, width, dir, head })
        }
        "extentIndicator" => {
            let frame = frame_of(l, p)?;
            let of =
                p.get("of").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()).ok_or("of (the frame to outline) is required")?;
            if of == frame || !matches!(l.element(of).map(|e| &e.kind), Some(ElementKind::MapFrame { .. })) {
                return Err("of must be another map frame".into());
            }
            unlocked(l, frame)?;
            let (color, width) = indicator_style(p, l.element(frame).map(|e| &e.kind))?;
            let l = active(s, p)?;
            if let Some(ElementKind::MapFrame { extent_of, extent_color, extent_width, .. }) = l.element_mut(frame).map(|e| &mut e.kind) {
                *extent_of = Some(of);
                *extent_color = color;
                *extent_width = width;
            }
            return Ok(json!({"element": frame}));
        }
        "grid" => {
            let frame = frame_of(l, p)?;
            unlocked(l, frame)?;
            // Options not given keep the frame's grid's, or the defaults for a new grid.
            let base = match l.element(frame).map(|e| &e.kind) {
                Some(ElementKind::MapFrame { grid: Some(g), .. }) => g.clone(),
                _ => Grid::default(),
            };
            let new = if b(p, "off").unwrap_or(false) { None } else { Some(grid_params(p, base)?) };
            let l = active(s, p)?;
            if let Some(ElementKind::MapFrame { grid, .. }) = l.element_mut(frame).map(|e| &mut e.kind) {
                *grid = new;
            }
            return Ok(json!({"element": frame}));
        }
        o => return Err(format!("unknown element kind '{o}'")),
    };
    let l = s.project.layouts.get_mut(i).ok_or("no layout")?;
    let id = l.add(name, rect, k);
    Ok(json!({"element": id}))
}

/// The raster layer a colorbar on `frame` shows: `name` checked against the frame's map, or the
/// map's top visible raster (`None` when the map has none yet).
fn colorbar_layer(s: &Session, l: &Layout, frame: u32, name: Option<&str>) -> Result<Option<String>, String> {
    let map = match l.element(frame).map(|e| &e.kind) {
        Some(ElementKind::MapFrame { map, .. }) => map.clone(),
        _ => return Err("no such frame".into()),
    };
    let m = s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(&map)).ok_or_else(|| format!("no map '{map}'"))?;
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => match rustgis_map::layout::colorbar_layer(m, Some(n)) {
            Some(layer) => Ok(Some(layer.name.clone())),
            None => Err(format!("no raster layer '{n}' in {map}")),
        },
        None => Ok(rustgis_map::layout::colorbar_layer(m, None).map(|l| l.name.clone())),
    }
}

fn orientation(p: &Value) -> Result<Option<Orientation>, String> {
    orientation_of(p, "orientation")
}

fn orientation_of(p: &Value, key: &str) -> Result<Option<Orientation>, String> {
    match ps(p, key).map(|o| o.trim().to_ascii_lowercase()) {
        None => Ok(None),
        Some(o) if o == "horizontal" => Ok(Some(Orientation::Horizontal)),
        Some(o) if o == "vertical" => Ok(Some(Orientation::Vertical)),
        Some(o) => Err(format!("unknown orientation '{o}' (horizontal or vertical)")),
    }
}

fn tick_side(p: &Value) -> Result<Option<TickSide>, String> {
    match ps(p, "tickSide") {
        None => Ok(None),
        Some(t) => TickSide::from_name(t).map(Some).ok_or_else(|| format!("unknown tickSide '{t}' (below, above, right or left)")),
    }
}

fn ticks(p: &Value) -> Option<usize> {
    f(p, "ticks").filter(|v| v.is_finite()).map(|v| v.round().clamp(2.0, MAX_COLORBAR_TICKS as f64) as usize)
}

/// An extent indicator's outline colour and width (points) from `color` and `width` in `p`, each
/// keeping the frame's (`kind`) when not given; one given but not a colour or a number is an
/// error, not left as it was.
fn indicator_style(p: &Value, kind: Option<&ElementKind>) -> Result<(Rgba, f64), String> {
    let (c, w) = match kind {
        Some(ElementKind::MapFrame { extent_color, extent_width, .. }) => (*extent_color, *extent_width),
        _ => (rustgis_map::layout::INDICATOR_COLOR, rustgis_map::layout::INDICATOR_WIDTH),
    };
    let given = |k: &str| p.get(k).is_some_and(|v| !v.is_null());
    let width = match (given("width"), f(p, "width")) {
        (_, Some(v)) => v.clamp(rustgis_map::layout::MIN_INDICATOR_WIDTH, rustgis_map::layout::MAX_INDICATOR_WIDTH),
        (true, None) => return Err("width must be a number of points".into()),
        (false, None) => w,
    };
    if given("color") && ps(p, "color").is_none() {
        return Err("color must be #RRGGBB or #RRGGBBAA".into());
    }
    Ok((color(p, "color", c)?, width))
}

/// A grid with the options in `p` over `base`'s; a bad value is an error, leaving nothing changed.
fn grid_params(p: &Value, base: Grid) -> Result<Grid, String> {
    let mut g = base;
    if let Some(v) = f(p, "step") {
        g.step = v.clamp(0.0, 90.0);
    }
    g.color = color(p, "color", g.color)?;
    if let Some(v) = b(p, "labels") {
        g.labels = v;
    }
    if let Some(v) = ps(p, "style") {
        g.style = GridStyle::from_name(v).ok_or_else(|| format!("unknown grid style '{v}' (lines, ticks or linesAndTicks)"))?;
    }
    if let Some(v) = f(p, "tickLength") {
        g.tick_length = v.clamp(0.0, MAX_TICK_MM);
    }
    if let Some(v) = f(p, "width") {
        g.width = v.clamp(0.0, MAX_GRID_WIDTH);
    }
    if let Some(v) = ps(p, "format") {
        g.format = CoordFormat::from_name(v).ok_or_else(|| format!("unknown format '{v}' (decimal or dms)"))?;
    }
    if let Some(v) = b(p, "hemisphere") {
        g.hemisphere = v;
    }
    if let Some(v) = p.get("edges") {
        g.edges = edges(v)?;
    }
    if let Some(o) = orientation_of(p, "labelOrientation")? {
        g.label_orientation = o;
    }
    Ok(g)
}

/// Labelled edges from `["top", "bottom", "left", "right"]` (any of them) or `"bottom,left"`.
fn edges(v: &Value) -> Result<GridEdges, String> {
    let names: Vec<String> = match v {
        Value::Array(a) => a.iter().map(|n| n.as_str().map(str::to_string).ok_or("edges are names")).collect::<Result<_, _>>()?,
        Value::String(t) => t.split(',').map(str::to_string).collect(),
        _ => return Err("edges is a list of top, bottom, left and right".into()),
    };
    let mut e = GridEdges { top: false, bottom: false, left: false, right: false };
    for n in names.iter().map(|n| n.trim().to_ascii_lowercase()).filter(|n| !n.is_empty()) {
        match n.as_str() {
            "top" => e.top = true,
            "bottom" => e.bottom = true,
            "left" => e.left = true,
            "right" => e.right = true,
            o => return Err(format!("unknown edge '{o}' (top, bottom, left or right)")),
        }
    }
    Ok(e)
}

/// A text element's italic, alignment and letter spacing (points) from `italic`, `align` and
/// `letterSpacing` in `p`, each keeping the current one when not given; one given but not of its
/// kind is an error.
pub(crate) fn text_style(p: &Value, italic: bool, align: TextAlign, spacing: f64) -> Result<(bool, TextAlign, f64), String> {
    let given = |k: &str| p.get(k).is_some_and(|v| !v.is_null());
    let italic = match p.get("italic") {
        None | Some(Value::Null) => italic,
        Some(Value::Bool(v)) => *v,
        Some(Value::String(t)) if ["true", "yes", "1", "on"].contains(&t.trim()) => true,
        Some(Value::String(t)) if ["false", "no", "0", "off"].contains(&t.trim()) => false,
        Some(_) => return Err("italic must be true or false".into()),
    };
    let align = match ps(p, "align") {
        Some(a) => TextAlign::from_name(a).ok_or_else(|| format!("unknown alignment '{a}' (left, center or right)"))?,
        None if given("align") => return Err("align must be left, center or right".into()),
        None => align,
    };
    let spacing = match (given("letterSpacing"), f(p, "letterSpacing")) {
        (_, Some(v)) => v.clamp(rustgis_map::layout::MIN_LETTER_SPACING, rustgis_map::layout::MAX_LETTER_SPACING),
        (true, None) => return Err("letterSpacing must be a number of points".into()),
        (false, None) => spacing,
    };
    Ok((italic, align, spacing))
}

/// A legend's `exclude` from `p`: the names of layers in its frame's map, as the map spells them;
/// `None` when not given. Not a list of names, or a name the map hasn't, is an error.
fn legend_exclude(s: &Session, l: &Layout, frame: u32, p: &Value) -> Result<Option<Vec<String>>, String> {
    let Some(v) = p.get("exclude").filter(|v| !v.is_null()) else { return Ok(None) };
    let names = v.as_array().ok_or("exclude is a list of layer names")?;
    if names.len() > rustgis_map::layout::MAX_LEGEND_EXCLUDED {
        return Err(format!("a legend leaves out at most {} layers", rustgis_map::layout::MAX_LEGEND_EXCLUDED));
    }
    let map_name = match l.element(frame).map(|e| &e.kind) {
        Some(ElementKind::MapFrame { map, .. }) => map.clone(),
        _ => return Err("the legend's frame isn't a map frame".into()),
    };
    let map = s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(&map_name)).ok_or_else(|| format!("no map '{map_name}'"))?;
    let mut out: Vec<String> = Vec::new();
    for n in names {
        let n = n.as_str().ok_or("exclude is a list of layer names")?;
        let layer = map.layer_by_name(n.trim()).ok_or_else(|| format!("no layer '{n}' in map '{map_name}'"))?;
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&layer.name)) {
            out.push(layer.name.clone());
        }
    }
    Ok(Some(out))
}

/// A north arrow or scale bar with the options in `p` over `kind`'s; `None` for other elements
/// or when `p` has none. A bad value is an error, leaving nothing changed.
fn restyle(kind: &ElementKind, p: &Value) -> Result<Option<ElementKind>, String> {
    match kind {
        ElementKind::NorthArrow { frame, style, fill, outline } => {
            if ["style", "fill", "outline"].iter().all(|k| p.get(k).is_none()) {
                return Ok(None);
            }
            let style = match ps(p, "style") {
                Some(v) => ArrowStyle::from_name(v)
                    .ok_or_else(|| format!("unknown north arrow style '{v}' (classic, compass, triangle or star)"))?,
                None => *style,
            };
            Ok(Some(ElementKind::NorthArrow {
                frame: *frame,
                style,
                fill: color(p, "fill", *fill)?,
                outline: color(p, "outline", *outline)?,
            }))
        }
        ElementKind::ScaleBar { frame, units, divisions, style } => {
            if ["style", "units", "divisions"].iter().all(|k| p.get(k).is_none()) {
                return Ok(None);
            }
            let style = match ps(p, "style") {
                Some(v) => {
                    ScaleBarStyle::from_name(v).ok_or_else(|| format!("unknown scale bar style '{v}' (alternating, stepped or line)"))?
                }
                None => *style,
            };
            let units = match ps(p, "units") {
                Some(v) => {
                    ScaleUnits::from_name(v).ok_or_else(|| format!("unknown units '{v}' (auto, kilometers, meters, miles or feet)"))?
                }
                None => *units,
            };
            let divisions = match p.get("divisions") {
                Some(_) => f(p, "divisions").ok_or("divisions is a number")?.round().clamp(1.0, MAX_SCALE_DIVISIONS as f64) as usize,
                None => *divisions,
            };
            Ok(Some(ElementKind::ScaleBar { frame: *frame, units, divisions, style }))
        }
        ElementKind::Rectangle { fill, outline, width } | ElementKind::Ellipse { fill, outline, width } => {
            if ["fill", "outline", "color", "width"].iter().all(|k| p.get(k).is_none()) {
                return Ok(None);
            }
            // The outline's colour is "outline", or "color" as when adding one.
            let (fill, outline) = (color(p, "fill", *fill)?, color(p, "outline", color(p, "color", *outline)?)?);
            let width = shape_width(p, *width)?.clamp(0.0, 50.0);
            Ok(Some(match kind {
                ElementKind::Ellipse { .. } => ElementKind::Ellipse { fill, outline, width },
                _ => ElementKind::Rectangle { fill, outline, width },
            }))
        }
        ElementKind::Line { color: c, width, dir, head } => {
            if ["color", "width", "dir", "head"].iter().all(|k| p.get(k).is_none()) {
                return Ok(None);
            }
            let dir = line_dir(p, *dir)?;
            let head = line_head(p, *head)?;
            let width = shape_width(p, *width)?.clamp(0.1, 50.0);
            Ok(Some(ElementKind::Line { color: color(p, "color", *c)?, width, dir, head }))
        }
        _ => Ok(None),
    }
}

fn update(s: &mut Session, p: &Value) -> CmdResult {
    let id = element_id(p)?;
    // Checked before anything changes, so a bad value leaves the element as it was.
    let new_center = match p.get("center") {
        Some(c) => {
            let a = c.as_array().ok_or("center is [lon, lat]")?;
            match (a.first().and_then(Value::as_f64), a.get(1).and_then(Value::as_f64)) {
                (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some(Coord::new(x.clamp(-540.0, 540.0), y.clamp(-85.0, 85.0))),
                _ => return Err("center is [lon, lat]".into()),
            }
        }
        None => None,
    };
    for k in ["bearing", "pitch"] {
        if p.get(k).is_some_and(|v| !v.is_null()) && f(p, k).is_none() {
            return Err(format!("{k} must be a number of degrees"));
        }
    }
    let li = index(s, p)?;
    // Locked, it takes no change but being unlocked, which may come with other changes.
    let changes =
        p.as_object().is_some_and(|o| o.iter().any(|(k, v)| !v.is_null() && !matches!(k.as_str(), "layout" | "element" | "locked")));
    if changes && b(p, "locked") != Some(false) {
        unlocked(s.project.layouts.get(li).ok_or("no layout")?, id)?;
    }
    // A new layer (tables and charts) or field (charts) must exist in the frame's map.
    let checked = {
        let l = s.project.layouts.get(li).ok_or("no layout")?;
        let e = l.element(id).ok_or_else(|| format!("no element {id}"))?;
        match (&e.kind, ps(p, "layer"), ps(p, "field")) {
            (ElementKind::Table { frame, layer, .. } | ElementKind::Chart { frame, layer, .. }, nl, nf) if nl.is_some() || nf.is_some() => {
                let map = match l.element(*frame).map(|f| &f.kind) {
                    Some(ElementKind::MapFrame { map, .. }) => map.clone(),
                    _ => return Err("the element's map frame is gone".into()),
                };
                let m = s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(&map)).ok_or_else(|| format!("no map '{map}'"))?;
                let target = m
                    .layer_by_name(nl.unwrap_or(layer))
                    .filter(|l| !l.is_raster())
                    .ok_or_else(|| format!("no feature layer '{}' in {map}", nl.unwrap_or(layer)))?;
                // Only charts have a field.
                let field = match nf.filter(|_| matches!(e.kind, ElementKind::Chart { .. })) {
                    Some(fl) => Some(
                        target
                            .dataset
                            .field_index(fl)
                            .and_then(|i| target.dataset.fields.get(i))
                            .filter(|f| f.ty.is_numeric())
                            .map(|f| f.name.clone())
                            .ok_or_else(|| format!("no numeric field '{fl}' in {}", target.name))?,
                    ),
                    // A chart moved to another layer keeps its field only if that layer has it as a
                    // number; otherwise it charts the layer's first numeric field.
                    None => match &e.kind {
                        ElementKind::Chart { field, .. } if nl.is_some() => {
                            let numeric = |f: &&rustgis_data::Field| f.ty.is_numeric();
                            let keep = target.dataset.field_index(field).and_then(|i| target.dataset.fields.get(i)).filter(numeric);
                            let f = keep
                                .or_else(|| target.dataset.fields.iter().find(numeric))
                                .ok_or_else(|| format!("{} has no numeric field to chart", target.name))?;
                            Some(f.name.clone())
                        }
                        _ => None,
                    },
                };
                Some((target.name.clone(), field))
            }
            _ => None,
        }
    };
    // A colorbar's new layer must be a raster in its frame's map; empty follows the top raster.
    let colorbar = {
        let l = s.project.layouts.get(li).ok_or("no layout")?;
        match l.element(id).map(|e| &e.kind) {
            Some(ElementKind::Colorbar { frame, .. }) => {
                let layer = match ps(p, "layer") {
                    Some(n) if n.trim().is_empty() => Some(None),
                    Some(n) => Some(colorbar_layer(s, l, *frame, Some(n))?),
                    None => None,
                };
                Some((layer, orientation(p)?, tick_side(p)?))
            }
            _ => None,
        }
    };
    // A north arrow's or scale bar's new look, and a map frame's grid: `"grid": false` removes it,
    // `true` adds the default one, an object changes the options it names, and null (not set)
    // leaves it as it is.
    let (restyled, new_grid, new_indicator) = {
        let l = s.project.layouts.get(li).ok_or("no layout")?;
        let kind = &l.element(id).ok_or_else(|| format!("no element {id}"))?.kind;
        // `"extentIndicator": false` takes the frame's indicator away; an object restyles it.
        let indicator = match (kind, p.get("extentIndicator").filter(|v| !v.is_null())) {
            (ElementKind::MapFrame { .. }, Some(Value::Bool(false))) => Some(None),
            (ElementKind::MapFrame { extent_of: Some(_), .. }, Some(v @ Value::Object(_))) => Some(Some(indicator_style(v, Some(kind))?)),
            (ElementKind::MapFrame { .. }, Some(Value::Object(_))) => {
                return Err("this frame has no extent indicator: add one with layout.addElement (kind extentIndicator)".into());
            }
            (ElementKind::MapFrame { .. }, Some(_)) => return Err("extentIndicator is an object of color and width, or false".into()),
            (_, Some(_)) => return Err("only map frames have an extent indicator".into()),
            _ => None,
        };
        let grid = match (kind, p.get("grid").filter(|v| !v.is_null())) {
            (ElementKind::MapFrame { grid, .. }, Some(v)) => Some(match v {
                Value::Bool(false) => None,
                Value::Bool(true) => Some(grid.clone().unwrap_or_default()),
                Value::Object(_) => Some(grid_params(v, grid.clone().unwrap_or_default())?),
                _ => return Err("grid is an object of grid options, or false to remove it".into()),
            }),
            (_, Some(_)) => return Err("only map frames have a grid".into()),
            _ => None,
        };
        (restyle(kind, p)?, grid, indicator)
    };
    // A line turned to an "angle" or made a "length" long, about its centre: its new box,
    // direction and heads.
    let (angle, length) = (given_num(p, "angle", "a number of degrees")?, given_num(p, "length", "a number of mm")?);
    let new_line = match (angle, length) {
        (None, None) => None,
        _ => {
            let e = s.project.layouts.get(li).and_then(|l| l.element(id)).ok_or_else(|| format!("no element {id}"))?;
            let ElementKind::Line { dir, head, .. } = &e.kind else {
                return Err("only lines and arrows take an angle or a length".into());
            };
            // The angle and length set the direction, so a "dir" as well would be overruled.
            if p.get("dir").is_some_and(|v| !v.is_null()) {
                return Err("give a line a dir or an angle and length, not both".into());
            }
            let r = (e.x, e.y, e.w, e.h);
            let length = length.unwrap_or_else(|| line_length(r, *dir));
            if length <= 0.0 {
                return Err("length is more than 0 mm".into());
            }
            // The angle the line points now, read with its current heads; it turns with the new.
            let angle = angle.unwrap_or_else(|| line_angle(r, *dir, *head));
            let head = match &restyled {
                Some(ElementKind::Line { head, .. }) => *head,
                _ => *head,
            };
            Some(line_at_angle((e.x + e.w / 2.0, e.y + e.h / 2.0), length, angle, head))
        }
    };
    let map_name = match ps(p, "map") {
        Some(n) => Some(
            s.project
                .maps
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(n))
                .map(|m| m.name.clone())
                .ok_or_else(|| format!("no map '{n}'"))?,
        ),
        None => None,
    };
    let col = match ps(p, "color") {
        Some(c) => Some(Rgba::from_hex(c).ok_or("color must be #RRGGBB or #RRGGBBAA")?),
        None => None,
    };
    // A text element's italic, alignment and spacing, checked before anything changes.
    let new_text_style = match s.project.layouts.get(li).and_then(|l| l.element(id)).map(|e| &e.kind) {
        Some(ElementKind::Text { italic, align, letter_spacing, .. }) => Some(text_style(p, *italic, *align, *letter_spacing)?),
        _ => None,
    };
    // A legend's left-out layers, checked against its frame's map before anything changes.
    let new_exclude = {
        let l = s.project.layouts.get(li).ok_or("no layout")?;
        match l.element(id).map(|e| &e.kind) {
            Some(ElementKind::Legend { frame, .. }) => legend_exclude(s, l, *frame, p)?,
            _ if p.get("exclude").is_some_and(|v| !v.is_null()) => return Err("only legends leave layers out".into()),
            _ => None,
        }
    };
    let l = active(s, p)?;
    let e = l.element_mut(id).ok_or_else(|| format!("no element {id}"))?;
    let num = |k: &str, lo: f64, hi: f64| f(p, k).map(|v| v.clamp(lo, hi));
    if let Some(v) = num("x", -5000.0, 5000.0) {
        e.x = v;
    }
    if let Some(v) = num("y", -5000.0, 5000.0) {
        e.y = v;
    }
    if let Some(v) = num("w", MIN_ELEMENT_MM, 5000.0) {
        e.w = v;
    }
    if let Some(v) = num("h", MIN_ELEMENT_MM, 5000.0) {
        e.h = v;
    }
    if let Some(n) = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        e.name = n.chars().take(200).collect();
    }
    if let Some(v) = b(p, "visible") {
        e.visible = v;
    }
    if let Some(v) = b(p, "locked") {
        e.locked = v;
    }
    // A colorbar turned keeps its size: its length and thickness swap, unless given.
    if let (ElementKind::Colorbar { orientation, .. }, Some((_, Some(o), _))) = (&e.kind, &colorbar)
        && o != orientation
        && f(p, "w").is_none()
        && f(p, "h").is_none()
    {
        std::mem::swap(&mut e.w, &mut e.h);
    }
    if let Some(k) = restyled {
        e.kind = k;
    }
    // The turned line's box; an "x" or "y" in the same call still moves it.
    if let Some(((x, y, w, h), d, hd)) = new_line {
        (e.x, e.y, e.w, e.h) = (num("x", -5000.0, 5000.0).unwrap_or(x), num("y", -5000.0, 5000.0).unwrap_or(y), w, h);
        if let ElementKind::Line { dir, head, .. } = &mut e.kind {
            (*dir, *head) = (d, hd);
        }
    }
    match &mut e.kind {
        ElementKind::Text { text, size, color, bold, italic, align, letter_spacing } => {
            if let Some((i, a, l)) = new_text_style {
                (*italic, *align, *letter_spacing) = (i, a, l);
            }
            if let Some(t) = ps(p, "text") {
                *text = t.chars().take(5000).collect();
            }
            if let Some(v) = num("size", *rustgis_map::graphics::FONT_SIZE.start(), *rustgis_map::graphics::FONT_SIZE.end()) {
                *size = v;
            }
            if let Some(c) = col {
                *color = c;
            }
            if let Some(v) = b(p, "bold") {
                *bold = v;
            }
        }
        ElementKind::Legend { title, all_classes, hide_other, exclude, .. } => {
            if let Some(names) = new_exclude {
                *exclude = names;
            }
            if let Some(t) = ps(p, "title") {
                *title = t.chars().take(200).collect();
            }
            if let Some(v) = b(p, "allClasses") {
                *all_classes = v;
            }
            if let Some(v) = b(p, "hideOther") {
                *hide_other = v;
            }
        }
        ElementKind::Colorbar { layer, title, orientation, ticks: n, tick_side, reverse, .. } => {
            if let Some(t) = ps(p, "title") {
                *title = t.chars().take(200).collect();
            }
            if let Some((new_layer, new_orientation, new_side)) = colorbar {
                if let Some(nl) = new_layer {
                    *layer = nl;
                }
                if let Some(o) = new_orientation {
                    *orientation = o;
                }
                if let Some(t) = new_side {
                    *tick_side = t;
                }
            }
            if let Some(v) = ticks(p) {
                *n = v;
            }
            if let Some(v) = b(p, "reverse") {
                *reverse = v;
            }
        }
        ElementKind::MapFrame { map, center, scale, grid, bearing, pitch, extent_of, extent_color, extent_width, .. } => {
            match new_indicator {
                Some(None) => *extent_of = None,
                Some(Some((c, w))) => (*extent_color, *extent_width) = (c, w),
                None => {}
            }
            if let Some(m) = map_name {
                *map = m;
            }
            if let Some(v) = f(p, "bearing") {
                *bearing = rustgis_map::camera::normalize_bearing(v);
            }
            if let Some(v) = f(p, "pitch") {
                *pitch = rustgis_map::camera::normalize_pitch(v);
            }
            if let Some(g) = new_grid {
                *grid = g;
            }
            if let Some(v) = num("scale", 100.0, 1e10) {
                *scale = v;
            }
            if let Some(c) = new_center {
                *center = c;
            }
        }
        ElementKind::Table { rows, layer, .. } => {
            if let Some(v) = p.get("rows").and_then(Value::as_u64) {
                *rows = v.clamp(1, 500) as usize;
            }
            if let Some((l, _)) = &checked {
                *layer = l.clone();
            }
        }
        ElementKind::Chart { field, layer, .. } => {
            if let Some((l, f)) = &checked {
                *layer = l.clone();
                if let Some(f) = f {
                    *field = f.clone();
                }
            }
        }
        _ => {}
    }
    Ok(Value::Null)
}

/// Points a frame at an extent.
fn set_frame(s: &mut Session, li: usize, frame: u32, e: &Extent) -> Result<(), String> {
    let l = s.project.layouts.get(li).ok_or("no layout")?;
    let el = l.element(frame).ok_or("no such frame")?;
    let (w, h) = (el.w, el.h);
    let ElementKind::MapFrame { map, .. } = &el.kind else { return Err("that isn't a map frame".into()) };
    let m = s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(map)).ok_or_else(|| format!("no map '{map}'"))?;
    let (c, sc) = fit_scale(m, e, w, h).ok_or("nothing to zoom to")?;
    if let Some(ElementKind::MapFrame { center, scale, .. }) =
        s.project.layouts.get_mut(li).and_then(|l| l.element_mut(frame)).map(|e| &mut e.kind)
    {
        *center = c;
        *scale = sc;
    }
    Ok(())
}

fn frame_zoom(s: &mut Session, p: &Value) -> CmdResult {
    let li = index(s, p)?;
    let l = s.project.layouts.get(li).ok_or("no layout")?;
    let frame = match p.get("element") {
        Some(_) => frame_of(l, &json!({"frame": p.get("element")}))?,
        None => frame_of(l, p)?,
    };
    unlocked(l, frame)?;
    let map = match l.element(frame).map(|e| &e.kind) {
        Some(ElementKind::MapFrame { map, .. }) => map.clone(),
        _ => return Err("no such frame".into()),
    };
    let m = s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(&map)).ok_or_else(|| format!("no map '{map}'"))?;
    match ps(p, "to").unwrap_or("view") {
        "view" => {
            let w = l.element(frame).map(|e| e.w).unwrap_or(100.0);
            let ground_w = s.view_size.0 * m.ground_res();
            let (c, sc) = (m.center_lonlat(), (ground_w / (w / 1000.0)).clamp(100.0, 1e10));
            let (b, t) = view_turn(s);
            if let Some(ElementKind::MapFrame { center, scale, bearing, pitch, .. }) =
                s.project.layouts.get_mut(li).and_then(|l| l.element_mut(frame)).map(|e| &mut e.kind)
            {
                *center = c;
                *scale = sc;
                (*bearing, *pitch) = (b, t);
            }
        }
        "full" => {
            let e = m.shown_extent();
            set_frame(s, li, frame, &e)?;
        }
        "selection" => {
            let e = m.selection_extent();
            set_frame(s, li, frame, &e)?;
        }
        "layer" => {
            let n = ps(p, "layer").ok_or("layer is required")?;
            let e = m.layer_by_name(n).ok_or_else(|| format!("no layer '{n}'"))?.shown_extent();
            set_frame(s, li, frame, &e)?;
        }
        o => return Err(format!("unknown zoom '{o}'")),
    }
    Ok(json!({"element": frame}))
}

fn series(s: &mut Session, p: &Value) -> CmdResult {
    let li = index(s, p)?;
    if b(p, "off").unwrap_or(false) {
        if let Some(l) = s.project.layouts.get_mut(li) {
            l.series = None;
        }
        return Ok(Value::Null);
    }
    let l = s.project.layouts.get(li).ok_or("no layout")?;
    let frame = frame_of(l, p)?;
    let map = match l.element(frame).map(|e| &e.kind) {
        Some(ElementKind::MapFrame { map, .. }) => map.clone(),
        _ => return Err("no such frame".into()),
    };
    let n = ps(p, "layer").ok_or("layer is required")?;
    let m = s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(&map)).ok_or_else(|| format!("no map '{map}'"))?;
    let layer = m.layer_by_name(n).filter(|l| !l.is_raster()).ok_or_else(|| format!("no feature layer '{n}' in {map}"))?;
    if layer.dataset.features.is_empty() {
        return Err("the index layer has no features".into());
    }
    let name_field = match ps(p, "nameField") {
        Some(fl) => layer
            .dataset
            .field_index(fl)
            .and_then(|i| layer.dataset.fields.get(i))
            .map(|f| f.name.clone())
            .ok_or_else(|| format!("no field '{fl}'"))?,
        None => layer.labels.field.clone(),
    };
    let layer = layer.name.clone();
    let margin = f(p, "margin").unwrap_or(0.1).clamp(0.0, 5.0);
    if let Some(l) = s.project.layouts.get_mut(li) {
        l.series = Some(MapSeries { frame, layer, name_field, margin, page: 0 });
    }
    series_page(s, &json!({"layout": li, "page": 0}))
}

/// The pages of a layout's map series: (count, page name of each row).
pub fn series_pages(s: &Session, li: usize) -> Option<Vec<String>> {
    let (layer, fi) = series_layer(s, li)?;
    Some((0..layer.dataset.features.len()).map(|r| page_name(layer, fi, r)).collect())
}

/// A map series' page count and the name of `page`, without naming every page.
pub fn series_info(s: &Session, li: usize, page: usize) -> Option<(usize, String)> {
    let (layer, fi) = series_layer(s, li)?;
    let n = layer.dataset.features.len();
    Some((n, if page < n { page_name(layer, fi, page) } else { String::new() }))
}

/// A map series' index layer and its name field.
fn series_layer(s: &Session, li: usize) -> Option<(&rustgis_map::Layer, Option<usize>)> {
    let l = s.project.layouts.get(li)?;
    let sr = l.series.as_ref()?;
    let map = match l.element(sr.frame).map(|e| &e.kind) {
        Some(ElementKind::MapFrame { map, .. }) => map,
        _ => return None,
    };
    let layer = s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(map))?.layer_by_name(&sr.layer)?;
    Some((layer, layer.dataset.field_index(&sr.name_field)))
}

fn page_name(layer: &rustgis_map::Layer, fi: Option<usize>, row: usize) -> String {
    fi.map(|i| layer.dataset.value(row, i).to_string()).unwrap_or_else(|| (row + 1).to_string())
}

fn series_page(s: &mut Session, p: &Value) -> CmdResult {
    let li = index(s, p)?;
    let (n, _) = series_info(s, li, 0).ok_or("the layout has no map series")?;
    let l = s.project.layouts.get(li).ok_or("no layout")?;
    let sr = l.series.clone().ok_or("the layout has no map series")?;
    let page = match p.get("page") {
        Some(Value::String(t)) if t == "next" => (sr.page + 1).min(n.saturating_sub(1)),
        Some(Value::String(t)) if t == "previous" => sr.page.saturating_sub(1),
        Some(Value::String(t)) if t == "current" => sr.page.min(n.saturating_sub(1)),
        Some(v) => v
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .filter(|v| *v < n)
            .ok_or_else(|| format!("page must be 0 to {}", n.saturating_sub(1)))?,
        None => return Err("page is required".into()),
    };
    // The page's feature extent, with the series margin.
    let map = match l.element(sr.frame).map(|e| &e.kind) {
        Some(ElementKind::MapFrame { map, .. }) => map.clone(),
        _ => return Err("the map series frame is gone".into()),
    };
    let layer = s
        .project
        .maps
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(&map))
        .and_then(|m| m.layer_by_name(&sr.layer))
        .ok_or("the index layer is gone")?;
    // A feature without geometry (common in CSV and GeoJSON) leaves the frame where it was.
    let e = layer.dataset.features.get(page).and_then(|f| f.geometry.as_ref()).map(|g| g.extent()).filter(|e| !e.is_empty());
    if let Some(e) = e {
        set_frame(s, li, sr.frame, &e.padded(sr.margin, 0.01))?;
    }
    if let Some(sr) = s.project.layouts.get_mut(li).and_then(|l| l.series.as_mut()) {
        sr.page = page;
    }
    let name = series_info(s, li, page).map(|x| x.1).unwrap_or_default();
    Ok(json!({"page": page, "pages": n, "name": name}))
}

/// Renders a layout to PNG or PDF bytes. `all` renders every map series page (PDF only).
pub fn layout_bytes(s: &mut Session, li: usize, pdf: bool, dpi: f64, all: bool) -> Result<Vec<u8>, String> {
    let saved = s.project.layouts.get(li).ok_or("no layout")?.clone();
    let dpi = if dpi.is_finite() { dpi.clamp(36.0, 600.0) } else { 150.0 };
    let current = saved.series.as_ref().map_or(0, |sr| sr.page);
    // Only a whole-series PDF names every page.
    let info = series_info(s, li, current);
    let names = if all && pdf { series_pages(s, li) } else { None };
    let total = info.as_ref().map_or(1, |i| i.0);
    // Every page of a map series (PDF), or just the layout as it is now.
    let pages: Vec<usize> = match (&names, all && pdf) {
        (Some(n), true) => {
            if n.len() > 500 {
                s.notify(format!("The map series has {} pages; the PDF holds the first 500", n.len()), false);
            }
            (0..n.len().min(500)).collect()
        }
        _ => vec![current],
    };
    let stepping = names.is_some() && pages.len() > 1;
    let mut render_pages = || -> Result<(Vec<rustgis_render::pdf::Page>, Option<Vec<u8>>), String> {
        let mut out = Vec::new();
        let mut png = None;
        let mut held = 0usize;
        for page in &pages {
            if stepping {
                series_page(s, &json!({"layout": li, "page": page}))?;
            }
            let layout = s.project.layouts.get(li).ok_or("no layout")?;
            let info = rustgis_render::layout::PageInfo {
                page: page + 1,
                pages: total,
                page_name: match &names {
                    Some(n) => n.get(*page).cloned().unwrap_or_default(),
                    None => info.as_ref().map(|i| i.1.clone()).unwrap_or_default(),
                },
            };
            let img = rustgis_render::layout::render(layout, &mut s.project.maps, dpi, &info).ok_or("the page is too large to draw")?;
            // Each page is compressed as soon as it's drawn, within a memory budget.
            if pdf {
                out.push(rustgis_render::pdf::Page::new(&img, layout.width, layout.height));
                held += out.last().map_or(0, rustgis_render::pdf::Page::len);
                if held > PDF_BUDGET {
                    return Err(format!(
                        "the PDF passed {} MB at page {}; export at a lower dpi, or fewer pages",
                        PDF_BUDGET >> 20,
                        page + 1
                    ));
                }
            } else {
                png = Some(img.encode_png().map_err(|e| e.to_string())?);
            }
        }
        Ok((out, png))
    };
    let result = render_pages();
    // Stepping through the series moved the frame; put the layout back however that went.
    if stepping && let Some(cur) = s.project.layouts.get_mut(li) {
        *cur = saved.clone();
    }
    let (pages, png) = result?;
    if pdf { Ok(rustgis_render::pdf::write(&pages, &saved.name)) } else { png.ok_or_else(|| "nothing to draw".to_string()) }
}

fn export(s: &mut Session, p: &Value) -> CmdResult {
    let path = ps(p, "path").ok_or("path is required")?.to_string();
    let li = index(s, p)?;
    let lower = path.to_ascii_lowercase();
    let pdf = lower.ends_with(".pdf");
    if !pdf && !lower.ends_with(".png") {
        return Err("export to a .png or .pdf file".into());
    }
    let bytes = layout_bytes(s, li, pdf, f(p, "dpi").unwrap_or(150.0), b(p, "allPages").unwrap_or(true))?;
    rustgis_data::io::write_atomic(Path::new(&path), &bytes).map_err(|e| format!("{path}: {e}"))?;
    s.notify(format!("Exported {path}"), false);
    Ok(json!({"path": path, "bytes": bytes.len()}))
}

pub(crate) fn import(s: &mut Session, text: &str) -> CmdResult {
    if text.len() as u64 > MAX_LAYOUT {
        return Err("the layout file is too large".into());
    }
    let mut l: Layout = serde_json::from_str(text).map_err(|e| format!("not a RustGIS layout: {e}"))?;
    l.clamp();
    l.name = unique_name(s, &l.name);
    // Frames showing maps this project lacks show the active map instead.
    let fallback = s.map().name.clone();
    let names: Vec<String> = s.project.maps.iter().map(|m| m.name.to_ascii_lowercase()).collect();
    for e in &mut l.elements {
        if let ElementKind::MapFrame { map, .. } = &mut e.kind
            && !names.contains(&map.to_ascii_lowercase())
        {
            *map = fallback.clone();
        }
    }
    let name = l.name.clone();
    s.project.layouts.push(l);
    let i = s.project.layouts.len() - 1;
    s.active_layout = Some(i);
    Ok(json!({"layout": i, "name": name}))
}
