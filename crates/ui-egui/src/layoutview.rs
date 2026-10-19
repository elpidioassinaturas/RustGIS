//! The layout view: the page with its elements, shown instead of the map when a layout is open.
//! Click an element to select it, drag it to move, drag its handles to resize, Shift-drag a map
//! frame to pan its map; the wheel zooms the page. The properties panel edits the selection.
//! Rulers run along the top and left: drag out of one to add a guide, drag a guide back onto one
//! to remove it. Dragged elements snap to guides, the page and each other (Alt places freely).

use egui::{Align2, Color32, CornerRadius, CursorIcon, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use rustgis_engine::map::layout::{
    ArrowStyle, CoordFormat, ElementKind, Grid, GridStyle, Layout, MAX_COLORBAR_TICKS, MAX_GRID_WIDTH, MAX_LETTER_SPACING,
    MAX_SCALE_DIVISIONS, MAX_TICK_MM, MIN_LETTER_SPACING, Orientation, PT_PER_MM, ScaleBarStyle, ScaleUnits, TextAlign, TickSide,
    frame_camera,
};
use serde_json::{Value, json};

use crate::RustGisApp;
use crate::theme::{Tokens, regular, semibold};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    /// Moving an element: its id and the pointer's page position (mm) at the start.
    Move(u32, (f64, f64)),
    /// Resizing from a handle (0..8 clockwise from top-left).
    Resize(u32, usize, (f64, f64)),
    /// Panning a map frame's map.
    PanFrame(u32, (f64, f64)),
    /// Panning the page view.
    Pan,
    /// Drawing a lasso frame.
    Lasso,
    /// Dragging a new guide out of a ruler: vertical (from the left ruler) or horizontal.
    NewGuide(bool),
    /// Moving a guide: vertical or horizontal, and its index.
    Guide(bool, usize),
}

/// Width of the rulers along the view's top and left edges (points).
const RULER: f32 = 18.0;
/// How near (points) a dragged edge snaps to a line.
const SNAP_PT: f32 = 6.0;
/// How near (points) the pointer picks up a guide.
const GUIDE_PICK: f32 = 4.0;
const GUIDE_COLOR: Color32 = Color32::from_rgb(0x00, 0x9E, 0xD8);
const SNAP_COLOR: Color32 = Color32::from_rgb(0xE0, 0x3E, 0x8C);

/// An element's box: x, y, width, height (mm).
type Box4 = (f64, f64, f64, f64);

#[derive(Default)]
pub struct LayoutView {
    texture: Option<egui::TextureHandle>,
    key: Option<(u64, usize, [usize; 2], i64, i64, i64)>,
    /// Page zoom (1 = fit) and pan, in screen points.
    zoom: f32,
    pan: egui::Vec2,
    pub selected: Option<u32>,
    drag: Option<Drag>,
    /// The page rectangle and points per millimetre the drag last saw.
    drag_page: Option<(Rect, f32)>,
    /// A frame being drawn: "polygon" (clicked) or "lasso" (dragged), and its points (mm).
    pub tool: Option<&'static str>,
    sketch: Vec<(f64, f64)>,
    /// Text being typed into the element properties, by (layout, element, field): kept here so
    /// keystrokes survive the frame, and saved when the field loses focus or stops being shown.
    typing: std::collections::HashMap<(usize, u32, &'static str), String>,
    /// The layout shown, shared between frames until the project changes (layouts can hold
    /// megabytes of pictures, too much to copy every frame), keyed by layout and revision.
    shown: Option<(usize, u64, std::rc::Rc<Layout>)>,
    /// The map series page count and the current page's name, by the same key.
    series: Option<(usize, u64, usize, String)>,
    /// Dragged elements don't snap (they do by default; Alt places freely for one drag).
    pub no_snap: bool,
    /// The rulers are hidden.
    pub no_rulers: bool,
    /// Map frames are navigated rather than moved: dragging one pans its map, the wheel zooms it.
    pub navigate: bool,
    /// A wheel zoom of a map frame shown but not yet applied (one undo step per gesture, not per
    /// wheel step): its layout's name (unique, unlike an index, which shifts when layouts are
    /// removed), the frame, the factor on its scale, the page point (mm) kept under the pointer,
    /// and when the wheel last turned (seconds).
    frame_zoom: Option<(String, u32, f64, (f64, f64), f64)>,
}

/// Seconds after the wheel stops before a map frame's zoom is applied.
const FRAME_ZOOM_SETTLE: f64 = 0.35;

impl LayoutView {
    /// Stops drawing a polygon or lasso frame, or navigating map frames (Esc).
    pub fn cancel_tool(&mut self) {
        self.navigate = false;
        self.tool = None;
        self.sketch.clear();
        if self.drag == Some(Drag::Lasso) {
            self.drag = None;
        }
    }
}

/// Page geometry on screen: the page rectangle and points per millimetre.
fn page_rect(lv: &LayoutView, area: Rect, l: &Layout) -> (Rect, f32) {
    let fit = ((area.width() - 48.0) / l.width as f32).min((area.height() - 48.0) / l.height as f32).max(0.05);
    let s = fit * if lv.zoom > 0.0 { lv.zoom } else { 1.0 };
    let size = vec2(l.width as f32 * s, l.height as f32 * s);
    (Rect::from_center_size(area.center() + lv.pan, size), s)
}

fn to_mm(page: Rect, s: f32, p: Pos2) -> (f64, f64) {
    (((p.x - page.min.x) / s) as f64, ((p.y - page.min.y) / s) as f64)
}

fn to_screen(page: Rect, s: f32, (x, y): (f64, f64)) -> Pos2 {
    pos2(page.min.x + x as f32 * s, page.min.y + y as f32 * s)
}

fn handles(r: Rect) -> [Pos2; 8] {
    let (c, m) = (r.center(), r);
    [
        m.left_top(),
        pos2(c.x, m.top()),
        m.right_top(),
        pos2(m.right(), c.y),
        m.right_bottom(),
        pos2(c.x, m.bottom()),
        m.left_bottom(),
        pos2(m.left(), c.y),
    ]
}

/// A rectangle after dragging handle `h` by (dx, dy) mm. A side the handle doesn't move keeps its
/// size, so widening a hairline rule leaves it a hairline.
fn resized((x, y, w, h): (f64, f64, f64, f64), handle: usize, dx: f64, dy: f64) -> (f64, f64, f64, f64) {
    let (mut x0, mut y0, mut x1, mut y1) = (x, y, x + w, y + h);
    match handle {
        0 => (x0, y0) = (x0 + dx, y0 + dy),
        1 => y0 += dy,
        2 => (x1, y0) = (x1 + dx, y0 + dy),
        3 => x1 += dx,
        4 => (x1, y1) = (x1 + dx, y1 + dy),
        5 => y1 += dy,
        6 => (x0, y1) = (x0 + dx, y1 + dy),
        _ => x0 += dx,
    }
    let (sizes_x, sizes_y) = (!matches!(handle, 1 | 5), !matches!(handle, 3 | 7));
    let w = if sizes_x { (x1 - x0).abs().max(2.0) } else { w };
    let h = if sizes_y { (y1 - y0).abs().max(2.0) } else { h };
    (x0.min(x1), y0.min(y1), w, h)
}

/// The lines a dragged element snaps to (mm): the page's edges and centre, the guides, and the
/// other visible elements' edges and centres; vertical lines (x), then horizontal (y).
fn snap_lines(l: &Layout, moving: u32) -> (Vec<f64>, Vec<f64>) {
    let mut xs = vec![0.0, l.width / 2.0, l.width];
    let mut ys = vec![0.0, l.height / 2.0, l.height];
    xs.extend(&l.guides.x);
    ys.extend(&l.guides.y);
    for e in l.elements.iter().filter(|e| e.id != moving && e.visible) {
        xs.extend([e.x, e.x + e.w / 2.0, e.x + e.w]);
        ys.extend([e.y, e.y + e.h / 2.0, e.y + e.h]);
    }
    (xs, ys)
}

/// The smallest shift that puts one of `edges` on one of `lines`, if one is within `tol`, and
/// the line.
fn snap_to(edges: &[f64], lines: &[f64], tol: f64) -> Option<(f64, f64)> {
    edges
        .iter()
        .flat_map(|e| lines.iter().map(move |l| (l - e, *l)))
        .filter(|(d, _)| d.abs() <= tol)
        .min_by(|a, b| a.0.abs().total_cmp(&b.0.abs()))
}

/// Where an element lands when moved (no handle) or resized from a handle by the pointer's
/// offset (mm): snapped to `lines` within `tol` mm when given, with the lines it snapped to.
fn landing(
    r: Box4,
    handle: Option<usize>,
    (mut dx, mut dy): (f64, f64),
    snap: Option<(&(Vec<f64>, Vec<f64>), f64)>,
) -> (Box4, Option<f64>, Option<f64>) {
    let (x, y, w, h) = r;
    let (mut sx, mut sy) = (None, None);
    if let Some(((xs, ys), tol)) = snap {
        // The edges that move: every edge and the centre for a move, the handle's for a resize.
        let (ex, ey) = match handle {
            None => (vec![x, x + w / 2.0, x + w], vec![y, y + h / 2.0, y + h]),
            Some(hd) => (
                match hd {
                    0 | 6 | 7 => vec![x],
                    2..=4 => vec![x + w],
                    _ => vec![],
                },
                match hd {
                    0..=2 => vec![y],
                    4..=6 => vec![y + h],
                    _ => vec![],
                },
            ),
        };
        let ex: Vec<f64> = ex.iter().map(|e| e + dx).collect();
        let ey: Vec<f64> = ey.iter().map(|e| e + dy).collect();
        if let Some((d, line)) = snap_to(&ex, xs, tol) {
            dx += d;
            sx = Some(line);
        }
        if let Some((d, line)) = snap_to(&ey, ys, tol) {
            dy += d;
            sy = Some(line);
        }
    }
    let out = match handle {
        Some(hd) => resized(r, hd, dx, dy),
        None => (x + dx, y + dy, w, h),
    };
    (out, sx, sy)
}

/// The element being moved or resized, where it lands with the pointer at `at` (mm), and the
/// lines it snapped to.
fn drag_landing(lv: &LayoutView, ui: &Ui, layout: &Layout, s: f32, at: (f64, f64)) -> Option<(u32, Box4, Option<f64>, Option<f64>)> {
    let (id, handle, start) = match lv.drag? {
        Drag::Move(id, start) => (id, None, start),
        Drag::Resize(id, h, start) => (id, Some(h), start),
        _ => return None,
    };
    let e = layout.element(id)?;
    let free = lv.no_snap || ui.input(|i| i.modifiers.alt);
    let lines = (!free).then(|| snap_lines(layout, id));
    let tol = f64::from(SNAP_PT / s.max(1e-3));
    let (r, sx, sy) = landing((e.x, e.y, e.w, e.h), handle, (at.0 - start.0, at.1 - start.1), lines.as_ref().map(|l| (l, tol)));
    Some((id, r, sx, sy))
}

/// The guide under a screen point: vertical or not, and its index.
fn guide_at(l: &Layout, page: Rect, s: f32, pos: Pos2) -> Option<(bool, usize)> {
    let near = |d: f32| d.abs() < GUIDE_PICK;
    l.guides
        .x
        .iter()
        .position(|x| near(to_screen(page, s, (*x, 0.0)).x - pos.x))
        .map(|i| (true, i))
        .or_else(|| l.guides.y.iter().position(|y| near(to_screen(page, s, (0.0, *y)).y - pos.y)).map(|i| (false, i)))
}

/// Which ruler a screen point is on: Some(true) the left one (vertical guides), Some(false) the top.
fn ruler_at(lv: &LayoutView, area: Rect, p: Pos2) -> Option<bool> {
    if lv.no_rulers || !area.contains(p) {
        return None;
    }
    let (left, top) = (p.x < area.left() + RULER, p.y < area.top() + RULER);
    match (left, top) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// Puts down a guide dragged out of a ruler or moved: dropped on a ruler or off the view, it's
/// removed.
#[allow(clippy::too_many_arguments)]
fn drop_guide(
    app: &mut RustGisApp,
    layout: &Layout,
    li: usize,
    page: Rect,
    s: f32,
    area: Rect,
    vertical: bool,
    moving: Option<usize>,
    at: Option<Pos2>,
) {
    let mut list = if vertical { layout.guides.x.clone() } else { layout.guides.y.clone() };
    if let Some(i) = moving.filter(|i| *i < list.len()) {
        list.remove(i);
    }
    if let Some(p) = at.filter(|p| area.contains(*p) && (app.layout.no_rulers || (p.x >= area.left() + RULER && p.y >= area.top() + RULER)))
    {
        let (mx, my) = to_mm(page, s, p);
        // To a tenth of a millimetre.
        list.push(((if vertical { mx } else { my }) * 10.0).round() / 10.0);
    }
    let key = if vertical { "x" } else { "y" };
    let _ = app.run("layout.guides", json!({"layout": li, key: list}));
}

/// Rulers along the view's top and left edges, in millimetres from the page's top-left corner,
/// with the guides and the pointer marked.
fn rulers(painter: &egui::Painter, area: Rect, page: Rect, s: f32, l: &Layout, pointer: Option<Pos2>, t: &Tokens) {
    const STEPS: [f64; 10] = [1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0];
    let sf = f64::from(s);
    let minor = STEPS.iter().copied().find(|st| st * sf >= 5.0).unwrap_or(1000.0);
    let major = STEPS.iter().copied().find(|st| st * sf >= 45.0 && (st / minor).fract() == 0.0).unwrap_or(1000.0);
    let top = Rect::from_min_max(area.min, pos2(area.max.x, area.min.y + RULER));
    let left = Rect::from_min_max(area.min, pos2(area.min.x + RULER, area.max.y));
    painter.rect_filled(top, 0.0, t.ribbon);
    painter.rect_filled(left, 0.0, t.ribbon);
    // The page's span, lighter.
    let span_x = Rect::from_min_max(pos2(page.min.x.max(left.max.x), top.min.y), pos2(page.max.x.min(top.max.x), top.max.y));
    let span_y = Rect::from_min_max(pos2(left.min.x, page.min.y.max(top.max.y)), pos2(left.max.x, page.max.y.min(left.max.y)));
    for r in [span_x, span_y] {
        if r.is_positive() {
            painter.rect_filled(r, 0.0, t.input);
        }
    }
    let tick = Stroke::new(1.0, t.text_dim);
    let is_major = |v: f64| ((v / major) - (v / major).round()).abs() < 1e-6;
    // Along the top, then down the side; at most a few thousand ticks either way.
    let (x0, y0) = to_mm(page, s, pos2(left.max.x, top.max.y));
    let (x1, y1) = to_mm(page, s, area.max);
    for k in ((x0 / minor).ceil() as i64..=(x1 / minor).floor() as i64).take(4000) {
        let v = k as f64 * minor;
        let x = to_screen(page, s, (v, 0.0)).x;
        let len = if is_major(v) { RULER * 0.6 } else { RULER * 0.25 };
        painter.line_segment([pos2(x, top.max.y - len), pos2(x, top.max.y)], tick);
        if is_major(v) {
            painter.text(pos2(x + 2.0, top.min.y + 1.0), Align2::LEFT_TOP, format!("{v:.0}"), regular(9.0), t.text_dim);
        }
    }
    for k in ((y0 / minor).ceil() as i64..=(y1 / minor).floor() as i64).take(4000) {
        let v = k as f64 * minor;
        let y = to_screen(page, s, (0.0, v)).y;
        let len = if is_major(v) { RULER * 0.6 } else { RULER * 0.25 };
        painter.line_segment([pos2(left.max.x - len, y), pos2(left.max.x, y)], tick);
        if is_major(v) {
            painter.text(pos2(left.min.x + 1.0, y + 1.0), Align2::LEFT_TOP, format!("{v:.0}"), regular(9.0), t.text_dim);
        }
    }
    // The guides, as small triangles pointing into the page.
    for x in &l.guides.x {
        let x = to_screen(page, s, (*x, 0.0)).x;
        if x > left.max.x {
            let pts = vec![pos2(x - 4.0, top.max.y - 6.0), pos2(x + 4.0, top.max.y - 6.0), pos2(x, top.max.y)];
            painter.add(egui::Shape::convex_polygon(pts, GUIDE_COLOR, Stroke::NONE));
        }
    }
    for y in &l.guides.y {
        let y = to_screen(page, s, (0.0, *y)).y;
        if y > top.max.y {
            let pts = vec![pos2(left.max.x - 6.0, y - 4.0), pos2(left.max.x - 6.0, y + 4.0), pos2(left.max.x, y)];
            painter.add(egui::Shape::convex_polygon(pts, GUIDE_COLOR, Stroke::NONE));
        }
    }
    // Where the pointer is.
    if let Some(p) = pointer.filter(|p| area.contains(*p)) {
        let mark = Stroke::new(1.0, t.accent);
        if p.x > left.max.x {
            painter.line_segment([pos2(p.x, top.min.y), pos2(p.x, top.max.y)], mark);
        }
        if p.y > top.max.y {
            painter.line_segment([pos2(left.min.x, p.y), pos2(left.max.x, p.y)], mark);
        }
    }
    painter.rect_filled(Rect::from_min_size(area.min, vec2(RULER, RULER)), 0.0, t.ribbon);
    painter.line_segment([pos2(left.max.x, top.max.y), pos2(top.max.x, top.max.y)], Stroke::new(1.0, t.border));
    painter.line_segment([pos2(left.max.x, top.max.y), pos2(left.max.x, left.max.y)], Stroke::new(1.0, t.border));
}

/// The layout to show, copied from the project only when it has changed since the last copy.
fn shown_layout(app: &mut RustGisApp, li: usize) -> Option<std::rc::Rc<Layout>> {
    let rev = app.session.revision;
    match &app.layout.shown {
        Some((i, r, l)) if *i == li && *r == rev => Some(l.clone()),
        _ => {
            let l = std::rc::Rc::new(app.session.project.layouts.get(li).cloned()?);
            app.layout.shown = Some((li, rev, l.clone()));
            Some(l)
        }
    }
}

pub fn show(app: &mut RustGisApp, ui: &mut Ui) {
    let Some(li) = app.session.active_layout else { return };
    let Some(layout) = shown_layout(app, li) else { return };
    let t = Tokens::get(ui.ctx());
    if app.layout.selected.is_some_and(|id| layout.element(id).is_none()) {
        app.layout.selected = None;
    }
    save_drafts(app, li);
    if app.layout.selected.is_some() {
        egui::Panel::right("layout_props")
            .resizable(false)
            .exact_size(270.0)
            .show_separator_line(true)
            .show(ui, |ui| egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| properties(app, ui, &layout, li)));
    }
    let area = ui.available_rect_before_wrap();
    // A stable id: dragging an element selects it and opens the properties beside the page,
    // and an automatic id would change with them, losing the drag before it is dropped (#325).
    ui.advance_cursor_after_rect(area);
    let resp = ui.interact(area, ui.id().with("layout_page"), Sense::click_and_drag());
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 0.0, if t.dark { Color32::from_gray(38) } else { Color32::from_rgb(0xD9, 0xDD, 0xE3) });
    let (page, s) = page_rect(&app.layout, area, &layout);
    interact(app, ui, &resp, &layout, li, page, s);
    // The properties or a drag may have just changed the layout: draw the page as it is now,
    // or the texture would be keyed to the new revision but show the old layout (#325).
    let Some(layout) = shown_layout(app, li) else { return };
    let (page, s) = page_rect(&app.layout, area, &layout);

