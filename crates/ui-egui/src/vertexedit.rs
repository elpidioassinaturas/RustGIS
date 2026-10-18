//! The Edit Vertices map tool: the selected features' vertices drawn as handles, with a midpoint
//! handle on each edge. Drag a vertex to move it, drag a midpoint (or click an edge) to insert
//! one, click a vertex and press Delete to remove it. Every change is one `edit.vertex` command,
//! so undo, attribute rules and the edit counts work as for any other edit.

use egui::{Color32, Painter, Pos2, Rect, Stroke, vec2};
use rustgis_engine::data::{Geometry, VertexId};
use rustgis_engine::geom::Coord;
use serde_json::json;

use crate::RustGisApp;
use crate::mapview::{lonlat_to_screen, screen_to_lonlat};

/// How near (points) the pointer must be to a vertex, a midpoint or an edge to take it.
const VERTEX_PX: f32 = 7.0;
const MID_PX: f32 = 6.0;
const EDGE_PX: f32 = 5.0;
/// Most features edited and handles drawn at once (a large selection still draws quickly).
const MAX_FEATURES: usize = 50;
const MAX_HANDLES: usize = 4000;
/// Edges shorter than this (points) get no midpoint handle.
const MID_MIN_PX: f32 = 24.0;

/// A vertex being dragged: the feature, the vertex (for an insertion, the vertex the new one
/// follows) and where the pointer has it now.
#[derive(Clone, Copy, Debug)]
pub struct VertexDrag {
    pub layer: u32,
    pub row: usize,
    pub vertex: VertexId,
    pub insert: bool,
    pub at: Coord,
    /// Whether `at` snapped to a feature (the overlay marks it).
    pub snapped: bool,
}

/// What is under the pointer.
#[derive(Clone, Copy, Debug)]
enum Handle {
    Vertex(u32, usize, VertexId, Coord),
    /// An edge's midpoint handle or a point on the edge: the edge's start and the place.
    Edge(u32, usize, VertexId, Coord),
}

/// The features whose vertices the tool edits: the selected features of visible feature layers
/// held in memory (a layer read in place from its file can't be edited), when editing is on.
fn targets(app: &RustGisApp) -> Vec<(u32, usize, &Geometry)> {
    let m = app.session.map();
    if !m.editing.settings.enabled {
        return Vec::new();
    }
    let mut out = Vec::new();
    for l in m.layers.iter().filter(|l| m.shows(l) && !l.selection.is_empty() && !l.is_raster() && !l.graphics && !l.is_streamed()) {
        for r in &l.selection {
            if out.len() >= MAX_FEATURES {
                return out;
            }
            if let Some(g) = l.dataset.features.get(*r).and_then(|f| f.geometry.as_ref()) {
                out.push((l.id, *r, g));
            }
        }
    }
    out
}

/// The screen points of an edge's ends, when both are in view.
fn edge_on_screen(app: &RustGisApp, rect: Rect, a: Coord, b: Coord) -> Option<(Pos2, Pos2)> {
    Some((lonlat_to_screen(app, rect, a)?, lonlat_to_screen(app, rect, b)?))
}

/// The nearest point to `p` on the segment `a`–`b`.
fn nearest_on(a: Pos2, b: Pos2, p: Pos2) -> Pos2 {
    let d = b - a;
    let len2 = d.length_sq();
    let t = if len2 > 0.0 { ((p - a).dot(d) / len2).clamp(0.0, 1.0) } else { 0.0 };
    a + d * t
}

/// The handle under a screen point: a vertex first, then a midpoint, then an edge.
fn handle_at(app: &RustGisApp, rect: Rect, p: Pos2) -> Option<Handle> {
    // Features nowhere near the pointer aren't searched: their extents (in degrees) padded by
    // the reach of a handle around it.
    let near = screen_to_lonlat(app, rect, p).map(|c| {
        let reach = [vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(-1.0, 0.0), vec2(0.0, -1.0)]
            .iter()
            .filter_map(|d| screen_to_lonlat(app, rect, p + *d * (VERTEX_PX + MID_PX)))
            .map(|q| (q.x - c.x).abs().max((q.y - c.y).abs()))
            .fold(0.0_f64, f64::max);
        (c, reach * 2.0)
    });
    let ts: Vec<_> =
        targets(app).into_iter().filter(|(_, _, g)| near.is_none_or(|(c, reach)| g.extent().padded(0.0, reach).contains(c))).collect();
    let mut best: Option<(f32, Handle)> = None;
    for &(layer, row, g) in &ts {
        g.for_each_vertex(|v, c| {
            let Some(s) = lonlat_to_screen(app, rect, c) else { return };
            let d = (s - p).length();
            if d <= VERTEX_PX && best.is_none_or(|b| d < b.0) {
                best = Some((d, Handle::Vertex(layer, row, v, c)));
            }
        });
    }
    if let Some((_, h)) = best {
        return Some(h);
    }
    for &(layer, row, g) in &ts {
        g.for_each_edge(|v, a, b| {
            let Some((sa, sb)) = edge_on_screen(app, rect, a, b) else { return };
            let mid = sa + (sb - sa) / 2.0;
            let (d, at) = if (sb - sa).length() >= MID_MIN_PX && (mid - p).length() <= MID_PX {
                ((mid - p).length() - MID_PX, mid)
            } else {
                let q = nearest_on(sa, sb, p);
                ((q - p).length(), q)
            };
            if d <= EDGE_PX
                && best.is_none_or(|b| d < b.0)
                && let Some(c) = screen_to_lonlat(app, rect, at)
            {
                best = Some((d, Handle::Edge(layer, row, v, c)));
            }
        });
    }
    best.map(|(_, h)| h)
}