    // Render the page at screen resolution when it changes.
    let ppp = ui.ctx().pixels_per_point();
    let px_per_mm = s * ppp;
    // The texture can't be larger than the GPU allows: zoomed far in, the page draws coarser.
    let max_side = ui.ctx().input(|i| i.max_texture_side) as f64;
    let dpi = (px_per_mm as f64 * 25.4).min(max_side / layout.width.max(layout.height) * 25.4).clamp(20.0, 300.0);
    let size = [(page.width() * ppp) as usize, (page.height() * ppp) as usize];
    let key = (app.session.revision, li, size, (dpi * 10.0) as i64, layout.width as i64, layout.height as i64);
    if app.layout.key != Some(key) && size[0] > 2 && size[1] > 2 {
        let info = page_info(app, li);
        if let Some(pm) = rustgis_engine::render::layout::render(&layout, &mut app.session.project.maps, dpi, &info) {
            let img = egui::ColorImage::from_rgba_premultiplied([pm.width() as usize, pm.height() as usize], pm.data());
            match &mut app.layout.texture {
                Some(tex) => tex.set(img, egui::TextureOptions::LINEAR),
                None => app.layout.texture = Some(ui.ctx().load_texture("layout", img, egui::TextureOptions::LINEAR)),
            }
            app.layout.key = Some(key);
        }
    }
    // Shadow, page, elements.
    painter.rect_filled(page.translate(vec2(3.0, 4.0)), 2.0, Color32::from_black_alpha(50));
    painter.rect_filled(page, 0.0, Color32::WHITE);
    let pointer = ui.input(|i| i.pointer.latest_pos());
    if let Some(tex) = &app.layout.texture {
        painter.image(tex.id(), page, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        // A map frame being panned or zoomed: its part of the page drawn shifted or scaled at
        // once, until the change is applied and the page redrawn.
        let preview = match (app.layout.drag, pointer) {
            (Some(Drag::PanFrame(id, start)), Some(p)) => {
                let (mx, my) = to_mm(page, s, p);
                Some((id, 1.0, (mx - start.0, my - start.1), (0.0, 0.0)))
            }
            _ => app.layout.frame_zoom.as_ref().filter(|z| z.0 == layout.name).map(|z| (z.1, z.2, (0.0, 0.0), z.3)),
        };
        if let Some((id, k, (dx, dy), (ax, ay))) = preview
            && let Some(e) = layout.element(id)
        {
            let frame = Rect::from_min_max(to_screen(page, s, (e.x, e.y)), to_screen(page, s, (e.x + e.w, e.y + e.h)));
            // The page region the frame now shows: moved back by the pan, scaled about the anchor.
            let src = |(x, y): (f64, f64)| {
                let (u, v) = (ax + (x - ax) * k - dx, ay + (y - ay) * k - dy);
                pos2((u / layout.width) as f32, (v / layout.height) as f32)
            };
            let uv = Rect::from_min_max(src((e.x, e.y)), src((e.x + e.w, e.y + e.h)));
            let clip = painter.with_clip_rect(frame);
            clip.rect_filled(frame, 0.0, Color32::WHITE);
            clip.image(tex.id(), frame, uv, Color32::WHITE);
        }
    }
    let accent = t.accent;
    // Guides across the view; one being moved is drawn at the pointer instead.
    let moving = match app.layout.drag {
        Some(Drag::Guide(v, i)) => Some((v, i)),
        _ => None,
    };
    let vline = |x: f32, c: Color32| painter.line_segment([pos2(x, area.top()), pos2(x, area.bottom())], Stroke::new(1.0, c));
    let hline = |y: f32, c: Color32| painter.line_segment([pos2(area.left(), y), pos2(area.right(), y)], Stroke::new(1.0, c));
    for (_, x) in layout.guides.x.iter().enumerate().filter(|(i, _)| moving != Some((true, *i))) {
        vline(to_screen(page, s, (*x, 0.0)).x, GUIDE_COLOR);
    }
    for (_, y) in layout.guides.y.iter().enumerate().filter(|(i, _)| moving != Some((false, *i))) {
        hline(to_screen(page, s, (0.0, *y)).y, GUIDE_COLOR);
    }
    if let (Some(Drag::NewGuide(v) | Drag::Guide(v, _)), Some(p)) = (app.layout.drag, pointer) {
        let (mx, my) = to_mm(page, s, p);
        let label = if v {
            vline(p.x, GUIDE_COLOR);
            format!("{mx:.1} mm")
        } else {
            hline(p.y, GUIDE_COLOR);
            format!("{my:.1} mm")
        };
        painter.text(p + vec2(8.0, 8.0), Align2::LEFT_TOP, label, regular(11.0), GUIDE_COLOR);
    }
    // The dragged element's outline where it will land, and the lines it snapped to.
    if let Some(p) = pointer
        && let Some((_, r, sx, sy)) = drag_landing(&app.layout, ui, &layout, s, to_mm(page, s, p))
    {
        if let Some(x) = sx {
            vline(to_screen(page, s, (x, 0.0)).x, SNAP_COLOR);
        }
        if let Some(y) = sy {
            hline(to_screen(page, s, (0.0, y)).y, SNAP_COLOR);
        }
        let rr = Rect::from_min_max(to_screen(page, s, (r.0, r.1)), to_screen(page, s, (r.0 + r.2, r.1 + r.3)));
        painter.rect_stroke(rr, 0.0, Stroke::new(1.5, accent), egui::StrokeKind::Middle);
    }
    if let Some(e) = app.layout.selected.and_then(|id| layout.element(id)) {
        let r = Rect::from_min_max(to_screen(page, s, (e.x, e.y)), to_screen(page, s, (e.x + e.w, e.y + e.h)));
        painter.extend(egui::Shape::dashed_line(
            &[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()],
            Stroke::new(1.2, accent),
            5.0,
            3.0,
        ));
        for h in handles(r) {
            painter.rect_filled(Rect::from_center_size(h, vec2(7.0, 7.0)), 1.0, Color32::WHITE);
            painter.rect_stroke(Rect::from_center_size(h, vec2(7.0, 7.0)), 1.0, Stroke::new(1.2, accent), egui::StrokeKind::Middle);
        }
        painter.text(r.left_top() + vec2(0.0, -4.0), Align2::LEFT_BOTTOM, &e.name, regular(11.0), accent);
    }
    // A frame being drawn.
    if app.layout.tool.is_some() && !app.layout.sketch.is_empty() {
        let mut pts: Vec<Pos2> = app.layout.sketch.iter().map(|p| to_screen(page, s, *p)).collect();
        if let (Some("polygon"), Some(p)) = (app.layout.tool, pointer) {
            pts.push(p);
        }
        painter.add(egui::Shape::closed_line(pts, Stroke::new(2.0, Color32::from_rgb(0x16, 0x9A, 0x5A))));
    }
    // The rulers, and room for them beside the notes drawn over the view.
    let inset = if app.layout.no_rulers {
        0.0
    } else {
        rulers(&painter, area, page, s, &layout, pointer, &t);
        RULER
    };
    // Status: page size, zoom and hint.
    let hint = match app.layout.tool {
        Some("polygon") => tl!("Click the frame's corners · double-click to finish · Esc to stop").to_string(),
        Some(_) => tl!("Drag the frame's outline · Esc to stop").to_string(),
        None if app.layout.navigate => tl!("Drag a map frame to pan its map, wheel to zoom it · Esc to stop").to_string(),
        None => crate::i18n::fmt(
            "{w} × {h} mm · {zoom}% · drag to move (Alt: no snapping), Shift-drag a map frame to pan it, wheel to zoom",
            &[
                ("w", &format!("{:.0}", layout.width)),
                ("h", &format!("{:.0}", layout.height)),
                ("zoom", &format!("{:.0}", (if app.layout.zoom > 0.0 { app.layout.zoom } else { 1.0 }) * 100.0)),
            ],
        ),
    };
    let g = painter.layout_no_wrap(hint, regular(11.5), t.text_dim);
    let bg = Rect::from_min_size(pos2(area.min.x + inset + 10.0, area.max.y - g.size().y - 14.0), g.size() + vec2(14.0, 8.0));
    painter.rect_filled(bg, CornerRadius::same(10), t.ribbon.gamma_multiply(0.9));
    painter.galley(bg.min + vec2(7.0, 4.0), g, t.text_dim);
    if layout.series.is_some()
        && let Some((total, name)) = series_label(app, li)
    {
        let page_no = layout.series.as_ref().map_or(0, |s| s.page);
        let label = crate::i18n::fmt(
            "Page {n} of {total}: {name}",
            &[("n", &(page_no + 1).to_string()), ("total", &total.to_string()), ("name", &name)],
        );
        let g = painter.layout_no_wrap(label, semibold(12.0), t.text);
        let bg = Rect::from_min_size(pos2(area.min.x + inset + 10.0, area.min.y + inset + 10.0), g.size() + vec2(16.0, 10.0));
        painter.rect_filled(bg, CornerRadius::same(10), t.ribbon);
        painter.galley(bg.min + vec2(8.0, 5.0), g, t.text);
    }
}

/// The map series page count and the current page's name, worked out once per project change.
fn series_label(app: &mut RustGisApp, li: usize) -> Option<(usize, String)> {
    let rev = app.session.revision;
    if let Some((i, r, n, name)) = &app.layout.series
        && *i == li
        && *r == rev
    {
        return Some((*n, name.clone()));
    }
    let page = app.session.project.layouts.get(li).and_then(|l| l.series.as_ref()).map_or(0, |s| s.page);
    let (n, name) = rustgis_engine::cmd::layout::series_info(&app.session, li, page)?;
    app.layout.series = Some((li, rev, n, name.clone()));
    Some((n, name))
}

/// Page values for dynamic text in the view.
pub fn page_info(app: &mut RustGisApp, li: usize) -> rustgis_engine::render::layout::PageInfo {
    let page = app.session.project.layouts.get(li).and_then(|l| l.series.as_ref()).map_or(0, |s| s.page);
    let (pages, page_name) = series_label(app, li).unwrap_or((1, String::new()));
    rustgis_engine::render::layout::PageInfo { page: page + 1, pages, page_name }
}

/// The map frame under a screen point, when map frames are being navigated.
fn nav_frame(app: &RustGisApp, layout: &Layout, page: Rect, s: f32, p: Pos2) -> Option<u32> {
    let (x, y) = to_mm(page, s, p);
    layout.hit(x, y).filter(|id| app.layout.navigate && matches!(layout.element(*id).map(|e| &e.kind), Some(ElementKind::MapFrame { .. })))
}

fn interact(app: &mut RustGisApp, ui: &mut Ui, resp: &egui::Response, layout: &Layout, li: usize, page: Rect, s: f32) {
    // A map frame's wheel zoom is applied once the wheel has stopped.
    let now = ui.input(|i| i.time);
    // A pan ending this frame is worked out from `layout` as it was: applying the zoom first
    // would have the pan overwrite its centre. It waits a frame, for the layout after the pan.
    let pan_ends = resp.drag_stopped() && matches!(app.layout.drag, Some(Drag::PanFrame(..)));
    if let Some((name, id, k, anchor, last)) = app.layout.frame_zoom.clone() {
        if name != layout.name {
            // Another layout is open now: the zoom was for a frame of that one.
            app.layout.frame_zoom = None;
        } else if now - last >= FRAME_ZOOM_SETTLE && !pan_ends {
            app.layout.frame_zoom = None;
            zoom_frame(app, layout, li, id, k, anchor);
        } else {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(FRAME_ZOOM_SETTLE));
        }
    }
    // Wheel zooms the page about the pointer, or a map frame's map when navigating.
    if let Some(p) = resp.hover_pos() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.zoom_delta().ln() * 300.0);
        if scroll.abs() > 0.1
            && let Some(id) = nav_frame(app, layout, page, s, p)
        {
            // Wheel up zooms in: a smaller scale denominator.
            let step = f64::from(-scroll * 0.0028).exp();
            let same = app.layout.frame_zoom.as_ref().filter(|z| z.1 == id && z.0 == layout.name).map(|z| z.2);
            let k = match same {
                Some(k) => k * step,
                None => {
                    // Another frame's zoom still waiting is applied, not lost, before this one's.
                    if let Some((name, zid, zk, anchor, _)) = app.layout.frame_zoom.take()
                        && name == layout.name
                    {
                        zoom_frame(app, layout, li, zid, zk, anchor);
                    }
                    step
                }
            };
            app.layout.frame_zoom = Some((layout.name.clone(), id, k.clamp(1e-3, 1e3), to_mm(page, s, p), now));
        } else if scroll.abs() > 0.1 {
            let old = if app.layout.zoom > 0.0 { app.layout.zoom } else { 1.0 };
            let new = (old * (scroll * 0.0028).exp()).clamp(0.2, 12.0);
            // Keep the page point under the pointer fixed.
            let centre = resp.rect.center() + app.layout.pan;
            app.layout.pan += (p - centre) * (1.0 - new / old);
            app.layout.zoom = new;
        }
    }
    // The page moves under a drag when the properties open beside it (the drag selects the
    // element): keep the drag's start under the same screen point, so the element follows the
    // pointer rather than the page.
    if let Some((old, old_s)) = app.layout.drag_page
        && (old, old_s) != (page, s)
    {
        let rebase = |p| to_mm(page, s, to_screen(old, old_s, p));
        app.layout.drag = match app.layout.drag {
            Some(Drag::Move(id, p)) => Some(Drag::Move(id, rebase(p))),
            Some(Drag::Resize(id, h, p)) => Some(Drag::Resize(id, h, rebase(p))),
            Some(Drag::PanFrame(id, p)) => Some(Drag::PanFrame(id, rebase(p))),
            d => d,
        };
    }
    app.layout.drag_page = Some((page, s));
    let pointer_mm = resp.interact_pointer_pos().map(|p| to_mm(page, s, p));
    // Drawing a polygon or lasso frame.
    if let Some(tool) = app.layout.tool {
        if tool == "polygon" {
            if resp.double_clicked() {
                finish_frame(app, li, "polygon");
            } else if resp.clicked()
                && let Some(p) = pointer_mm
            {
                app.layout.sketch.push(p);
            }
        } else {
            if resp.drag_started() {
                app.layout.sketch.clear();
                app.layout.drag = Some(Drag::Lasso);
            }
            if app.layout.drag == Some(Drag::Lasso)
                && let Some(p) = pointer_mm
                && app.layout.sketch.last().is_none_or(|l| ((l.0 - p.0).powi(2) + (l.1 - p.1).powi(2)).sqrt() > 1.0)
            {
                app.layout.sketch.push(p);
            }
            if resp.drag_stopped() {
                app.layout.drag = None;
                finish_frame(app, li, "lasso");
            }
        }
        return;
    }
    // Rulers and guides show they can be dragged.
    if let Some(p) = resp.hover_pos() {
        let vertical = match app.layout.drag {
            Some(Drag::NewGuide(v) | Drag::Guide(v, _)) => Some(v),
            Some(_) => None,
            None => ruler_at(&app.layout, resp.rect, p).or_else(|| guide_at(layout, page, s, p).map(|g| g.0)),
        };
        match vertical {
            Some(true) => ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal),
            Some(false) => ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical),
            None if matches!(app.layout.drag, Some(Drag::PanFrame(..))) => ui.ctx().set_cursor_icon(CursorIcon::Grabbing),
            None if app.layout.drag.is_none() && nav_frame(app, layout, page, s, p).is_some() => ui.ctx().set_cursor_icon(CursorIcon::Grab),
            None => {}
        }
    }
    if resp.clicked()
        && let Some(p) = pointer_mm
        && resp.interact_pointer_pos().is_none_or(|pos| ruler_at(&app.layout, resp.rect, pos).is_none())
    {
        app.layout.selected = layout.hit(p.0, p.1);
    }
    if resp.double_clicked()
        && let Some(id) = app.layout.selected
        && matches!(layout.element(id).map(|e| &e.kind), Some(ElementKind::MapFrame { .. }))
    {
        // Double-clicking a map frame zooms it to the map view.
        let _ = app.run("layout.frameZoom", json!({"layout": li, "element": id, "to": "view"}));
    }
    if resp.drag_started()
        && let (Some(p), Some(pos)) = (pointer_mm, resp.interact_pointer_pos())
    {
        // Navigating, a map frame pans as Shift-dragging one does; its handles stay put.
        let shift = ui.input(|i| i.modifiers.shift) || app.layout.navigate;
        let sel = app.layout.selected.and_then(|id| layout.element(id)).filter(|_| nav_frame(app, layout, page, s, pos).is_none());
        let handle = sel.and_then(|e| {
            let r = Rect::from_min_max(to_screen(page, s, (e.x, e.y)), to_screen(page, s, (e.x + e.w, e.y + e.h)));
            handles(r).iter().position(|h| (*h - pos).length() < 7.0).map(|h| (e.id, h))
        });
        // Where the press was: a drag starts only once the pointer has moved off it.
        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
        let ruler = ruler_at(&app.layout, resp.rect, origin);
        let guide = guide_at(layout, page, s, origin);
        app.layout.drag = Some(match (ruler, handle, guide, layout.hit(p.0, p.1)) {
            (Some(vertical), ..) => Drag::NewGuide(vertical),
            (None, Some((id, h)), ..) => Drag::Resize(id, h, p),
            (None, None, Some((vertical, i)), _) => Drag::Guide(vertical, i),
            (None, None, None, Some(id)) if shift && matches!(layout.element(id).map(|e| &e.kind), Some(ElementKind::MapFrame { .. })) => {
                app.layout.selected = Some(id);
                Drag::PanFrame(id, p)
            }
            (None, None, None, Some(id)) => {
                app.layout.selected = Some(id);
                Drag::Move(id, p)
            }
            (None, None, None, None) => Drag::Pan,
        });
    }
    if app.layout.drag == Some(Drag::Pan) {
        app.layout.pan += resp.drag_delta();
    }
    if resp.drag_stopped() {
        let end = pointer_mm.or_else(|| ui.input(|i| i.pointer.latest_pos()).map(|p| to_mm(page, s, p)));
        match (app.layout.drag, end) {
            (Some(Drag::Move(_, start)), Some(end)) => {
                // A press that hardly moved is a click, not a move.
                if ((end.0 - start.0).abs() > 0.2 || (end.1 - start.1).abs() > 0.2)
                    && let Some((id, (x, y, _, _), _, _)) = drag_landing(&app.layout, ui, layout, s, end)
                {
                    let _ = app.run("layout.update", json!({"layout": li, "element": id, "x": x, "y": y}));
                }
            }
            (Some(Drag::Resize(..)), Some(end)) => {
                if let Some((id, (x, y, w, hh), _, _)) = drag_landing(&app.layout, ui, layout, s, end) {
                    let _ = app.run("layout.update", json!({"layout": li, "element": id, "x": x, "y": y, "w": w, "h": hh}));
                }
            }
            (Some(Drag::NewGuide(v)), _) => {
                let at = ui.input(|i| i.pointer.latest_pos());
                drop_guide(app, layout, li, page, s, resp.rect, v, None, at);
            }
            (Some(Drag::Guide(v, i)), _) => {
                let at = ui.input(|i| i.pointer.latest_pos());
                drop_guide(app, layout, li, page, s, resp.rect, v, Some(i), at);
            }
            (Some(Drag::PanFrame(id, start)), Some(end)) => pan_frame(app, layout, li, id, end.0 - start.0, end.1 - start.1),
            _ => {}
        }
        app.layout.drag = None;
    }
}

/// Moves a map frame's map by a page offset (mm).
fn pan_frame(app: &mut RustGisApp, layout: &Layout, li: usize, id: u32, dx: f64, dy: f64) {
    let Some(e) = layout.element(id) else { return };
    let ElementKind::MapFrame { map, center, scale, bearing, pitch, .. } = &e.kind else { return };
    let Some(m) = app.session.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(map)) else { return };
    let mut cam = frame_camera(m, *center, *scale, *bearing, *pitch);
    // Dragged at the frame's centre, so a turned or tilted frame's map follows the pointer.
    let (w, h) = (e.w * PT_PER_MM, e.h * PT_PER_MM);
    cam.drag_px((w / 2.0, h / 2.0), (w / 2.0 + dx * PT_PER_MM, h / 2.0 + dy * PT_PER_MM), w, h);
    if let Some(ll) = m.to_lonlat(cam.center) {
        let _ = app.run("layout.update", json!({"layout": li, "element": id, "center": [ll.x, ll.y]}));
    }
}

/// Zooms a map frame's map by `k` on its scale (below 1 zooms in), keeping the map under the
/// page point `anchor` (mm) where it is.
fn zoom_frame(app: &mut RustGisApp, layout: &Layout, li: usize, id: u32, k: f64, (ax, ay): (f64, f64)) {
    let Some(e) = layout.element(id) else { return };
    let ElementKind::MapFrame { map, center, scale, bearing, pitch, .. } = &e.kind else { return };
    let Some(m) = app.session.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(map)) else { return };
    let cam = frame_camera(m, *center, *scale, *bearing, *pitch);
    let new_scale = (scale * k).clamp(100.0, 1e10);
    // A turned or tilted frame zooms as the map view does, about the anchor.
    if cam.bearing != 0.0 || cam.pitch != 0.0 {
        let (w, h) = (e.w * PT_PER_MM, e.h * PT_PER_MM);
        let mut c = cam;
        c.zoom_at(*scale / new_scale, (ax - e.x) * PT_PER_MM, (ay - e.y) * PT_PER_MM, w, h);
        if let Some(ll) = m.to_lonlat(c.center) {
            let _ = app.run("layout.update", json!({"layout": li, "element": id, "scale": new_scale, "center": [ll.x, ll.y]}));
        }
        return;
    }
    // The anchor's offset from the frame's centre, in points; times the resolution, map units.
    let (px, py) = ((ax - (e.x + e.w / 2.0)) * PT_PER_MM, -(ay - (e.y + e.h / 2.0)) * PT_PER_MM);
    let target = rustgis_engine::geom::Coord::new(cam.center.x + px * cam.res, cam.center.y + py * cam.res);
    // Centre the frame so the anchor's map point stays put. The resolution for a scale depends on
    // the centre's latitude (Web Mercator stretches away from the equator), so solve for the
    // centre's y by Newton's method, then place x with the resolution there.
    let res_at =
        |x: f64, y: f64| m.to_lonlat(rustgis_engine::geom::Coord::new(x, y)).map(|ll| frame_camera(m, ll, new_scale, 0.0, 0.0).res);
    let mut cx = cam.center.x;
    let mut cy = target.y - py * cam.res * new_scale / scale;
    for _ in 0..3 {
        for _ in 0..20 {
            let h = cy.abs().max(1.0) * 1e-7;
            let (Some(r0), Some(r1)) = (res_at(cx, cy), res_at(cx, cy + h)) else { return };
            let (f0, f1) = (cy + py * r0 - target.y, cy + h + py * r1 - target.y);
            let slope = (f1 - f0) / h;
            if !slope.is_finite() || slope.abs() < 1e-12 {
                break;
            }
            cy -= f0 / slope;
            if f0.abs() < r0 * 1e-3 {
                break;
            }
        }
        let Some(r) = res_at(cx, cy) else { return };
        cx = target.x - px * r;
    }
    let Some(ll) = m.to_lonlat(rustgis_engine::geom::Coord::new(cx, cy)) else { return };
    let _ = app.run("layout.update", json!({"layout": li, "element": id, "scale": new_scale, "center": [ll.x, ll.y]}));
}