/// A press at `origin` starts dragging the vertex or midpoint there, if there is one.
pub fn drag_start(app: &mut RustGisApp, rect: Rect, origin: Pos2) -> bool {
    let drag = match handle_at(app, rect, origin) {
        Some(Handle::Vertex(layer, row, vertex, at)) => VertexDrag { layer, row, vertex, insert: false, at, snapped: false },
        Some(Handle::Edge(layer, row, vertex, at)) => VertexDrag { layer, row, vertex, insert: true, at, snapped: false },
        None => return false,
    };
    app.map.vertex = (!drag.insert).then_some((drag.layer, drag.row, drag.vertex));
    app.map.vertex_drag = Some(drag);
    true
}

/// The dragged vertex follows the pointer, snapping when snapping is on.
pub fn drag_to(app: &mut RustGisApp, rect: Rect, p: Pos2) {
    let Some(c) = screen_to_lonlat(app, rect, p) else { return };
    let Some(d) = app.map.vertex_drag else { return };
    // Asked directly, not as a command: commands mark the map changed, redrawing it every frame.
    // The feature being changed isn't a snap target: its old vertex and edges would hold the
    // vertex in place.
    let snap = rustgis_engine::cmd::design::snap_except(&mut app.session, c, Some((d.layer, d.row)));
    if let Some(d) = &mut app.map.vertex_drag {
        d.at = snap.map_or(c, |(at, _)| at);
        d.snapped = snap.is_some();
    }
}

/// Letting go commits the move or insertion as one edit.
pub fn drag_stop(app: &mut RustGisApp) {
    let Some(d) = app.map.vertex_drag.take() else { return };
    // A vertex let go where it was is no edit (and no undo step).
    let at_start = targets(app).iter().any(|(l, r, g)| {
        *l == d.layer && *r == d.row && g.vertex(d.vertex).is_some_and(|c| (c.x - d.at.x).abs() < 1e-9 && (c.y - d.at.y).abs() < 1e-9)
    });
    if d.insert || !at_start {
        commit(app, d);
    }
}

fn commit(app: &mut RustGisApp, d: VertexDrag) {
    let action = if d.insert { "insert" } else { "move" };
    let p = json!({
        "layer": d.layer, "row": d.row, "action": action,
        "part": d.vertex.part, "ring": d.vertex.ring, "index": d.vertex.index, "lon": d.at.x, "lat": d.at.y,
    });
    if let Ok(v) = app.run("edit.vertex", p) {
        let num = |k: &str| v.get(k).and_then(serde_json::Value::as_u64).map(|n| n as usize);
        if let (Some(part), Some(ring), Some(index)) = (num("part"), num("ring"), num("index")) {
            app.map.vertex = Some((d.layer, d.row, VertexId::new(part, ring, index)));
        }
    }
}

/// A click chooses the vertex under it, inserts a vertex on the edge under it, or else selects
/// the feature there (Shift adds it to the selection).
pub fn click(app: &mut RustGisApp, rect: Rect, p: Pos2, c: Coord, shift: bool) {
    match handle_at(app, rect, p) {
        Some(Handle::Vertex(layer, row, v, _)) => app.map.vertex = Some((layer, row, v)),
        Some(Handle::Edge(layer, row, vertex, at)) => commit(app, VertexDrag { layer, row, vertex, insert: true, at, snapped: false }),
        None => {
            app.map.vertex = None;
            let mode = if shift { "add" } else { "new" };
            let _ = app.run("select.atPoint", json!({"lon": c.x, "lat": c.y, "tolerancePx": 5, "mode": mode}));
        }
    }
}