/// Adds a polygon or lasso frame from the sketch.
fn finish_frame(app: &mut RustGisApp, li: usize, shape: &str) {
    let pts = std::mem::take(&mut app.layout.sketch);
    app.layout.tool = None;
    if pts.len() < 3 {
        return;
    }
    let (x0, y0) = pts.iter().fold((f64::MAX, f64::MAX), |a, p| (a.0.min(p.0), a.1.min(p.1)));
    let (x1, y1) = pts.iter().fold((f64::MIN, f64::MIN), |a, p| (a.0.max(p.0), a.1.max(p.1)));
    let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
    let unit: Vec<[f64; 2]> = pts.iter().map(|p| [(p.0 - x0) / w, (p.1 - y0) / h]).collect();
    if let Ok(v) = app.run("layout.addFrame", json!({"layout": li, "shape": shape, "points": unit, "x": x0, "y": y0, "w": w, "h": h})) {
        app.layout.selected = v["element"].as_u64().map(|v| v as u32);
    }
}

/// Saves the text typed into fields that are no longer shown: the selection or the layout changed
/// before the field could notice it lost focus.
fn save_drafts(app: &mut RustGisApp, li: usize) {
    let shown = app.layout.selected.map(|id| (li, id));
    let stale: Vec<_> = app.layout.typing.keys().filter(|(l, id, _)| Some((*l, *id)) != shown).copied().collect();
    for key in stale {
        let Some(text) = app.layout.typing.remove(&key) else { continue };
        let (l, id, field) = key;
        let current = app.session.project.layouts.get(l).and_then(|x| x.element(id)).and_then(|e| match (field, &e.kind) {
            ("name", _) => Some(e.name.clone()),
            ("text", ElementKind::Text { text, .. }) => Some(text.clone()),
            ("title", ElementKind::Legend { title, .. } | ElementKind::Colorbar { title, .. }) => Some(title.clone()),
            _ => None,
        });
        if current.is_some_and(|c| c != text) {
            let _ = app.run("layout.update", json!({"layout": l, "element": id, field: text}));
        }
    }
}

/// A text field whose typing is kept in the view's state while it has focus. Returns the new text
/// when the field loses focus with a change.
fn text_field(app: &mut RustGisApp, ui: &mut Ui, key: (usize, u32, &'static str), current: &str, multiline: bool) -> Option<String> {
    let mut buf = app.layout.typing.remove(&key).unwrap_or_else(|| current.to_string());
    let edit = if multiline {
        egui::TextEdit::multiline(&mut buf).desired_rows(3).desired_width(240.0)
    } else {
        egui::TextEdit::singleline(&mut buf)
    };
    let r = ui.add(edit);
    if r.lost_focus() {
        return (buf != current).then_some(buf);
    }
    if r.has_focus() {
        app.layout.typing.insert(key, buf);
    }
    None
}

/// A map frame's grid options; `set` sends the changed ones (`false` removes the grid).
fn grid_properties(app: &mut RustGisApp, ui: &mut Ui, grid: Option<&Grid>, id: u32, set: &dyn Fn(&mut RustGisApp, Value)) {
    let mut on = grid.is_some();
    if ui.checkbox(&mut on, tl!("Show grid")).changed() {
        set(app, json!(on));
    }
    let Some(g) = grid else { return };
    let changed = |r: &egui::Response| r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged());
    egui::Grid::new(("grid_props", id)).num_columns(2).spacing(vec2(8.0, 6.0)).show(ui, |ui| {
        ui.label(tl!("Interval"));
        let mut step = g.step;
        let r = ui.add(
            egui::DragValue::new(&mut step)
                .range(0.0..=90.0)
                .speed(0.05)
                .max_decimals(4)
                .custom_formatter(|v, _| if v <= 0.0 { tl!("Auto").to_string() } else { format!("{v}°") }),
        );
        if changed(&r) && (step - g.step).abs() > 1e-9 {
            set(app, json!({"step": step}));
        }
        ui.end_row();
        ui.label(tl!("Style"));
        egui::ComboBox::from_id_salt(("grid_style", id)).selected_text(tl!(g.style.label())).show_ui(ui, |ui| {
            for v in GridStyle::ALL {
                if ui.selectable_label(g.style == v, tl!(v.label())).clicked() && g.style != v {
                    set(app, json!({"style": v.code()}));
                }
            }
        });
        ui.end_row();
        ui.label(tl!("Color"));
        let mut c = crate::theme::c32(g.color);
        if crate::widgets::color_button(ui, &mut c).changed() {
            set(app, json!({"color": crate::theme::rgba(c).hex()}));
        }
        ui.end_row();
        ui.label(tl!("Line width"));
        let mut w = g.width;
        let r = ui.add(egui::DragValue::new(&mut w).range(0.0..=MAX_GRID_WIDTH).speed(0.05).max_decimals(2).suffix(" pt"));
        if changed(&r) && (w - g.width).abs() > 1e-9 {
            set(app, json!({"width": w}));
        }
        ui.end_row();
        if g.style.ticks() {
            ui.label(tl!("Tick length"));
            let mut t = g.tick_length;
            let r = ui.add(egui::DragValue::new(&mut t).range(0.0..=MAX_TICK_MM).speed(0.1).max_decimals(1).suffix(" mm"));
            if changed(&r) && (t - g.tick_length).abs() > 1e-9 {
                set(app, json!({"tickLength": t}));
            }
            ui.end_row();
        }
        ui.label(tl!("Labels"));
        let mut labels = g.labels;
        if ui.checkbox(&mut labels, "").changed() {
            set(app, json!({"labels": labels}));
        }
        ui.end_row();
        if !g.labels {
            return;
        }
        ui.label(tl!("Format"));
        ui.horizontal(|ui| {
            for (label, v, code) in [("Decimal", CoordFormat::Decimal, "decimal"), ("DMS", CoordFormat::Dms, "dms")] {
                if ui.selectable_label(g.format == v, tl!(label)).clicked() && g.format != v {
                    set(app, json!({"format": code}));
                }
            }
        });
        ui.end_row();
        ui.label(tl!("N/S/E/W"));
        let mut hemi = g.hemisphere;
        if ui.checkbox(&mut hemi, "").on_hover_text(tl!("Off, west and south are negative: -112.5°.")).changed() {
            set(app, json!({"hemisphere": hemi}));
        }
        ui.end_row();
        // Two rows, so the four edges fit the panel's width.
        let e = g.edges;
        let all = [("Top", "top", e.top), ("Bottom", "bottom", e.bottom), ("Left", "left", e.left), ("Right", "right", e.right)];
        for (row, label) in [(0, tl!("Edges")), (2, "")] {
            ui.label(label);
            ui.horizontal(|ui| {
                for (i, (label, _, shown)) in all.iter().enumerate().skip(row).take(2) {
                    if ui.selectable_label(*shown, tl!(label)).clicked() {
                        let names: Vec<&str> = all
                            .iter()
                            .enumerate()
                            .filter(|(j, (_, _, s))| if *j == i { !*s } else { *s })
                            .map(|(_, (_, n, _))| *n)
                            .collect();
                        set(app, json!({"edges": names}));
                    }
                }
            });
            ui.end_row();
        }
        ui.label(tl!("Side labels"));
        ui.horizontal(|ui| {
            let vertical = g.label_orientation == Orientation::Vertical;
            for (label, v) in [("Horizontal", false), ("Vertical", true)] {
                if ui.selectable_label(vertical == v, tl!(label)).clicked() && vertical != v {
                    set(app, json!({"labelOrientation": if v { "vertical" } else { "horizontal" }}));
                }
            }
        });
        ui.end_row();
    });
}