/// Delete removes the chosen vertex; `false` when none is chosen.
pub fn delete_chosen(app: &mut RustGisApp) -> bool {
    let Some((layer, row, v)) = app.map.vertex else { return false };
    // A vertex chosen before an undo, or on a feature no longer selected, is gone: Delete then
    // deletes the selected features as usual.
    if !targets(app).iter().any(|(l, r, g)| *l == layer && *r == row && g.vertex(v).is_some()) {
        app.map.vertex = None;
        return false;
    }
    let p = json!({"layer": layer, "row": row, "action": "delete", "part": v.part, "ring": v.ring, "index": v.index});
    if app.run("edit.vertex", p).is_ok() {
        // The vertex before it is chosen next, so Delete can go on removing.
        app.map.vertex = v.index.checked_sub(1).map(|i| (layer, row, VertexId::new(v.part, v.ring, i)));
    }
    true
}

/// The vertices either side of `v` on its path (a ring's first and last are neighbours).
fn neighbours(g: &Geometry, v: VertexId) -> Vec<Coord> {
    let edges: Vec<(VertexId, Coord, Coord)> = g.edges().into_iter().filter(|e| e.0.part == v.part && e.0.ring == v.ring).collect();
    let n = g.vertices().iter().filter(|(w, _)| w.part == v.part && w.ring == v.ring).count();
    // A ring has as many edges as vertices, its last edge ending at the first vertex.
    let prev = match v.index.checked_sub(1) {
        Some(i) => Some(i),
        None => (edges.len() == n).then(|| n.checked_sub(1)).flatten(),
    };
    let next = edges.iter().find(|e| e.0.index == v.index).map(|e| e.2);
    let prev = prev.and_then(|i| edges.iter().find(|e| e.0.index == i)).map(|e| e.1);
    prev.into_iter().chain(next).collect()
}

/// The pointer's cursor over a handle.
pub fn cursor(app: &RustGisApp, rect: Rect, p: Pos2) -> Option<egui::CursorIcon> {
    if app.map.vertex_drag.is_some() {
        return Some(egui::CursorIcon::Grabbing);
    }
    match handle_at(app, rect, p)? {
        Handle::Vertex(..) => Some(egui::CursorIcon::Grab),
        Handle::Edge(..) => Some(egui::CursorIcon::Copy),
    }
}

/// Draws the handles, the chosen vertex and, mid-drag, the edges the vertex pulls.
pub fn draw(app: &RustGisApp, p: &Painter, rect: Rect, accent: Color32) {
    let green = Color32::from_rgb(0x16, 0x9A, 0x5A);
    let drag = app.map.vertex_drag;
    // Handles off the map aren't drawn and don't count towards the cap.
    let shown = rect.expand(8.0);
    let mut handles = 0;
    for (layer, row, g) in targets(app) {
        // The edges a dragged vertex pulls: its neighbours' (or the split edge's ends) to it.
        if let Some(d) = drag.filter(|d| d.layer == layer && d.row == row) {
            let ends = if d.insert {
                g.edges().into_iter().find(|e| e.0 == d.vertex).map(|e| vec![e.1, e.2]).unwrap_or_default()
            } else {
                neighbours(g, d.vertex)
            };
            if let Some(to) = lonlat_to_screen(app, rect, d.at) {
                for e in ends.into_iter().filter_map(|c| lonlat_to_screen(app, rect, c)) {
                    p.line_segment([e, to], Stroke::new(2.0, accent));
                }
                p.circle_filled(to, 5.0, accent);
                p.circle_stroke(to, 5.0, Stroke::new(1.5, Color32::WHITE));
            }
        }
        g.for_each_edge(|v, a, b| {
            if handles >= MAX_HANDLES {
                return;
            }
            let Some((sa, sb)) = edge_on_screen(app, rect, a, b) else { return };
            let mid = sa + (sb - sa) / 2.0;
            if shown.contains(mid)
                && (sb - sa).length() >= MID_MIN_PX
                && !drag.is_some_and(|d| d.insert && d.layer == layer && d.row == row && d.vertex == v)
            {
                p.circle_filled(mid, 3.5, Color32::WHITE.gamma_multiply(0.85));
                p.circle_stroke(mid, 3.5, Stroke::new(1.2, green));
                handles += 1;
            }
        });
        g.for_each_vertex(|v, c| {
            if handles >= MAX_HANDLES {
                return;
            }
            // The dragged vertex is drawn where the pointer has it.
            if drag.is_some_and(|d| !d.insert && d.layer == layer && d.row == row && d.vertex == v) {
                return;
            }
            let Some(s) = lonlat_to_screen(app, rect, c).filter(|s| shown.contains(*s)) else { return };
            let chosen = app.map.vertex == Some((layer, row, v));
            let size = if chosen { 9.0 } else { 7.0 };
            let r = Rect::from_center_size(s, vec2(size, size));
            p.rect_filled(r, 0.0, if chosen { accent } else { Color32::WHITE });
            p.rect_stroke(r, 0.0, Stroke::new(1.3, if chosen { Color32::WHITE } else { green }), egui::StrokeKind::Middle);
            handles += 1;
        });
    }
}