/// The selected element's properties.
fn properties(app: &mut RustGisApp, ui: &mut Ui, layout: &Layout, li: usize) {
    let Some(e) = app.layout.selected.and_then(|id| layout.element(id)).cloned() else { return };
    let id = e.id;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tl!(e.kind.label())).font(semibold(14.0)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if crate::widgets::icon_button(ui, "close", tl!("Close")).clicked() {
                app.layout.selected = None;
            }
        });
    });
    ui.separator();
    let update = |app: &mut RustGisApp, p: Value| {
        let mut p = p;
        p["layout"] = json!(li);
        p["element"] = json!(id);
        let _ = app.run("layout.update", p);
    };
    egui::Grid::new(("layout_props", id)).num_columns(2).spacing(vec2(8.0, 6.0)).show(ui, |ui| {
        ui.label(tl!("Name"));
        if let Some(name) = text_field(app, ui, (li, id, "name"), &e.name, false) {
            update(app, json!({"name": name}));
        }
        ui.end_row();
        for (label, key, v) in [("X (mm)", "x", e.x), ("Y (mm)", "y", e.y), ("Width (mm)", "w", e.w), ("Height (mm)", "h", e.h)] {
            ui.label(tl!(label));
            let mut val = v;
            let r = ui.add(egui::DragValue::new(&mut val).speed(0.5).max_decimals(1));
            if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged())) && (val - v).abs() > 1e-6 {
                update(app, json!({key: val}));
            }
            ui.end_row();
        }
        ui.label(tl!("Visible"));
        let mut vis = e.visible;
        if ui.checkbox(&mut vis, "").changed() {
            update(app, json!({"visible": vis}));
        }
        ui.end_row();
        match &e.kind {
            ElementKind::Text { text, size, color, bold, italic, align, letter_spacing } => {
                ui.label(tl!("Size"));
                let mut sz = *size;
                let r = ui.add(egui::DragValue::new(&mut sz).range(2.0..=400.0).speed(0.2).suffix(" pt"));
                if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged())) && (sz - size).abs() > 1e-6 {
                    update(app, json!({"size": sz}));
                }
                ui.end_row();
                ui.label(tl!("Color"));
                let mut c = crate::theme::c32(*color);
                if crate::widgets::color_button(ui, &mut c).changed() {
                    update(app, json!({"color": crate::theme::rgba(c).hex()}));
                }
                ui.end_row();
                ui.label(tl!("Bold"));
                let mut bb = *bold;
                if ui.checkbox(&mut bb, "").changed() {
                    update(app, json!({"bold": bb}));
                }
                ui.end_row();
                ui.label(tl!("Italic"));
                let mut it = *italic;
                // Named for assistive technology, as its label sits in the other column.
                let r = ui.checkbox(&mut it, "");
                r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, it, tl!("Italic")));
                if r.changed() {
                    update(app, json!({"italic": it}));
                }
                ui.end_row();
                ui.label(tl!("Alignment"));
                egui::ComboBox::from_id_salt(("text_align", id)).selected_text(tl!(align.label())).show_ui(ui, |ui| {
                    for v in TextAlign::ALL {
                        if ui.selectable_label(*align == v, tl!(v.label())).clicked() && *align != v {
                            update(app, json!({"align": v.code()}));
                        }
                    }
                });
                ui.end_row();
                ui.label(tl!("Letter spacing"));
                let mut ls = *letter_spacing;
                let r = ui.add(
                    egui::DragValue::new(&mut ls).range(MIN_LETTER_SPACING..=MAX_LETTER_SPACING).speed(0.1).max_decimals(1).suffix(" pt"),
                );
                r.widget_info(|| egui::WidgetInfo {
                    label: Some(tl!("Letter spacing").to_string()),
                    ..egui::WidgetInfo::drag_value(true, ls)
                });
                if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged())) && (ls - letter_spacing).abs() > 1e-6 {
                    update(app, json!({"letterSpacing": ls}));
                }
                ui.end_row();
                ui.label(tl!("Text"));
                ui.end_row();
                if let Some(tx) = text_field(app, ui, (li, id, "text"), text, true) {
                    update(app, json!({"text": tx}));
                }
                ui.end_row();
            }
            ElementKind::Legend { title, all_classes, hide_other, .. } => {
                ui.label(tl!("Title"));
                if let Some(tt) = text_field(app, ui, (li, id, "title"), title, false) {
                    update(app, json!({"title": tt}));
                }
                ui.end_row();
                // Off, the legend lists only what the frame draws (#207).
                ui.label(tl!("All classes"));
                let mut all = *all_classes;
                if ui.checkbox(&mut all, "").changed() {
                    update(app, json!({"allClasses": all}));
                }
                ui.end_row();
                ui.label(tl!("Hide other values"));
                let mut hide = *hide_other;
                if ui.checkbox(&mut hide, "").changed() {
                    update(app, json!({"hideOther": hide}));
                }
                ui.end_row();
            }
            ElementKind::Colorbar { frame, layer, title, orientation, ticks, tick_side, reverse } => {
                ui.label(tl!("Title"));
                if let Some(tt) = text_field(app, ui, (li, id, "title"), title, false) {
                    update(app, json!({"title": tt}));
                }
                ui.end_row();
                // The rasters of the frame's map; none picked follows the top visible one.
                ui.label(tl!("Layer"));
                let rasters: Vec<String> = match layout.element(*frame).map(|f| &f.kind) {
                    Some(ElementKind::MapFrame { map, .. }) => app
                        .session
                        .project
                        .maps
                        .iter()
                        .find(|m| m.name.eq_ignore_ascii_case(map))
                        .map(|m| m.layers.iter().filter(|l| l.is_raster()).map(|l| l.name.clone()).collect())
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                let top = tl!("Top visible raster");
                egui::ComboBox::from_id_salt(("colorbar_layer", id)).selected_text(layer.as_deref().unwrap_or(top)).show_ui(ui, |ui| {
                    if ui.selectable_label(layer.is_none(), top).clicked() {
                        update(app, json!({"layer": ""}));
                    }
                    for n in rasters {
                        if ui.selectable_label(layer.as_deref() == Some(n.as_str()), n.as_str()).clicked() {
                            update(app, json!({"layer": n}));
                        }
                    }
                });
                ui.end_row();
                let vertical = *orientation == Orientation::Vertical;
                ui.label(tl!("Orientation"));
                ui.horizontal(|ui| {
                    for (label, v) in [("Horizontal", false), ("Vertical", true)] {
                        if ui.selectable_label(vertical == v, tl!(label)).clicked() && vertical != v {
                            update(app, json!({"orientation": if v { "vertical" } else { "horizontal" }}));
                        }
                    }
                });
                ui.end_row();
                ui.label(tl!("Labels"));
                ui.horizontal(|ui| {
                    let sides = if vertical { [("Right", "right"), ("Left", "left")] } else { [("Below", "below"), ("Above", "above")] };
                    for ((label, code), side) in sides.into_iter().zip([TickSide::End, TickSide::Start]) {
                        if ui.selectable_label(*tick_side == side, tl!(label)).clicked() && *tick_side != side {
                            update(app, json!({"tickSide": code}));
                        }
                    }
                });
                ui.end_row();
                ui.label(tl!("Ticks"));
                let mut n = *ticks as u64;
                let r = ui.add(egui::DragValue::new(&mut n).range(2..=MAX_COLORBAR_TICKS as u64));
                if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged())) && n as usize != *ticks {
                    update(app, json!({"ticks": n}));
                }
                ui.end_row();
                ui.label(tl!("Reverse"));
                let mut rev = *reverse;
                if ui.checkbox(&mut rev, "").changed() {
                    update(app, json!({"reverse": rev}));
                }
                ui.end_row();
            }
            ElementKind::MapFrame { map, scale, bearing, pitch, .. } => {
                ui.label(tl!("Map"));
                let maps: Vec<String> = app.session.project.maps.iter().map(|m| m.name.clone()).collect();
                egui::ComboBox::from_id_salt(("frame_map", id)).selected_text(map.as_str()).show_ui(ui, |ui| {
                    for m in maps {
                        if ui.selectable_label(m == *map, m.as_str()).clicked() {
                            update(app, json!({"map": m}));
                        }
                    }
                });
                ui.end_row();
                ui.label(tl!("Scale 1:"));
                let mut sc = *scale;
                let speed = sc * 0.01;
                let r = ui.add(egui::DragValue::new(&mut sc).range(100.0..=1e10).speed(speed).max_decimals(0));
                if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged())) && (sc - scale).abs() > 0.5 {
                    update(app, json!({"scale": sc}));
                }
                ui.end_row();
                // Turned and tilted as the map view is (Ctrl+drag there; Shift+arrows).
                for (label, key, current, range) in
                    [("Bearing", "bearing", *bearing, -180.0..=180.0), ("Pitch", "pitch", *pitch, 0.0..=85.0)]
                {
                    ui.label(tl!(label));
                    let mut v = current;
                    let r = ui.add(egui::DragValue::new(&mut v).range(range).speed(0.5).max_decimals(1).suffix("°"));
                    if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged())) && (v - current).abs() > 1e-6 {
                        update(app, json!({key: v}));
                    }
                    ui.end_row();
                }
            }
            ElementKind::NorthArrow { style, fill, outline, .. } => {
                ui.label(tl!("Style"));
                egui::ComboBox::from_id_salt(("arrow_style", id)).selected_text(tl!(style.label())).show_ui(ui, |ui| {
                    for v in ArrowStyle::ALL {
                        if ui.selectable_label(*style == v, tl!(v.label())).clicked() && *style != v {
                            update(app, json!({"style": v.code()}));
                        }
                    }
                });
                ui.end_row();
                for (label, key, current) in [("Fill", "fill", *fill), ("Outline", "outline", *outline)] {
                    ui.label(tl!(label));
                    let mut c = crate::theme::c32(current);
                    if crate::widgets::color_button(ui, &mut c).changed() {
                        update(app, json!({key: crate::theme::rgba(c).hex()}));
                    }
                    ui.end_row();
                }
            }
            ElementKind::ScaleBar { units, divisions, style, .. } => {
                ui.label(tl!("Style"));
                egui::ComboBox::from_id_salt(("scale_style", id)).selected_text(tl!(style.label())).show_ui(ui, |ui| {
                    for v in ScaleBarStyle::ALL {
                        if ui.selectable_label(*style == v, tl!(v.label())).clicked() && *style != v {
                            update(app, json!({"style": v.code()}));
                        }
                    }
                });
                ui.end_row();
                ui.label(tl!("Units"));
                egui::ComboBox::from_id_salt(("scale_units", id)).selected_text(tl!(units.label())).show_ui(ui, |ui| {
                    for v in ScaleUnits::ALL {
                        if ui.selectable_label(*units == v, tl!(v.label())).clicked() && *units != v {
                            update(app, json!({"units": v.code()}));
                        }
                    }
                });
                ui.end_row();
                ui.label(tl!("Divisions"));
                let mut n = *divisions as u64;
                let r = ui.add(egui::DragValue::new(&mut n).range(1..=MAX_SCALE_DIVISIONS as u64));
                if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged())) && n as usize != *divisions {
                    update(app, json!({"divisions": n}));
                }
                ui.end_row();
            }
            ElementKind::Table { rows, .. } => {
                ui.label(tl!("Rows"));
                let mut n = *rows as u64;
                let r = ui.add(egui::DragValue::new(&mut n).range(1..=500));
                if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged())) && n as usize != *rows {
                    update(app, json!({"rows": n}));
                }
                ui.end_row();
            }
            _ => {}
        }
    });
    // The layers a legend lists: a checkbox for each vector layer of its frame's map, ticked when
    // listed. Names gone from the map drop out of the list as it's written back.
    if let ElementKind::Legend { frame, exclude, .. } = &e.kind {
        let map = layout.element(*frame).and_then(|f| match &f.kind {
            ElementKind::MapFrame { map, .. } => app.session.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(map)),
            _ => None,
        });
        let names: Vec<String> = map
            .map(|m| {
                // Once each: layers may share a name, and the legend leaves names out.
                let mut names: Vec<String> = Vec::new();
                for l in m.layers.iter().filter(|l| !l.graphics && !l.is_raster()) {
                    if !names.iter().any(|n| n.eq_ignore_ascii_case(&l.name)) {
                        names.push(l.name.clone());
                    }
                }
                names
            })
            .unwrap_or_default();
        // The names left out that the map still has, those not listed here (a raster's) too.
        let kept: Vec<String> =
            exclude.iter().filter(|x| map.is_some_and(|m| m.layers.iter().any(|l| l.name.eq_ignore_ascii_case(x)))).cloned().collect();
        if !names.is_empty() {
            ui.add_space(4.0);
            egui::CollapsingHeader::new(tl!("Layers")).id_salt(("legend_layers", id)).default_open(!exclude.is_empty()).show(ui, |ui| {
                let left_out = |n: &str| exclude.iter().any(|x| x.eq_ignore_ascii_case(n));
                for n in &names {
                    let mut listed = !left_out(n);
                    if ui.checkbox(&mut listed, n.as_str()).on_hover_text(tl!("List this layer in the legend")).changed() {
                        let mut next: Vec<&String> = kept.iter().filter(|x| !x.eq_ignore_ascii_case(n)).collect();
                        if !listed {
                            next.push(n);
                        }
                        update(app, json!({"exclude": next}));
                    }
                }
            });
        }
    }
    if let ElementKind::MapFrame { grid, .. } = &e.kind {
        ui.add_space(4.0);
        egui::CollapsingHeader::new(tl!("Grid")).id_salt(("frame_grid", id)).default_open(grid.is_some()).show(ui, |ui| {
            grid_properties(app, ui, grid.as_ref(), id, &|app, g| update(app, json!({"grid": g})));
        });
    }
    // The extent indicator this frame draws (another frame's extent): its colour and width.
    if let ElementKind::MapFrame { extent_of: Some(of), extent_color, extent_width, .. } = &e.kind {
        let other = layout.element(*of).map_or_else(|| of.to_string(), |o| o.name.clone());
        ui.add_space(4.0);
        egui::CollapsingHeader::new(tl!("Extent Indicator")).id_salt(("frame_indicator", id)).default_open(true).show(ui, |ui| {
            ui.label(egui::RichText::new(crate::i18n::fmt("Outlines {frame}", &[("frame", &other)])).size(11.5));
            egui::Grid::new(("indicator_props", id)).num_columns(2).show(ui, |ui| {
                ui.label(tl!("Color"));
                let mut c = crate::theme::c32(*extent_color);
                if crate::widgets::color_button(ui, &mut c).changed() {
                    update(app, json!({"extentIndicator": {"color": crate::theme::rgba(c).hex()}}));
                }
                ui.end_row();
                ui.label(tl!("Line width"));
                let mut w = *extent_width;
                let r = ui.add(
                    egui::DragValue::new(&mut w)
                        .range(rustgis_engine::map::layout::MIN_INDICATOR_WIDTH..=rustgis_engine::map::layout::MAX_INDICATOR_WIDTH)
                        .speed(0.05)
                        .max_decimals(2)
                        .suffix(" pt"),
                );
                if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged())) && (w - extent_width).abs() > 1e-6 {
                    update(app, json!({"extentIndicator": {"width": w}}));
                }
                ui.end_row();
            });
            if ui.button(tl!("Remove Extent Indicator")).clicked() {
                update(app, json!({"extentIndicator": false}));
            }
        });
    }
    ui.add_space(8.0);
    if let ElementKind::MapFrame { .. } = e.kind {
        ui.toggle_value(&mut app.layout.navigate, tl!("Pan and Zoom Map"))
            .on_hover_text(tl!("Drag inside map frames to pan their maps and turn the wheel to zoom them, instead of moving them."));
        ui.horizontal_wrapped(|ui| {
            for (label, to) in [("Zoom To Map View", "view"), ("Full Extent", "full"), ("Selection", "selection")] {
                if ui.button(tl!(label)).clicked() {
                    let _ = app.run("layout.frameZoom", json!({"layout": li, "element": id, "to": to}));
                }
            }
        });
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button(tl!("Duplicate")).on_hover_text(tl!("Copy this element (Ctrl+D, ⌘D on a Mac)")).clicked()
            && let Ok(v) = app.run("layout.duplicateElement", json!({"layout": li, "element": id}))
        {
            app.layout.selected = v["element"].as_u64().and_then(|v| u32::try_from(v).ok());
        }
        if ui.button(tl!("Delete Element")).on_hover_text(tl!("Delete (or Backspace)")).clicked() {
            let _ = app.run("layout.removeElement", json!({"layout": li, "element": id}));
            app.layout.selected = None;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{RULER, landing, page_rect, resized, snap_to, to_screen};
    use crate::RustGisApp;
    use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, vec2};
    use serde_json::json;
    use std::{cell::Cell, rc::Rc};

    /// Dragging an element redraws the page with it where it was dropped (#325).
    #[test]
    fn dropped_element_is_drawn_where_it_lands() {
        let mut app = RustGisApp::with_sample();
        app.run("layout.new", json!({"name": "Drag"})).unwrap();
        // The drop itself, not snapping (tested below).
        app.layout.no_snap = true;
        let area = Rc::new(Cell::new(Rect::NOTHING));
        let shown = area.clone();
        // Fonts take effect on the frame after they are installed.
        let mut fonts = false;
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1000.0, 760.0)).build_ui_state(
            move |ui, app: &mut RustGisApp| {
                if !fonts {
                    crate::theme::install_fonts(ui.ctx(), &[], None);
                    fonts = true;
                    return;
                }
                shown.set(ui.available_rect_before_wrap());
                super::show(app, ui);
            },
            app,
        );
        h.run_steps(3);
        let li = h.state().session.active_layout.unwrap();
        let layout = h.state().session.project.layouts[li].clone();
        let title = layout.elements.iter().find(|e| e.name == "Title").unwrap().clone();
        let (page, s) = page_rect(&h.state().layout, area.get(), &layout);
        let from = to_screen(page, s, (title.x + title.w / 2.0, title.y + title.h / 2.0));
        let to = from + vec2(0.0, 120.0);
        let button = |pos: Pos2, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        h.event(Event::PointerMoved(from));
        h.step();
        h.event(button(from, true));
        h.step();
        for i in 1..=6 {
            h.event(Event::PointerMoved(from + (to - from) * i as f32 / 6.0));
            h.step();
        }
        let before = h.state().session.revision;
        h.event(button(to, false));
        h.step();
        let app = h.state();
        assert!(app.session.revision > before, "the drop should update the layout");
        let moved = app.session.project.layouts[li].element(title.id).unwrap();
        // A vertical drag moves it straight down by the pointer's motion, although the
        // properties opened beside the page and re-fitted it mid-drag.
        let (_, s) = app.layout.drag_page.unwrap();
        assert!((moved.x - title.x).abs() < 0.01, "x moved from {} to {}", title.x, moved.x);
        // The drag starts once the pointer has moved, a step into the 120 points.
        let dy = (moved.y - title.y) * s as f64;
        assert!((90.0..=120.5).contains(&dy), "y moved {dy} points");
        // The page was drawn, and keyed, from the layout as it is after the drop.
        let (i, rev, drawn) = app.layout.shown.as_ref().unwrap();
        assert_eq!((*i, *rev), (li, app.session.revision));
        assert_eq!(drawn.element(title.id).map(|e| e.y), Some(moved.y));
        assert_eq!(app.layout.key.map(|k| k.0), Some(app.session.revision));
    }

    /// Dragging the map in a turned frame moves it the way the pointer goes, as in the map view.
    #[test]
    fn panning_a_turned_frame_follows_the_pointer() {
        use rustgis_engine::map::layout::ElementKind;
        let mut app = RustGisApp::with_sample();
        app.run("layout.new", json!({"name": "Turned"})).unwrap();
        let li = app.session.active_layout.unwrap();
        let id = app.session.project.layouts[li].first_frame().unwrap();
        // East at the top: the page's right is south, its left north.
        app.run("layout.update", json!({"layout": li, "element": id, "bearing": 90, "center": [10.0, 20.0]})).unwrap();
        let layout = app.session.project.layouts[li].clone();
        super::pan_frame(&mut app, &layout, li, id, 10.0, 0.0);
        let Some(ElementKind::MapFrame { center, .. }) = app.session.project.layouts[li].element(id).map(|e| e.kind.clone()) else {
            panic!("not a map frame")
        };
        // Dragged right, the map brings in what lay to the left: the centre moves north.
        assert!(center.y > 20.5 && (center.x - 10.0).abs() < 0.01, "{center:?}");
    }

    /// Zooming a map frame about a point keeps the map under that point where it was.
    #[test]
    fn zooming_a_frame_keeps_the_map_under_the_pointer() {
        use rustgis_engine::map::layout::{ElementKind, PT_PER_MM, frame_camera};
        let mut app = RustGisApp::with_sample();
        app.run("layout.new", json!({"name": "Zoom"})).unwrap();
        let li = app.session.active_layout.unwrap();
        let layout = app.session.project.layouts[li].clone();
        let frame = layout.elements.iter().find(|e| matches!(e.kind, ElementKind::MapFrame { .. })).unwrap().clone();
        // A point a quarter of the way into the frame.
        let anchor = (frame.x + frame.w / 4.0, frame.y + frame.h / 4.0);
        // The map point (map units) at the anchor, the frame's scale, and map units per point.
        let map_at = |app: &RustGisApp| {
            let e = app.session.project.layouts[li].element(frame.id).unwrap().clone();
            let ElementKind::MapFrame { map, center, scale, .. } = &e.kind else { panic!("not a map frame") };
            let m = app.session.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(map)).unwrap();
            let cam = frame_camera(m, *center, *scale, 0.0, 0.0);
            let x = cam.center.x + (anchor.0 - (e.x + e.w / 2.0)) * PT_PER_MM * cam.res;
            let y = cam.center.y - (anchor.1 - (e.y + e.h / 2.0)) * PT_PER_MM * cam.res;
            (x, y, *scale, cam.res)
        };
        let before = map_at(&app);
        let undo = app.session.undo_depth();
        super::zoom_frame(&mut app, &layout, li, frame.id, 0.5, anchor);
        let after = map_at(&app);
        assert!((after.2 - before.2 * 0.5).abs() < 1e-6 * before.2, "the scale halves: {} → {}", before.2, after.2);
        assert!((after.0 - before.0).hypot(after.1 - before.1) < after.3, "the anchor's map point moved: {before:?} → {after:?}");
        assert_eq!(app.session.undo_depth(), undo + 1, "one undo step");
    }

    /// A wheel zoom still waiting is applied, not lost, when another frame is zoomed; one waiting
    /// for a frame of another layout is dropped, never applied to the open layout.
    #[test]
    fn waiting_frame_zooms_are_applied_or_dropped_never_misapplied() {
        use rustgis_engine::map::layout::ElementKind;
        let scale_of = |app: &RustGisApp, li: usize, id: u32| match app.session.project.layouts[li].element(id).map(|e| &e.kind) {
            Some(ElementKind::MapFrame { scale, .. }) => *scale,
            k => panic!("not a map frame: {k:?}"),
        };
        let mut app = RustGisApp::with_sample();
        app.run("layout.new", json!({"name": "Two"})).unwrap();
        let li = app.session.active_layout.unwrap();
        let name = app.session.project.layouts[li].name.clone();
        let a = app.session.project.layouts[li].first_frame().unwrap();
        let b = app.run("layout.addFrame", json!({"layout": li, "x": 20, "y": 20, "w": 60, "h": 40})).unwrap()["element"].as_u64().unwrap()
            as u32;
        app.layout.navigate = true;
        let (sa, sb) = (scale_of(&app, li, a), scale_of(&app, li, b));
        // Frame A's zoom waits: its wheel turned "later than ever", so it can't settle by itself.
        app.layout.frame_zoom = Some((name.clone(), a, 0.5, (100.0, 100.0), f64::MAX));
        let area = Rc::new(Cell::new(Rect::NOTHING));
        let shown = area.clone();
        let mut fonts = false;
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1000.0, 760.0)).build_ui_state(
            move |ui, app: &mut RustGisApp| {
                if !fonts {
                    crate::theme::install_fonts(ui.ctx(), &[], None);
                    fonts = true;
                    return;
                }
                shown.set(ui.available_rect_before_wrap());
                super::show(app, ui);
            },
            app,
        );
        h.run_steps(2);
        let layout = h.state().session.project.layouts[li].clone();
        let (page, s) = page_rect(&h.state().layout, area.get(), &layout);
        // Zoom in over frame B.
        h.event(Event::PointerMoved(to_screen(page, s, (50.0, 40.0))));
        h.step();
        h.event(Event::Zoom(1.25));
        h.step();
        assert!((scale_of(h.state(), li, a) - sa * 0.5).abs() < 1e-6 * sa, "A's waiting zoom was applied");
        assert_eq!(h.state().layout.frame_zoom.as_ref().map(|z| z.1), Some(b), "B's zoom waits for the wheel to stop");
        h.run_steps(3);
        assert!(h.state().layout.frame_zoom.is_none());
        assert!(scale_of(h.state(), li, b) < sb, "B zoomed in once the wheel stopped");

        // A zoom left waiting by another layout is dropped without touching this one.
        h.state_mut().layout.frame_zoom = Some(("Another layout".into(), a, 0.5, (100.0, 100.0), 0.0));
        let rev = h.state().session.revision;
        h.run_steps(2);
        assert!(h.state().layout.frame_zoom.is_none());
        assert_eq!(h.state().session.revision, rev, "nothing was applied");
    }

    /// Ctrl+D (Cmd+D) duplicates the selected element and selects the copy; Backspace, the Mac's
    /// delete key, deletes it.
    #[test]
    fn keys_duplicate_and_delete_the_selected_element() {
        let mut app = RustGisApp::with_sample();
        app.run("layout.new", json!({"name": "Keys"})).unwrap();
        let li = app.session.active_layout.unwrap();
        let title = app.session.project.layouts[li].elements.iter().find(|e| e.name == "Title").unwrap().clone();
        app.layout.selected = Some(title.id);
        let mut fonts = false;
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1000.0, 760.0)).build_ui_state(
            move |ui, app: &mut RustGisApp| {
                if !fonts {
                    crate::theme::install_fonts(ui.ctx(), &[], None);
                    fonts = true;
                    return;
                }
                let ctx = ui.ctx().clone();
                crate::chrome::shortcuts(app, &ctx);
                super::show(app, ui);
            },
            app,
        );
        h.run_steps(3);
        let key = |key, modifiers| Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        let n = h.state().session.project.layouts[li].elements.len();

        h.event(key(Key::D, Modifiers::COMMAND));
        h.step();
        let app = h.state();
        let layout = &app.session.project.layouts[li];
        assert_eq!(layout.elements.len(), n + 1, "Ctrl+D added a copy");
        let copy = layout.element(app.layout.selected.unwrap()).unwrap();
        assert_ne!(copy.id, title.id, "the copy is selected");
        assert_eq!((copy.name.as_str(), copy.x, copy.y, copy.w), ("Title 2", title.x + 5.0, title.y + 5.0, title.w));
        let copy = copy.id;

        h.event(key(Key::Backspace, Modifiers::NONE));
        h.step();
        let app = h.state();
        let layout = &app.session.project.layouts[li];
        assert_eq!(layout.elements.len(), n, "Backspace deleted the copy");
        assert!(layout.element(copy).is_none() && layout.element(title.id).is_some());
        assert_eq!(app.layout.selected, None);
    }

    /// A text element's properties set italic, alignment and letter spacing (#213, point c).
    #[test]
    fn a_text_element_takes_italic_alignment_and_spacing() {
        use egui_kittest::kittest::{NodeT, Queryable};
        use rustgis_engine::map::layout::{ElementKind, TextAlign};
        let mut app = RustGisApp::with_sample();
        app.run("layout.new", json!({"name": "Type"})).unwrap();
        let li = app.session.active_layout.unwrap();
        let title = app.session.project.layouts[li].elements.iter().find(|e| e.name == "Title").unwrap().id;
        app.layout.selected = Some(title);
        let mut fonts = false;
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1200.0, 900.0)).build_ui_state(
            move |ui, app: &mut RustGisApp| {
                if !fonts {
                    crate::theme::install_fonts(ui.ctx(), &[], None);
                    fonts = true;
                    return;
                }
                super::show(app, ui);
            },
            app,
        );
        h.run_steps(3);
        use egui::accesskit::{Action, ActionData, ActionRequest, Role};
        assert!(h.query_by_label("Alignment").is_some());
        // Ticking Italic, and setting the spacing as assistive technology does, store them.
        h.get_by_role_and_label(Role::CheckBox, "Italic").click();
        h.run_steps(2);
        let (node, tree) = h.get_by_role_and_label(Role::SpinButton, "Letter spacing").accesskit_node().locate();
        h.event(egui::Event::AccessKitActionRequest(ActionRequest {
            action: Action::SetValue,
            target_tree: tree,
            target_node: node,
            data: Some(ActionData::NumericValue(3.5)),
        }));
        h.run_steps(3);
        let kind = &h.state().session.project.layouts[li].element(title).unwrap().kind;
        assert!(
            matches!(kind, ElementKind::Text { italic: true, align: TextAlign::Left, letter_spacing, .. } if *letter_spacing == 3.5),
            "{kind:?}"
        );
    }

    /// An overview frame's properties show its extent indicator's colour and width (#213), and
    /// its Remove button takes the indicator away.
    #[test]
    fn an_overview_frame_shows_its_extent_indicator() {
        use egui_kittest::kittest::Queryable;
        use rustgis_engine::map::layout::ElementKind;
        let mut app = RustGisApp::with_sample();
        app.run("layout.new", json!({"name": "Overview"})).unwrap();
        let li = app.session.active_layout.unwrap();
        let main = app.session.project.layouts[li].elements.iter().find(|e| matches!(e.kind, ElementKind::MapFrame { .. })).unwrap().id;
        let inset = app.run("layout.addFrame", json!({"x": 200, "y": 150, "w": 60, "h": 40})).unwrap()["element"].as_u64().unwrap() as u32;
        app.run("layout.addElement", json!({"kind": "extentIndicator", "frame": inset, "of": main, "width": 1.5})).unwrap();
        app.layout.selected = Some(inset);
        let mut fonts = false;
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1200.0, 900.0)).build_ui_state(
            move |ui, app: &mut RustGisApp| {
                if !fonts {
                    crate::theme::install_fonts(ui.ctx(), &[], None);
                    fonts = true;
                    return;
                }
                super::show(app, ui);
            },
            app,
        );
        h.run_steps(3);
        assert!(h.query_by_label("Extent Indicator").is_some(), "the section shows for an overview frame");
        h.get_by_label("Remove Extent Indicator").click();
        h.run_steps(2);
        let kind = &h.state().session.project.layouts[li].element(inset).unwrap().kind;
        assert!(matches!(kind, ElementKind::MapFrame { extent_of: None, .. }), "removed");
        h.run_steps(2);
        assert!(h.query_by_label("Remove Extent Indicator").is_none(), "and its section with it");
    }

    /// A legend's properties list the layers of its frame's map; unticking one leaves it out of
    /// the legend.
    #[test]
    fn a_legend_leaves_out_the_layers_unticked() {
        use egui::accesskit::Role;
        use egui_kittest::kittest::Queryable;
        use rustgis_engine::map::layout::ElementKind;
        let mut app = RustGisApp::with_sample();
        app.run("layout.new", json!({"name": "Legend"})).unwrap();
        let li = app.session.active_layout.unwrap();
        let legend = app.session.project.layouts[li].elements.iter().find(|e| matches!(e.kind, ElementKind::Legend { .. })).unwrap().id;
        // A layer the section doesn't list (graphics here), left out through the command, stays
        // left out whatever is ticked.
        app.session.map_mut().layers.iter_mut().find(|l| l.name == "Cities").unwrap().graphics = true;
        app.run("layout.update", json!({"element": legend, "exclude": ["Cities"]})).unwrap();
        app.layout.selected = Some(legend);
        let mut fonts = false;
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1200.0, 900.0)).build_ui_state(
            move |ui, app: &mut RustGisApp| {
                if !fonts {
                    crate::theme::install_fonts(ui.ctx(), &[], None);
                    fonts = true;
                    return;
                }
                super::show(app, ui);
            },
            app,
        );
        h.run_steps(3);
        assert!(h.query_by_role_and_label(Role::CheckBox, "Cities").is_none());
        h.get_by_role_and_label(Role::CheckBox, "Rivers").click();
        h.run_steps(2);
        let left_out =
            |h: &egui_kittest::Harness<'_, RustGisApp>| match &h.state().session.project.layouts[li].element(legend).unwrap().kind {
                ElementKind::Legend { exclude, .. } => exclude.clone(),
                _ => panic!("a legend"),
            };
        assert_eq!(left_out(&h), ["Cities", "Rivers"]);
        // Ticked again, it's listed again.
        h.get_by_role_and_label(Role::CheckBox, "Rivers").click();
        h.run_steps(2);
        assert_eq!(left_out(&h), ["Cities"]);
    }

    /// Element ids are per layout: opening another layout ends the selection, so Delete or
    /// Ctrl+D never reaches the element of the same id there.
    #[test]
    fn opening_another_layout_clears_the_selection() {
        let mut app = RustGisApp::with_sample();
        app.run("layout.new", json!({"name": "First"})).unwrap();
        app.run("layout.new", json!({"name": "Second"})).unwrap();
        app.run("layout.activate", json!({"name": "First"})).unwrap();
        let first = app.session.active_layout.unwrap();
        let title = app.session.project.layouts[first].elements.iter().find(|e| e.name == "Title").unwrap().id;
        app.layout.selected = Some(title);
        app.run("layout.removeElement", json!({"element": 999_999})).ok();
        assert_eq!(app.layout.selected, Some(title), "a command in the same layout keeps it");
        app.run("layout.activate", json!({"name": "Second"})).unwrap();
        assert_ne!(app.session.active_layout, Some(first));
        assert!(app.session.project.layouts[app.session.active_layout.unwrap()].element(title).is_some(), "the id exists there too");
        assert_eq!(app.layout.selected, None);
    }

    #[test]
    fn dragged_elements_snap_to_the_nearest_line() {
        let lines = (vec![0.0, 100.0, 150.0], vec![0.0, 50.0]);
        assert_eq!(snap_to(&[98.5, 120.0], &lines.0, 2.0), Some((1.5, 100.0)));
        assert_eq!(snap_to(&[90.0], &lines.0, 2.0), None);
        let r = (10.0, 10.0, 40.0, 20.0);
        // Moved: the right edge (50 + 49) is nearest a line, so the box lands with it on 100.
        let (moved, sx, sy) = landing(r, None, (49.0, 0.5), Some((&lines, 2.0)));
        assert_eq!((moved, sx, sy), ((60.0, 10.5, 40.0, 20.0), Some(100.0), None));
        // Without snapping it lands where the pointer put it.
        assert_eq!(landing(r, None, (49.0, 0.5), None).0, (59.0, 10.5, 40.0, 20.0));
        // Resized from the bottom-right handle: only those edges snap (148.5 → 150, 49 → 50);
        // the top-left stays.
        let (sized, sx, sy) = landing(r, Some(4), (98.5, 19.0), Some((&lines, 2.0)));
        assert_eq!((sized, sx, sy), ((10.0, 10.0, 140.0, 40.0), Some(150.0), Some(50.0)));
    }

    /// Dragging out of the top ruler adds a horizontal guide; dragging it back onto the ruler
    /// removes it.
    #[test]
    fn guides_come_out_of_the_rulers_and_go_back() {
        let mut app = RustGisApp::with_sample();
        app.run("layout.new", json!({"name": "Guides", "template": "blank"})).unwrap();
        let area = Rc::new(Cell::new(Rect::NOTHING));
        let shown = area.clone();
        let mut fonts = false;
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1000.0, 760.0)).build_ui_state(
            move |ui, app: &mut RustGisApp| {
                if !fonts {
                    crate::theme::install_fonts(ui.ctx(), &[], None);
                    fonts = true;
                    return;
                }
                shown.set(ui.available_rect_before_wrap());
                super::show(app, ui);
            },
            app,
        );
        h.run_steps(3);
        let li = h.state().session.active_layout.unwrap();
        let layout = h.state().session.project.layouts[li].clone();
        let (page, s) = page_rect(&h.state().layout, area.get(), &layout);
        let drag = |h: &mut egui_kittest::Harness<'_, RustGisApp>, from: Pos2, to: Pos2| {
            let button =
                |pos: Pos2, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            h.event(Event::PointerMoved(from));
            h.step();
            h.event(button(from, true));
            h.step();
            for i in 1..=6 {
                h.event(Event::PointerMoved(from + (to - from) * i as f32 / 6.0));
                h.step();
            }
            h.event(button(to, false));
            h.step();
        };
        let ruler = Pos2::new(page.center().x, area.get().top() + RULER / 2.0);
        let at = to_screen(page, s, (0.0, 40.0));
        drag(&mut h, ruler, Pos2::new(ruler.x, at.y));
        let y = h.state().session.project.layouts[li].guides.y.clone();
        assert_eq!(y.len(), 1, "a guide was added: {y:?}");
        assert!((y[0] - 40.0).abs() < 0.5, "at about 40 mm: {y:?}");
        drag(&mut h, Pos2::new(page.center().x, to_screen(page, s, (0.0, y[0])).y), ruler);
        assert!(h.state().session.project.layouts[li].guides.y.is_empty(), "dropped on the ruler, it's gone");
    }

    #[test]
    fn widening_a_hairline_keeps_it_thin() {
        let rule = (10.0, 30.0, 100.0, 0.15);
        assert_eq!(resized(rule, 3, 20.0, 5.0), (10.0, 30.0, 120.0, 0.15));
        assert_eq!(resized(rule, 7, -10.0, 0.0), (0.0, 30.0, 110.0, 0.15));
        // Corner and top/bottom handles size the height, at least 2 mm.
        assert_eq!(resized(rule, 4, 0.0, 0.5), (10.0, 30.0, 100.0, 2.0));
        assert_eq!(resized(rule, 5, 0.0, 0.5), (10.0, 30.0, 100.0, 2.0));
    }
}
