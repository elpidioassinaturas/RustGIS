//! L3: the CPU map renderer.
//!
//! [`render`] draws a [`Map`] (basemap, graticule, layers, selection) into a premultiplied RGBA
//! `tiny_skia::Pixmap` and places labels without collisions. The desktop app shows the pixmap
//! as a texture and draws the placed labels with its own text engine; [`render_png`] rasterises
//! the labels too, for export and the command line.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod effects;
pub mod fonts;
pub mod gpu_scene;
pub mod layout;
pub mod logo;
mod mesh;
pub mod pdf;
mod picture;
pub mod post;
mod raster;
pub mod report;
pub mod terrain;
mod text;
pub mod tile_mesh;
mod tiles;
pub use tiles::{Live, live as live_tiles};

pub use rustgis_tiles;
pub use tiles::{spec as tile_spec, style_view};

/// Natural Earth 1:110m sample data (public domain), fetched by `build.rs` from rustgis-assets.
pub mod sample_data {
    pub const COUNTRIES: &str = include_str!(concat!(env!("OUT_DIR"), "/countries.geojson"));
    /// The countries as China sees its borders (Natural Earth's China point of view).
    pub const COUNTRIES_CHN: &str = include_str!(concat!(env!("OUT_DIR"), "/countries_chn.geojson"));
    pub const CITIES: &str = include_str!(concat!(env!("OUT_DIR"), "/cities.geojson"));
    pub const RIVERS: &str = include_str!(concat!(env!("OUT_DIR"), "/rivers.geojson"));
    pub const LAKES: &str = include_str!(concat!(env!("OUT_DIR"), "/lakes.geojson"));

    use std::sync::atomic::{AtomicU8, Ordering};

    /// Whose view of disputed borders the built-in countries and world outline show.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum Worldview {
        /// Natural Earth's de facto borders.
        #[default]
        Default,
        /// China's view (required for maps shown in China).
        China,
    }

    impl Worldview {
        pub fn code(self) -> &'static str {
            match self {
                Worldview::Default => "default",
                Worldview::China => "china",
            }
        }

        pub fn from_code(code: &str) -> Option<Worldview> {
            match code.trim().to_ascii_lowercase().as_str() {
                "default" => Some(Worldview::Default),
                "china" | "chn" | "cn" => Some(Worldview::China),
                _ => None,
            }
        }

        /// The worldview for a system in time zone `tz` (IANA, such as `Asia/Shanghai`) whose
        /// preferred locales are `locales` (such as `zh-CN`, `zh_CN.UTF-8`): China when the time
        /// zone is one of mainland China's or the first locale is mainland Chinese.
        pub fn detect<'a>(tz: Option<&str>, locales: impl IntoIterator<Item = &'a str>) -> Worldview {
            let china_tz = tz.is_some_and(|tz| {
                matches!(
                    tz.trim(),
                    "Asia/Shanghai" | "Asia/Chongqing" | "Asia/Chungking" | "Asia/Harbin" | "Asia/Urumqi" | "Asia/Kashgar" | "PRC"
                )
            });
            let china_locale = locales.into_iter().next().is_some_and(|tag| {
                let base = tag.split(['.', '@']).next().unwrap_or("").replace('_', "-").to_ascii_lowercase();
                let mut parts = base.split('-');
                parts.next() == Some("zh") && parts.any(|p| p == "cn")
            });
            if china_tz || china_locale { Worldview::China } else { Worldview::Default }
        }
    }

    static WORLDVIEW: AtomicU8 = AtomicU8::new(0);

    /// Sets the worldview of the countries sample and the basemap's world outline.
    pub fn set_worldview(w: Worldview) {
        WORLDVIEW.store(w as u8, Ordering::Relaxed);
    }

    pub fn worldview() -> Worldview {
        if WORLDVIEW.load(Ordering::Relaxed) == Worldview::China as u8 { Worldview::China } else { Worldview::Default }
    }

    /// The countries GeoJSON in the current worldview.
    pub fn countries() -> &'static str {
        match worldview() {
            Worldview::Default => COUNTRIES,
            Worldview::China => COUNTRIES_CHN,
        }
    }
}

use std::sync::OnceLock;

use rustgis_data::{Dataset, Geometry, GeometryType};
use rustgis_geom::{Coord, Extent, extent_to_merc, lonlat_to_merc};
use rustgis_map::globe::{V3, densify};
use rustgis_map::layout::TextAlign;
use rustgis_map::{Camera, Globe, LineStroke, Map, Marker, Rgba, Symbol};
/// The image type pages and maps are drawn into.
pub use tiny_skia::Pixmap;
use tiny_skia::{
    FillRule, GradientStop, LineCap, LineJoin, Paint, PathBuilder, PixmapPaint, Point, RadialGradient, SpreadMode, Stroke, Transform,
};

/// Selection highlight (cyan, as desktop GIS users expect).
pub const SELECTION: Rgba = Rgba::rgb(0x00, 0xE5, 0xFF);

/// Largest raster rendered (pixels per side).
pub const MAX_SIDE: u32 = 8192;

/// How far past each side of the view (a fraction of its size) features are still drawn.
pub const CULL_MARGIN: f64 = 0.05;

/// The wider margin the GPU backend tessellates vector layers out to: while the map moves,
/// its meshes are moved rather than rebuilt, so they hold the ground a pan of half the view
/// or zooming out to twice the extent reveals (as MapLibre keeps the tiles around the view).
pub const GPU_CULL_MARGIN: f64 = 0.5;

/// The map extent a frame drawn for `view` holds features for: the view grown by `margin`
/// ([`CULL_MARGIN`], or [`GPU_CULL_MARGIN`] for GPU meshes) on every side.
pub fn cull_extent(view: &View, margin: f64) -> Extent {
    view.camera.extent(view.width, view.height).padded(margin, 0.0)
}

/// What to draw and at which size.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Size in logical points.
    pub width: f64,
    pub height: f64,
    /// Physical pixels per logical point.
    pub pixels_per_point: f64,
    pub camera: Camera,
    /// The interactive view: the globe sits in dark, starry space. Exports and layouts leave it
    /// off and get a light space that suits paper (a dark one only on the Midnight basemap).
    pub sky: bool,
}

impl View {
    pub fn pixel_size(&self) -> (u32, u32) {
        let w = (self.width * self.pixels_per_point).round().clamp(1.0, MAX_SIDE as f64) as u32;
        let h = (self.height * self.pixels_per_point).round().clamp(1.0, MAX_SIDE as f64) as u32;
        (w, h)
    }
    /// Logical screen position of a Web Mercator point.
    pub fn to_screen(&self, p: Coord) -> (f64, f64) {
        self.camera.to_screen(p, self.width, self.height)
    }
}

/// A placed label in logical points (centre of the text).
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedLabel {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub color: Rgba,
    pub halo: Option<Rgba>,
    pub bold: bool,
    /// An icon placed with the label (a tile style's `icon-image`), drawn into the map.
    pub icon: Option<PlacedIcon>,
    /// Which part of the text is at `x`: 0 its centre, -0.5 its left edge, 0.5 its right edge.
    /// Each renderer lines the text up with its own measure of the width, so text set beside an
    /// icon keeps clear of it whatever the font.
    pub align: f32,
    /// Framed in the selection colour: a selected text graphic. Each renderer frames the text as
    /// it lays it out.
    pub framed: bool,
    /// Slanted, and the letter spacing in points: a text graphic's (#535); other labels are
    /// upright with no spacing.
    pub italic: bool,
    pub spacing: f32,
    /// How the lines line up in their block (a text graphic's, #535); the block itself sits where
    /// `align` puts it.
    pub line_align: TextAlign,
    /// Turned with the map (a tile label along a street): the text's angle in radians,
    /// clockwise, about its centre (`align` is then 0), kept upright. `None` is upright on the
    /// screen.
    pub angle: Option<f32>,
    /// How it collides with other labels (padding, optional text or icon, overlaps).
    pub collide: rustgis_tiles::Collide,
}

impl PlacedLabel {
    /// The centre of the text, by this crate's measure of its width.
    pub fn center_x(&self) -> f32 {
        self.x - self.align * text::label_width(self)
    }
}

/// An icon in logical points: its picture and centre.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedIcon {
    pub image: std::sync::Arc<Pixmap>,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Turned with the map (an arrow along a street), in radians clockwise; `None` is upright.
    pub angle: Option<f32>,
}

pub struct Rendered {
    pub pixmap: Pixmap,
    pub labels: Vec<PlacedLabel>,
    /// Labels that didn't fit, when the map shows unplaced labels (drawn in red).
    pub unplaced: Vec<PlacedLabel>,
    /// The credits the map's tile layers ask for.
    pub attribution: Vec<String>,
    /// Tiles are still on their way: drawing again later shows more.
    pub tiles_pending: bool,
    /// Streamed layers are still reading the view: drawing again later shows them.
    pub data_pending: bool,
    /// Tile layers that couldn't be drawn, and why.
    pub tile_errors: Vec<String>,
}

/// Which rasteriser [`render_backend`] targets: the tiny-skia CPU pixmap (the default, and the
/// only path for exports, the globe and effect-heavy frames) or a lyon-tessellated GPU scene
/// drawn by the UI inside egui's wgpu pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Backend {
    #[default]
    Cpu,
    Gpu,
    /// A GPU scene whose raster tile layers, on a flat north-up map, are left for the GPU map to
    /// place for every frame it shows ([`gpu_scene::DrawCmd::LiveTiles`], #635).
    GpuLive,
}

impl Backend {
    /// Whether this is a GPU scene.
    pub fn is_gpu(self) -> bool {
        matches!(self, Backend::Gpu | Backend::GpuLive)
    }
}

/// A finished render in either backend.
pub enum RenderOutput {
    Cpu(Rendered),
    Gpu(gpu_scene::GpuScene),
}

/// Renders the map for the screen in the chosen backend. `Backend::Cpu` is exactly
/// [`render`]; `Backend::Gpu` draws what it can as tessellated meshes and everything else
/// (basemap, tiles, rasters, masked/blended/transparent layers) as CPU-rasterized images in
/// the same z-order.
pub fn render_backend(map: &mut Map, view: &View, backend: Backend) -> Option<RenderOutput> {
    render_with_backend(map, view, false, backend)
}

/// Decodes PNG bytes into a pixmap.
pub fn decode_png(png: &[u8]) -> Option<Pixmap> {
    Pixmap::decode_png(png).ok()
}

/// The pixel size of a base64 PNG or JPEG picture, `None` when it can't be decoded.
pub fn picture_size(b64: &str) -> Option<(u32, u32)> {
    picture::size(b64)
}

/// The colour unplaced labels are drawn in.
pub const UNPLACED: Rgba = Rgba::rgb(0xD0, 0x1C, 0x1C);

/// The distance (metres) a viewer of a flat map at a ground resolution would see it from: the
/// view's height seen through a 60° field of view.
pub fn viewing_distance(ground_res: f64, view_height: f64) -> f64 {
    ground_res * view_height * 0.866
}

/// The built-in world outline used by every basemap (Natural Earth 1:110m, public domain), in the
/// current worldview.
fn world() -> &'static Vec<Geometry> {
    static W: [OnceLock<Vec<Geometry>>; 2] = [OnceLock::new(), OnceLock::new()];
    let wv = sample_data::worldview();
    let cell = &W[wv as usize];
    cell.get_or_init(|| match rustgis_data::io::read_geojson(sample_data::countries(), "world") {
        Ok((ds, _)) => ds.features.into_iter().filter_map(|f| f.geometry).map(|g| g.map_coords(&lonlat_to_merc)).collect(),
        Err(e) => {
            log::warn!("built-in world outline unreadable: {e}");
            Vec::new()
        }
    })
}

/// The built-in world outline in longitude/latitude, for the globe.
fn world_lonlat() -> &'static Vec<Geometry> {
    static W: [OnceLock<Vec<Geometry>>; 2] = [OnceLock::new(), OnceLock::new()];
    let cell = &W[sample_data::worldview() as usize];
    cell.get_or_init(|| world().iter().map(|g| g.map_coords(&rustgis_geom::merc_to_lonlat)).collect())
}

thread_local! {
    /// The world outline in each custom projection used so far.
    static WORLD_IN: std::cell::RefCell<std::collections::HashMap<String, std::rc::Rc<Vec<Geometry>>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The built-in world outline in a custom projection's coordinates (kept per projection).
fn world_in(flat: &rustgis_map::projection::Flat, key: &str) -> std::rc::Rc<Vec<Geometry>> {
    WORLD_IN.with(|cell| {
        cell.borrow_mut()
            .entry(format!("{}:{key}", sample_data::worldview().code()))
            .or_insert_with(|| std::rc::Rc::new(world_lonlat().iter().map(|g| flat.project(g)).collect()))
            .clone()
    })
}

/// Largest angle between globe vertices before edges are bent along great circles (2°).
const GLOBE_STEP: f64 = 0.035;

/// The built-in world dataset (lon/lat), for the sample project.
pub fn world_dataset(name: &str, text: &str) -> Option<Dataset> {
    rustgis_data::io::read_geojson(text, name).ok().map(|(d, _)| d)
}

thread_local! {
    /// Antialiasing for the map being drawn (off in the fast display type).
    static ANTIALIAS: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Sets antialiasing for one render and puts it back on when dropped, however the render ends.
struct Antialias;

impl Antialias {
    fn set(on: bool) -> Antialias {
        ANTIALIAS.with(|a| a.set(on));
        Antialias
    }
}

impl Drop for Antialias {
    fn drop(&mut self) {
        ANTIALIAS.with(|a| a.set(true));
    }
}

fn paint(c: Rgba, opacity: f32) -> Paint<'static> {
    let mut p = Paint::default();
    let a = (c.a() as f32 * opacity).round().clamp(0.0, 255.0) as u8;
    p.set_color_rgba8(c.r(), c.g(), c.b(), a);
    p.anti_alias = ANTIALIAS.with(|a| a.get());
    p
}

struct Ctx<'a> {
    view: &'a View,
    ppp: f32,
    /// Polygons drawn as outlines only.
    wireframe: bool,
    /// Visible Mercator extent with a margin, for culling.
    cull: Extent,
    /// Set when drawing the globe; coordinates are then longitude/latitude, not Web Mercator.
    globe: Option<Globe>,
    /// Point marker sprites drawn so far, by symbol and quarter-pixel offset.
    sprites: std::cell::RefCell<std::collections::HashMap<SpriteKey, Option<Pixmap>>>,
}

/// A marker sprite's symbol and quarter-pixel offset.
type SpriteKey = (Rgba, Rgba, u32, u32, Marker, u8, u8);

impl Ctx<'_> {
    /// Logical screen position of a coordinate, `None` when it is behind the globe.
    fn screen(&self, c: Coord) -> Option<(f64, f64)> {
        match &self.globe {
            Some(g) => g.forward(c),
            None => Some(self.view.to_screen(c)),
        }
    }

    fn px(&self, c: Coord) -> (f32, f32) {
        match self.screen(c) {
            Some((x, y)) => ((x * self.view.pixels_per_point) as f32, (y * self.view.pixels_per_point) as f32),
            None => (f32::NAN, f32::NAN),
        }
    }

    fn vpx(&self, g: &Globe, v: V3) -> (f32, f32) {
        let (x, y) = g.project(v);
        ((x * self.view.pixels_per_point) as f32, (y * self.view.pixels_per_point) as f32)
    }

    /// View vectors of a ring or line, with long edges bent along great circles.
    fn globe_vectors(g: &Globe, pts: &[Coord]) -> Vec<V3> {
        let mut out: Vec<V3> = Vec::with_capacity(pts.len());
        for c in pts {
            if !c.is_finite() {
                continue;
            }
            let v = g.to_view(*c);
            match out.last().copied() {
                Some(prev) => out.extend(densify(prev, v, GLOBE_STEP)),
                None => out.push(v),
            }
        }
        out
    }

    /// A polygon ring on the globe. Hidden vertices are moved radially onto the horizon circle at
    /// their own bearing, so the hidden part of the ring runs along the limb in its true order and
    /// encloses no area; a ring that is wholly hidden is skipped.
    fn globe_ring(&self, g: &Globe, pb: &mut PathBuilder, ring: &[Coord]) {
        let vs = Self::globe_vectors(g, ring);
        let h = g.horizon();
        if vs.len() < 3 || vs.iter().all(|v| v[2] <= h) {
            return;
        }
        let r = (1.0 - h * h).sqrt();
        let limb = |v: V3| {
            let a = v[1].atan2(v[0]);
            [r * a.cos(), r * a.sin(), h]
        };
        let mut out: Vec<V3> = Vec::with_capacity(vs.len() + 8);
        let push_limb = |out: &mut Vec<V3>, p: V3| {
            // Follow the limb between hidden points that are far apart in bearing.
            if let Some(last) = out.last().copied()
                && last[2] <= h + 1e-9
            {
                out.extend(g.horizon_arc(last, p));
            }
            out.push(p);
        };
        let mut prev: Option<V3> = None;
        for v in vs {
            let vis = v[2] > h;
            if let Some(p) = prev
                && (p[2] > h) != vis
            {
                push_limb(&mut out, g.horizon_crossing(p, v));
            }
            if vis {
                out.push(v)
            } else {
                push_limb(&mut out, limb(v))
            }
            prev = Some(v);
        }
        // A ring that ends and starts behind the horizon closes along the limb, not across it.
        if let (Some(first), Some(last)) = (out.first().copied(), out.last().copied())
            && first[2] <= h + 1e-9
            && last[2] <= h + 1e-9
        {
            out.extend(g.horizon_arc(last, first));
        }
        for (i, v) in out.iter().enumerate() {
            let (x, y) = self.vpx(g, *v);
            if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
        }
        if !out.is_empty() {
            pb.close();
        }
    }

    /// A line on the globe, broken where it goes behind the horizon.
    fn globe_line(&self, g: &Globe, pb: &mut PathBuilder, part: &[Coord]) {
        let vs = Self::globe_vectors(g, part);
        let h = g.horizon();
        let mut prev: Option<V3> = None;
        for v in vs {
            let vis = v[2] > h;
            match prev {
                Some(p) if p[2] > h && vis => {
                    let (x, y) = self.vpx(g, v);
                    pb.line_to(x, y);
                }
                Some(p) if p[2] > h => {
                    let (x, y) = self.vpx(g, g.horizon_crossing(p, v));
                    pb.line_to(x, y);
                }
                Some(p) if vis => {
                    let (x, y) = self.vpx(g, g.horizon_crossing(p, v));
                    pb.move_to(x, y);
                    let (x, y) = self.vpx(g, v);
                    pb.line_to(x, y);
                }
                None if vis => {
                    let (x, y) = self.vpx(g, v);
                    pb.move_to(x, y);
                }
                _ => {}
            }
            prev = Some(v);
        }
    }

    fn ring_path(&self, pb: &mut PathBuilder, ring: &[Coord], close: bool) {
        if let Some(g) = &self.globe {
            if close {
                self.globe_ring(g, pb, ring)
            } else {
                self.globe_line(g, pb, ring)
            }
            return;
        }
        let ppp = self.view.pixels_per_point;
        let dev = |c: &Coord| {
            let (x, y) = self.view.to_screen(*c);
            (x * ppp, y * ppp)
        };
        // Each run of points the projection can show; a point it can't breaks the line rather
        // than being bridged.
        let clip = self.clip_box();
        for run in ring.split(|c| !c.is_finite()) {
            let mut b = Extent::EMPTY;
            for c in run {
                let (x, y) = dev(c);
                if x.is_finite() && y.is_finite() {
                    b.expand(Coord::new(x, y));
                }
            }
            if b.is_empty() || !b.intersects(&clip) {
                continue; // wholly off the image
            }
            if clip.contains(Coord::new(b.min_x, b.min_y)) && clip.contains(Coord::new(b.max_x, b.max_y)) {
                emit_run(pb, run.iter().map(dev), close);
            } else if close {
                // Zoomed in on a big polygon: its far vertices would be millions of pixels away,
                // which the rasteriser can't take (and needn't walk). Clipping each ring to a box
                // well beyond the image keeps the even-odd fill the same inside it.
                let pts: Vec<(f64, f64)> = run.iter().map(dev).collect();
                emit_run(pb, clip_ring(&pts, &clip).into_iter(), true);
            } else {
                let pts: Vec<(f64, f64)> = run.iter().map(dev).collect();
                for part in clip_line(&pts, &clip) {
                    emit_run(pb, part.into_iter(), false);
                }
            }
        }
    }

    /// The box, in device pixels, paths are clipped to: the image with a wide margin, so a cut
    /// edge and its stroke stay out of sight.
    fn clip_box(&self) -> Extent {
        let (w, h) = self.view.pixel_size();
        let (w, h) = (w as f64, h as f64);
        let m = w.max(h) / 2.0 + 64.0;
        Extent { min_x: -m, min_y: -m, max_x: w + m, max_y: h + m }
    }

    fn polygon(&self, pm: &mut Pixmap, polys: &[Vec<Vec<Coord>>], fill: Option<Paint>, stroke: Option<(Paint, f32)>) {
        let mut pb = PathBuilder::new();
        for rings in polys {
            for r in rings {
                self.ring_path(&mut pb, r, true);
            }
        }
        let Some(path) = pb.finish() else { return };
        if let Some(f) = fill {
            pm.fill_path(&path, &f, FillRule::EvenOdd, Transform::identity(), None);
        }
        if let Some((s, w)) = stroke
            && w > 0.0
        {
            let st = Stroke { width: w * self.ppp, line_join: LineJoin::Round, ..Stroke::default() };
            pm.stroke_path(&path, &s, &st, Transform::identity(), None);
        }
    }

    fn line(&self, pm: &mut Pixmap, parts: &[Vec<Coord>], p: &Paint, w: f32) {
        let mut pb = PathBuilder::new();
        for part in parts {
            self.ring_path(&mut pb, part, false);
        }
        let Some(path) = pb.finish() else { return };
        let st = Stroke { width: (w * self.ppp).max(0.5), line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
        pm.stroke_path(&path, p, &st, Transform::identity(), None);
    }

    /// A line drawn as a layer's stacked strokes (`Layer::line_strokes`), bottom first, each
    /// shifted sideways by its offset.
    fn stacked_line(&self, pm: &mut Pixmap, parts: &[Vec<Coord>], s: &Symbol, strokes: &[LineStroke]) {
        let mut pb = PathBuilder::new();
        for part in parts {
            self.ring_path(&mut pb, part, false);
        }
        let Some(path) = pb.finish() else { return };
        // Shifted once per offset: a casing and its core usually share one.
        let mut shifted: Vec<(f32, Option<tiny_skia::Path>)> = Vec::new();
        for st in strokes {
            let (c, w) = st.resolve(s);
            if w.is_nan() || w <= 0.0 || c.a() == 0 {
                continue;
            }
            if st.offset != 0.0 && !shifted.iter().any(|(o, _)| *o == st.offset) {
                shifted.push((st.offset, offset_path(&path, st.offset * self.ppp)));
            }
            let drawn = match st.offset {
                0.0 => Some(&path),
                o => shifted.iter().find(|(x, _)| *x == o).and_then(|(_, p)| p.as_ref()),
            };
            let Some(drawn) = drawn else { continue };
            let stroke = Stroke {
                width: (w * self.ppp).max(0.5),
                line_cap: match st.cap {
                    rustgis_map::LineCap::Round => LineCap::Round,
                    rustgis_map::LineCap::Butt => LineCap::Butt,
                    rustgis_map::LineCap::Square => LineCap::Square,
                },
                line_join: match st.join {
                    rustgis_map::LineJoin::Round => LineJoin::Round,
                    rustgis_map::LineJoin::Miter => LineJoin::Miter,
                    rustgis_map::LineJoin::Bevel => LineJoin::Bevel,
                },
                ..Stroke::default()
            };
            pm.stroke_path(drawn, &paint(c, 1.0), &stroke, Transform::identity(), None);
        }
    }

    fn marker(&self, pm: &mut Pixmap, c: Coord, s: &Symbol, opacity: f32, halo: Option<Rgba>) {
        let (x, y) = self.px(c);
        if !x.is_finite() || !y.is_finite() {
            return; // behind the globe
        }
        // Off the image by more than any marker's reach: nothing to draw.
        let reach = (s.size.max(1.0) * 0.6 + s.stroke_width.max(0.0) + 4.0) * self.ppp;
        if x < -reach || y < -reach || x > pm.width() as f32 + reach || y > pm.height() as f32 + reach {
            return;
        }
        draw_marker(pm, x, y, s, opacity, halo, self.ppp);
    }

    /// A layer's point marker, stamped from a sprite drawn once per symbol and quarter-pixel
    /// offset: a dense point layer otherwise fills and strokes a path for every point.
    fn stamp(&self, pm: &mut Pixmap, c: Coord, s: &Symbol) {
        let (x, y) = self.px(c);
        if !x.is_finite() || !y.is_finite() {
            return; // behind the globe
        }
        let (fx, fy) = (x.floor(), y.floor());
        let (qx, qy) = (((x - fx) * 4.0).round(), ((y - fy) * 4.0).round());
        let (ix, iy) = (fx as i64 + (qx >= 4.0) as i64, fy as i64 + (qy >= 4.0) as i64);
        let (qx, qy) = ((qx as u8) % 4, (qy as u8) % 4);
        let reach = ((s.size.max(1.0) * 0.6 + s.stroke_width.max(0.0)) * self.ppp).ceil() as i64 + 3;
        if ix < -reach || iy < -reach || ix > pm.width() as i64 + reach || iy > pm.height() as i64 + reach {
            return;
        }
        let key = (s.fill, s.stroke, s.stroke_width.to_bits(), s.size.to_bits(), s.marker, qx, qy);
        let mut sprites = self.sprites.borrow_mut();
        if !sprites.contains_key(&key) {
            // Many symbols (sizes by value, say) would each make a sprite used once: draw directly.
            if sprites.len() >= MAX_SPRITES || reach > MAX_SPRITE_REACH {
                drop(sprites);
                draw_marker(pm, x, y, s, 1.0, None, self.ppp);
                return;
            }
            let side = (2 * reach + 1) as u32;
            let sprite = Pixmap::new(side, side).map(|mut sp| {
                let c = reach as f32 + qx as f32 / 4.0;
                let d = reach as f32 + qy as f32 / 4.0;
                draw_marker(&mut sp, c, d, s, 1.0, None, self.ppp);
                sp
            });
            sprites.insert(key, sprite);
        }
        if let Some(Some(sp)) = sprites.get(&key) {
            blend_over(pm, sp, ix - reach, iy - reach);
        }
    }

    fn feature(&self, pm: &mut Pixmap, g: &Geometry, s: &Symbol) {
        match g {
            Geometry::Point(c) => self.stamp(pm, *c, s),
            Geometry::MultiPoint(v) => v.iter().for_each(|c| self.stamp(pm, *c, s)),
            Geometry::Line(parts) => self.line(pm, parts, &paint(s.stroke, 1.0), s.stroke_width),
            Geometry::Polygon(polys) if self.wireframe => {
                self.polygon(pm, polys, None, Some((paint(s.stroke.with_alpha(255), 1.0), s.stroke_width.max(1.0))))
            }
            Geometry::Polygon(polys) => self.polygon(pm, polys, Some(paint(s.fill, 1.0)), Some((paint(s.stroke, 1.0), s.stroke_width))),
        }
    }

    /// Draws a line or polygon smaller than a device pixel across as the coverage its fill and
    /// outline would have, spread over the pixels around its centre. `false` (nothing drawn) for
    /// a bigger feature or a point.
    fn splat(&self, pm: &mut Pixmap, e: &Extent, g: &Geometry, s: &Symbol) -> bool {
        let k = self.view.pixels_per_point / self.view.camera.res.max(1e-12);
        let (sx, sy) = ((e.width() * k) as f32, (e.height() * k) as f32);
        if !(sx.is_finite() && sy.is_finite()) || sx > 1.0 || sy > 1.0 || e.is_empty() {
            return false;
        }
        let (stroke, width, fill) = match g {
            Geometry::Point(_) | Geometry::MultiPoint(_) => return false,
            Geometry::Line(_) => (s.stroke, (s.stroke_width * self.ppp).max(0.5), None),
            Geometry::Polygon(_) if self.wireframe => (s.stroke.with_alpha(255), s.stroke_width.max(1.0) * self.ppp, None),
            Geometry::Polygon(_) => (s.stroke, if s.stroke.a() > 0 { s.stroke_width.max(0.0) * self.ppp } else { 0.0 }, Some(s.fill)),
        };
        // With its outline, still about a pixel: a thick outline drawn as a dot would lose its ink.
        if sx + width > 2.0 || sy + width > 2.0 {
            return false;
        }
        let (cx, cy) = self.px(Coord::new((e.min_x + e.max_x) / 2.0, (e.min_y + e.max_y) / 2.0));
        // The outline covers the shape grown by half its width all round (round joins); the
        // fill shows inside it.
        let inner = (sx - width).max(0.0) * (sy - width).max(0.0);
        let outer = if width > 0.0 {
            let hw = width / 2.0;
            match g {
                Geometry::Line(_) => (sx.hypot(sy) + width) * width,
                _ => (sx + width) * (sy + width) - (4.0 - std::f32::consts::PI) * hw * hw,
            }
        } else {
            0.0
        };
        if let Some(f) = fill {
            let area = if width > 0.0 { inner } else { sx * sy };
            splat_color(pm, cx, cy, f, area);
        }
        if width > 0.0 {
            splat_color(pm, cx, cy, stroke, outer - inner);
        }
        true
    }

    fn highlight(&self, pm: &mut Pixmap, g: &Geometry, s: &Symbol, strokes: &[LineStroke]) {
        let cyan = paint(SELECTION, 1.0);
        match g {
            Geometry::Line(parts) if !strokes.is_empty() => {
                // A halo around each stroke that shows, under them all.
                let halo: Vec<LineStroke> = strokes
                    .iter()
                    .filter(|st| {
                        let (c, w) = st.resolve(s);
                        w > 0.0 && c.a() > 0
                    })
                    .map(|st| LineStroke { color: Some(SELECTION), width: Some(st.resolve(s).1 + 3.0), ..*st })
                    .collect();
                self.stacked_line(pm, parts, s, &halo);
                self.stacked_line(pm, parts, s, strokes);
            }
            Geometry::Point(c) => self.marker(pm, *c, s, 1.0, Some(SELECTION)),
            Geometry::MultiPoint(v) => v.iter().for_each(|c| self.marker(pm, *c, s, 1.0, Some(SELECTION))),
            Geometry::Line(parts) => {
                self.line(pm, parts, &cyan, s.stroke_width + 3.0);
                self.line(pm, parts, &paint(s.stroke, 1.0), s.stroke_width);
            }
            Geometry::Polygon(polys) => self.polygon(pm, polys, Some(paint(SELECTION.with_alpha(40), 1.0)), Some((cyan, 2.0))),
        }
    }
}

/// `path`'s polylines shifted `d` pixels to the right of their direction (screen y down).
fn offset_path(path: &tiny_skia::Path, d: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let mut run: Vec<(f32, f32)> = Vec::new();
    let mut flush = |run: &mut Vec<(f32, f32)>| {
        let pts = offset_polyline(run, d);
        if let Some((first, rest)) = pts.split_first() {
            pb.move_to(first.0, first.1);
            for p in rest {
                pb.line_to(p.0, p.1);
            }
        }
        run.clear();
    };
    for seg in path.segments() {
        match seg {
            tiny_skia::PathSegment::MoveTo(p) => {
                flush(&mut run);
                run.push((p.x, p.y));
            }
            tiny_skia::PathSegment::LineTo(p) | tiny_skia::PathSegment::QuadTo(_, p) | tiny_skia::PathSegment::CubicTo(_, _, p) => {
                run.push((p.x, p.y))
            }
            tiny_skia::PathSegment::Close => {}
        }
    }
    flush(&mut run);
    pb.finish()
}

/// A polyline shifted `d` to the right of its direction (screen y down; negative `d` shifts
/// it left). Corners are mitred, and cut back to a bevel where the mitre would reach beyond
/// four times the offset (a sharp turn). Repeated points are skipped; fewer than two distinct
/// points give nothing.
/// Like any offset that moves each vertex on its own, an offset larger than the segments on the
/// inside of a bend can fold back into a small loop there.
fn offset_polyline(pts: &[(f32, f32)], d: f32) -> Vec<(f32, f32)> {
    let mut p: Vec<(f32, f32)> = Vec::with_capacity(pts.len());
    for &q in pts.iter().filter(|q| q.0.is_finite() && q.1.is_finite()) {
        if p.last().is_none_or(|l: &(f32, f32)| (l.0 - q.0).hypot(l.1 - q.1) > 1e-3) {
            p.push(q);
        }
    }
    // The unit normal to the right of a → b.
    let normal = |a: (f32, f32), b: (f32, f32)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l = dx.hypot(dy);
        (-dy / l, dx / l)
    };
    let mut out = Vec::with_capacity(p.len() + 4);
    let (Some(&[a, b]), Some(&[y, z])) = (p.first_chunk::<2>(), p.last_chunk::<2>()) else { return out };
    let n = normal(a, b);
    out.push((a.0 + n.0 * d, a.1 + n.1 * d));
    for w in p.windows(3) {
        let [a, v, b] = [w.first(), w.get(1), w.get(2)].map(|x| x.copied().unwrap_or_default());
        let (na, nb) = (normal(a, v), normal(v, b));
        let (sx, sy) = (na.0 + nb.0, na.1 + nb.1);
        let s2 = sx * sx + sy * sy;
        if s2 >= 0.25 {
            // The mitre: along the bisector, 2d / |na + nb|² times na + nb.
            let k = 2.0 * d / s2;
            out.push((v.0 + sx * k, v.1 + sy * k));
        } else {
            out.push((v.0 + na.0 * d, v.1 + na.1 * d));
            out.push((v.0 + nb.0 * d, v.1 + nb.1 * d));
        }
    }
    let n = normal(y, z);
    out.push((z.0 + n.0 * d, z.1 + n.1 * d));
    out
}

/// Adds one run of device-pixel points to a path, dropping points closer than ~0.35 px to the
/// last one kept (a big speed-up zoomed out), but always the last.
fn emit_run(pb: &mut PathBuilder, pts: impl ExactSizeIterator<Item = (f64, f64)>, close: bool) {
    let n = pts.len();
    let mut last: Option<(f32, f32)> = None;
    for (i, (x, y)) in pts.enumerate() {
        let (x, y) = (x as f32, y as f32);
        if !x.is_finite() || !y.is_finite() {
            continue;
        }
        match last {
            None => pb.move_to(x, y),
            Some((lx, ly)) => {
                if (x - lx).abs() + (y - ly).abs() < 0.35 && i + 1 != n {
                    continue;
                }
                pb.line_to(x, y)
            }
        }
        last = Some((x, y));
    }
    if close && last.is_some() {
        pb.close();
    }
}

/// A ring clipped to a box (Sutherland–Hodgman): the part inside it, closed along its edges.
fn clip_ring(pts: &[(f64, f64)], b: &Extent) -> Vec<(f64, f64)> {
    let mut cur: Vec<(f64, f64)> = pts.to_vec();
    // Each edge: is a point inside, and where a segment crosses it.
    for edge in 0..4 {
        if cur.is_empty() {
            break;
        }
        let inside = |p: (f64, f64)| match edge {
            0 => p.0 >= b.min_x,
            1 => p.0 <= b.max_x,
            2 => p.1 >= b.min_y,
            _ => p.1 <= b.max_y,
        };
        let cross = |p: (f64, f64), q: (f64, f64)| {
            let (v, t) = match edge {
                0 => (b.min_x, (b.min_x - p.0) / (q.0 - p.0)),
                1 => (b.max_x, (b.max_x - p.0) / (q.0 - p.0)),
                2 => (b.min_y, (b.min_y - p.1) / (q.1 - p.1)),
                _ => (b.max_y, (b.max_y - p.1) / (q.1 - p.1)),
            };
            if edge < 2 { (v, p.1 + (q.1 - p.1) * t) } else { (p.0 + (q.0 - p.0) * t, v) }
        };
        let mut out = Vec::with_capacity(cur.len() + 4);
        let mut prev = cur.last().copied().unwrap_or((0.0, 0.0));
        for &p in &cur {
            match (inside(prev), inside(p)) {
                (true, true) => out.push(p),
                (true, false) => out.push(cross(prev, p)),
                (false, true) => {
                    out.push(cross(prev, p));
                    out.push(p);
                }
                (false, false) => {}
            }
            prev = p;
        }
        cur = out;
    }
    if cur.len() < 3 { Vec::new() } else { cur }
}

/// The parts of a line inside a box (each segment clipped by Liang–Barsky).
fn clip_line(pts: &[(f64, f64)], b: &Extent) -> Vec<Vec<(f64, f64)>> {
    let mut parts: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut cur: Vec<(f64, f64)> = Vec::new();
    for w in pts.windows(2) {
        let (Some(&p), Some(&q)) = (w.first(), w.get(1)) else { continue };
        let (dx, dy) = (q.0 - p.0, q.1 - p.1);
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        let mut keep = true;
        for (num, den) in [(p.0 - b.min_x, -dx), (b.max_x - p.0, dx), (p.1 - b.min_y, -dy), (b.max_y - p.1, dy)] {
            if den == 0.0 {
                if num < 0.0 {
                    keep = false;
                    break;
                }
            } else {
                let t = num / den;
                if den < 0.0 {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
            }
        }
        if !keep || t0 > t1 {
            if cur.len() > 1 {
                parts.push(std::mem::take(&mut cur));
            }
            cur.clear();
            continue;
        }
        let a = (p.0 + dx * t0, p.1 + dy * t0);
        let z = (p.0 + dx * t1, p.1 + dy * t1);
        if cur.is_empty() || t0 > 0.0 {
            if cur.len() > 1 {
                parts.push(std::mem::take(&mut cur));
            }
            cur = vec![a];
        }
        cur.push(z);
        if t1 < 1.0 {
            parts.push(std::mem::take(&mut cur));
        }
    }
    if cur.len() > 1 {
        parts.push(cur);
    }
    parts
}

/// Blends `color` at `coverage` (in pixels, spread bilinearly) around (`cx`, `cy`) device pixels.
fn splat_color(pm: &mut Pixmap, cx: f32, cy: f32, color: Rgba, coverage: f32) {
    if color.a() == 0 || coverage.is_nan() || coverage <= 0.0 || !cx.is_finite() || !cy.is_finite() {
        return;
    }
    let (x, y) = (cx - 0.5, cy - 0.5);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let (ix, iy) = (x.floor() as i64, y.floor() as i64);
    let (w, h) = (pm.width() as i64, pm.height() as i64);
    let data = pm.data_mut();
    for (dx, dy, wt) in [(0, 0, (1.0 - fx) * (1.0 - fy)), (1, 0, fx * (1.0 - fy)), (0, 1, (1.0 - fx) * fy), (1, 1, fx * fy)] {
        let (px, py) = (ix + dx, iy + dy);
        if px < 0 || py < 0 || px >= w || py >= h {
            continue;
        }
        let a = (color.a() as f32 * (coverage * wt).min(1.0)).round() as u16;
        if a == 0 {
            continue;
        }
        let i = ((py * w + px) * 4) as usize;
        let Some(d) = data.get_mut(i..i + 4) else { continue };
        let inv = 255 - a;
        let src = [color.r(), color.g(), color.b()];
        for (dc, sc) in d.iter_mut().zip(src) {
            *dc = ((sc as u16 * a + *dc as u16 * inv + 127) / 255) as u8;
        }
        d[3] = (a + (d[3] as u16 * inv + 127) / 255).min(255) as u8;
    }
}

/// Most marker sprites one render keeps.
const MAX_SPRITES: usize = 1024;

/// Largest marker reach (device pixels from its centre) drawn from a sprite: a bigger marker
/// gains little from one. With [`MAX_SPRITES`], the cache stays under about 10 MB.
const MAX_SPRITE_REACH: i64 = 24;

/// A marker centred at (`x`, `y`) device pixels.
fn draw_marker(pm: &mut Pixmap, x: f32, y: f32, s: &Symbol, opacity: f32, halo: Option<Rgba>, ppp: f32) {
    let r = (s.size.max(1.0) * ppp) / 2.0;
    let mut pb = PathBuilder::new();
    match s.marker {
        Marker::Circle => pb.push_circle(x, y, r),
        Marker::Square => {
            if let Some(rect) = tiny_skia::Rect::from_xywh(x - r * 0.9, y - r * 0.9, r * 1.8, r * 1.8) {
                pb.push_rect(rect)
            }
        }
        Marker::Triangle => {
            pb.move_to(x, y - r * 1.1);
            pb.line_to(x + r, y + r * 0.75);
            pb.line_to(x - r, y + r * 0.75);
            pb.close();
        }
        Marker::Diamond => {
            pb.move_to(x, y - r * 1.15);
            pb.line_to(x + r * 0.9, y);
            pb.line_to(x, y + r * 1.15);
            pb.line_to(x - r * 0.9, y);
            pb.close();
        }
        Marker::Star => {
            for i in 0..10 {
                let a = std::f32::consts::PI * (i as f32) / 5.0 - std::f32::consts::FRAC_PI_2;
                let rr = if i % 2 == 0 { r * 1.2 } else { r * 0.5 };
                let (px, py) = (x + rr * a.cos(), y + rr * a.sin());
                if i == 0 { pb.move_to(px, py) } else { pb.line_to(px, py) }
            }
            pb.close();
        }
    }
    let Some(path) = pb.finish() else { return };
    if let Some(h) = halo {
        let st = Stroke { width: 3.5 * ppp, line_join: LineJoin::Round, ..Stroke::default() };
        pm.stroke_path(&path, &paint(h, 1.0), &st, Transform::identity(), None);
    }
    pm.fill_path(&path, &paint(s.fill, opacity), FillRule::Winding, Transform::identity(), None);
    if s.stroke_width > 0.0 && s.stroke.a() > 0 {
        let st = Stroke { width: s.stroke_width * ppp, line_join: LineJoin::Round, ..Stroke::default() };
        pm.stroke_path(&path, &paint(s.stroke, opacity), &st, Transform::identity(), None);
    }
}

/// Composites premultiplied `src` over `dst` with its top left corner at (`x`, `y`).
fn blend_over(dst: &mut Pixmap, src: &Pixmap, x: i64, y: i64) {
    let (dw, dh) = (dst.width() as i64, dst.height() as i64);
    let (sw, sh) = (src.width() as i64, src.height() as i64);
    let (x0, x1) = (x.max(0), (x + sw).min(dw));
    let (y0, y1) = (y.max(0), (y + sh).min(dh));
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let (s, d) = (src.data(), dst.data_mut());
    let n = ((x1 - x0) * 4) as usize;
    for row in y0..y1 {
        let si = (((row - y) * sw + (x0 - x)) * 4) as usize;
        let di = ((row * dw + x0) * 4) as usize;
        let (Some(sr), Some(dr)) = (s.get(si..si + n), d.get_mut(di..di + n)) else { continue };
        for (sp, dp) in sr.as_chunks::<4>().0.iter().zip(dr.as_chunks_mut::<4>().0.iter_mut()) {
            let sa = sp[3];
            if sa == 0 {
                continue;
            }
            if sa == 255 {
                dp.copy_from_slice(sp);
                continue;
            }
            let inv = 255 - sa as u16;
            for (dc, sc) in dp.iter_mut().zip(sp) {
                *dc = (*sc as u16 + (*dc as u16 * inv + 127) / 255).min(255) as u8;
            }
        }
    }
}

/// The longitude/latitude extent a view shows (the whole world on the globe, or when the
/// projection can't say).
pub fn view_lonlat(map: &Map, view: &View) -> Extent {
    if map.projection.is_globe() {
        return Extent::WORLD_LONLAT;
    }
    let flat = map.flat();
    let e = view.camera.extent(view.width, view.height);
    let mut out = Extent::EMPTY;
    const N: usize = 8;
    for i in 0..=N {
        let t = i as f64 / N as f64;
        let (x, y) = (e.min_x + t * e.width(), e.min_y + t * e.height());
        for c in [Coord::new(x, e.min_y), Coord::new(x, e.max_y), Coord::new(e.min_x, y), Coord::new(e.max_x, y)] {
            match flat.inverse(c) {
                Some(p) if p.is_finite() => out.expand(p),
                _ => return Extent::WORLD_LONLAT,
            }
        }
    }
    // A pole inside the view lies beyond every edge sample (a polar projection): reach it.
    for pole in [Coord::new(0.0, 90.0), Coord::new(0.0, -90.0)] {
        if let Some(p) = flat.forward(pole)
            && p.is_finite()
            && e.contains(p)
        {
            out = Extent::new(-180.0, out.min_y.min(pole.y), 180.0, out.max_y.max(pole.y));
        }
    }
    if out.is_empty() { Extent::WORLD_LONLAT } else { out }
}

/// Draws the map for the screen. Layers are drawn bottom-up (the last layer in the list first).
/// On high-density screens rasters are computed at logical-pixel resolution, for speed.
pub fn render(map: &mut Map, view: &View) -> Option<Rendered> {
    render_with(map, view, false)
}

fn render_with(map: &mut Map, view: &View, full_res_rasters: bool) -> Option<Rendered> {
    match render_with_backend(map, view, full_res_rasters, Backend::Cpu) {
        Some(RenderOutput::Cpu(r)) => Some(r),
        // The Cpu backend never produces RenderOutput::Gpu.
        Some(RenderOutput::Gpu(_)) | None => None,
    }
}

/// Flushes the GPU pass's CPU accumulator pixmap as an Image draw command and resets it to
/// transparent. Basemap, tiles, rasters and tiny-skia-only layers draw into the accumulator
/// between GPU mesh commands, preserving the exact painter's-algorithm order.
pub(crate) fn flush_acc(pm: &mut Pixmap, cmds: &mut Vec<gpu_scene::DrawCmd>, dirty: &mut bool) -> Option<()> {
    if *dirty {
        // Move the pixmap into the command (no full-screen copy) and start a fresh transparent
        // accumulator.
        let old = std::mem::replace(pm, Pixmap::new(pm.width(), pm.height())?);
        cmds.push(gpu_scene::DrawCmd::Image { pixmap: old });
        *dirty = false;
    }
    Some(())
}

/// The shared render body behind [`render`], [`render_backend`] and the export paths.
/// `full_res_rasters` renders rasters at full resolution (exports); `backend` selects the CPU
/// pixmap or a GPU scene (see [`Backend`]).
fn render_with_backend(map: &mut Map, view: &View, full_res_rasters: bool, backend: Backend) -> Option<RenderOutput> {
    // A bearing or pitch set directly on the camera may be unnormalised (or no number, which
    // faces north and looks straight down).
    let mut camera = view.camera;
    camera.bearing = rustgis_map::camera::normalize_bearing(camera.bearing);
    camera.pitch = rustgis_map::camera::normalize_pitch(camera.pitch);
    // A persisted Shapefile has no chunk handle until it opens. Preflight required local
    // sources in every view so an open failure cannot become a successful incomplete export.
    let complete_layers: std::collections::HashSet<u32> = {
        let mut scales = vec![map.ground_res_at(&camera) / rustgis_geom::PIXEL_M];
        if camera.pitch != 0.0 && !map.projection.is_globe() {
            scales.extend(camera.tilt_bands(view.width, view.height).iter().map(|b| map.ground_res_at(&b.camera) / rustgis_geom::PIXEL_M));
        }
        map.layers
            .iter()
            .filter(|l| scales.iter().any(|scale| map.draws_at(l, *scale)))
            .flat_map(|l| {
                let mask = l
                    .mask
                    .as_ref()
                    .and_then(|name| {
                        map.layers.iter().find(|m| m.name.eq_ignore_ascii_case(name) && m.geometry_type() == GeometryType::Polygon)
                    })
                    .map(|m| m.id);
                std::iter::once(l.id).chain(mask)
            })
            .collect()
    };
    if !complete_layers.is_empty() {
        for layer in &mut map.layers {
            if !complete_layers.contains(&layer.id) {
                continue;
            }
            if let Some(source) = &mut layer.source
                && !source.is_open()
                && is_local_shapefile(source)
            {
                if let Err(e) = source.open() {
                    source.error = Some(e);
                    return None;
                }
                if layer.dataset.fields.is_empty()
                    && let Some(info) = source.info()
                {
                    layer.dataset.fields = info.fields.clone();
                }
            }
        }
    }
    let depth_scale = map.ground_res_at(&camera) / rustgis_geom::PIXEL_M;
    // Perspective markers and extrusions have a global depth pass. Preserve that complete
    // upstream path using an isolated full-source snapshot until it has a chunk-aware sorter.
    // The owning interaction window, selection and editing provider are never replaced.
    if camera.pitch != 0.0
        && !map.projection.is_globe()
        && map.layers.iter().any(|l| needs_tilted_snapshot(map, l, &complete_layers, depth_scale))
    {
        // Keep asynchronous windows and pending work on their owner, never a disposable copy.
        let bands = camera.tilt_bands(view.width, view.height);
        let data_pending = sync_tilted(map, &View { camera, ..*view }, &bands, full_res_rasters);
        let mut snapshot = map.clone();
        let mut snapshot_budget = SnapshotBudget::new(complete_snapshot_limit());
        for layer in &mut snapshot.layers {
            if !complete_layers.contains(&layer.id) || layer.source.as_ref().and_then(|s| s.chunk_source()).is_none() {
                layer.source = None;
                continue;
            }
            if !needs_tilted_snapshot(map, layer, &complete_layers, depth_scale) {
                // Plain fills/lines keep their complete per-band traversal, outside the
                // global depth-order snapshot and its feature-storage budget.
                continue;
            }
            let Some(source) = layer.source.as_ref().and_then(|s| s.chunk_source()) else { continue };
            let ids: std::collections::HashSet<u64> =
                layer.selection.iter().filter_map(|r| layer.dataset.features.get(*r).map(|f| f.id)).collect();
            let mut features = Vec::new();
            let result = visit_complete(source.as_ref(), &mut |chunk| snapshot_budget.append(&mut features, chunk));
            if let Err(e) = result {
                if let Some(s) = map.layer_mut(layer.id).and_then(|l| l.source.as_mut()) {
                    s.error = Some(e.to_string());
                }
                return None;
            }
            if let Some(source) = map.layer_mut(layer.id).and_then(|l| l.source.as_mut()) {
                source.error = None;
            }
            layer.selection = features.iter().enumerate().filter_map(|(r, f)| ids.contains(&f.id).then_some(r)).collect();
            layer.source = None;
            layer.dataset.features = features.into();
            layer.clear_cache();
            layer.touch_geometry();
        }
        let view = View { camera, ..*view };
        // Owner synchronization has already run. Repeating it on the copy could consume a
        // shared pending result before its owner installs it, even for a plain chunk source.
        let mut output = if backend.is_gpu() {
            gpu_turned(&mut snapshot, &view, full_res_rasters, true)
                .map(RenderOutput::Gpu)
                .or_else(|| render_tilted(&mut snapshot, &view, full_res_rasters, true).map(RenderOutput::Cpu))
        } else {
            render_tilted(&mut snapshot, &view, full_res_rasters, true).map(RenderOutput::Cpu)
        };
        map.adopt_caches(&snapshot);
        if let Some(output) = &mut output {
            match output {
                RenderOutput::Cpu(rendered) => rendered.data_pending |= data_pending,
                RenderOutput::Gpu(scene) => scene.data_pending |= data_pending,
            }
        }
        return output;
    }
    // The globe turns and tilts itself (`Globe`): one frame.
    if map.projection.is_globe() {
        let view = View { camera, ..*view };
        return render_frame(map, &view, full_res_rasters, backend, Labels::Own);
    }
    {
        let view = View { camera, ..*view };
        if backend.is_gpu()
            && (camera.pitch != 0.0 || camera.bearing != 0.0)
            && let Some(scene) = gpu_turned(map, &view, full_res_rasters, false)
        {
            return Some(RenderOutput::Gpu(scene));
        }
        if camera.pitch != 0.0 {
            return render_tilted(map, &view, full_res_rasters, false).map(RenderOutput::Cpu);
        }
        if camera.bearing != 0.0 {
            return render_turned(map, &view, full_res_rasters).map(RenderOutput::Cpu);
        }
    }
    render_frame(map, view, full_res_rasters, backend, Labels::Own)
}

/// Persisted local Shapefiles can require complete traversal before a handle is opened.
/// Converted, embedded, remote and database sources retain their own provider behavior.
pub(crate) fn is_local_shapefile(source: &rustgis_map::source::SourceLayer) -> bool {
    source.embedded.is_none()
        && source.gdal.is_none()
        && source.duckdb.is_none()
        && source.postgis.is_none()
        && source.mysql.is_none()
        && source.sqlserver.is_none()
        && source.path.as_ref().is_some_and(|p| {
            !rustgis_data::source::is_remote_name(p) && std::path::Path::new(p).extension().is_some_and(|e| e.eq_ignore_ascii_case("shp"))
        })
}

/// Layers whose perspective painter needs complete globally depth-ordered features.
/// Ordinary fills/lines and their masks stay on the complete per-band chunk path.
fn needs_tilted_snapshot(map: &Map, layer: &rustgis_map::Layer, complete_layers: &std::collections::HashSet<u32>, scale: f64) -> bool {
    complete_layers.contains(&layer.id)
        && map.draws_at(layer, scale)
        && layer.source.as_ref().and_then(|s| s.chunk_source()).is_some()
        && (stands_up(map, layer) || (layer.geometry_type() == GeometryType::Polygon && layer.extrusion.is_some()))
}

// Separate from the interaction budget: this limits retained complete-source feature storage
// across the perspective snapshot. Raster targets, derived caches and decoding scratch are extra.
const COMPLETE_SNAPSHOT_BYTES: usize = rustgis_map::source::VIEW_BYTES * 4;
/// Owned-storage target for a complete traversal; distinct from interaction and depth budgets.
const COMPLETE_CHUNK_BYTES: usize = 8 * 1024 * 1024;

#[cfg(test)]
thread_local! {
    static SNAPSHOT_TEST_LIMIT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

fn complete_snapshot_limit() -> usize {
    #[cfg(test)]
    if let Some(limit) = SNAPSHOT_TEST_LIMIT.with(|s| s.get()) {
        return limit;
    }
    COMPLETE_SNAPSHOT_BYTES
}

/// Account owned rows and actual vector capacity across all materialized chunk sources.
/// Exceeding the budget fails the entire frame; no source is sampled or truncated.
struct SnapshotBudget {
    limit: usize,
    used: usize,
}

impl SnapshotBudget {
    fn new(limit: usize) -> Self {
        Self { limit, used: 0 }
    }

    fn append(&mut self, features: &mut Vec<rustgis_data::Feature>, mut chunk: Vec<rustgis_data::Feature>) -> rustgis_data::Result<()> {
        let bad =
            || rustgis_data::DataError::Format(format!("complete perspective snapshot exceeds its {} byte feature budget", self.limit));
        let row_size = std::mem::size_of::<rustgis_data::Feature>();
        let heap = chunk.iter().fold(0usize, |n, f| n.saturating_add(rustgis_data::source::chunk_memory::feature_bytes(f)));
        let needed = features.len().checked_add(chunk.len()).ok_or_else(bad)?;
        let old_capacity = features.capacity();
        let extra = needed.saturating_sub(old_capacity).saturating_mul(row_size);
        if self.used.saturating_add(heap).saturating_add(extra) > self.limit {
            return Err(bad());
        }
        features
            .try_reserve_exact(chunk.len())
            .map_err(|e| rustgis_data::DataError::Format(format!("complete perspective snapshot: {e}")))?;
        let extra = features.capacity().saturating_sub(old_capacity).saturating_mul(row_size);
        let used = self.used.saturating_add(heap).saturating_add(extra);
        if used > self.limit {
            return Err(bad());
        }
        self.used = used;
        features.append(&mut chunk);
        Ok(())
    }
}

/// A capable source must explicitly finish the traversal; an incomplete summary must never
/// publish an apparently successful frame, even if it already delivered some chunks.
fn visit_complete(
    source: &dyn rustgis_data::source::FeatureSource,
    visit: &mut dyn FnMut(Vec<rustgis_data::Feature>) -> rustgis_data::Result<()>,
) -> rustgis_data::Result<rustgis_data::source::Batch> {
    let result = source.visit_chunks(None, COMPLETE_CHUNK_BYTES, visit)?;
    if !result.complete {
        return Err(rustgis_data::DataError::Format("source traversal was incomplete".into()));
    }
    Ok(result)
}

/// A label waiting to be placed: the label, its priority and its (layer id, row).
type Candidate = (PlacedLabel, f64, (u32, usize));

/// Keep exactly the candidates the upstream placement limit can consider, with stable ties.
fn compact_label_candidates(candidates: &mut Vec<Candidate>) {
    if candidates.len() > 40_000 {
        let rank = |p: f64| if p.is_nan() { f64::NEG_INFINITY } else { p };
        candidates.sort_by(|a, b| rank(b.1).total_cmp(&rank(a.1)));
        candidates.truncate(20_000);
    }
}

/// Where a frame's labels go.
enum Labels<'a> {
    /// Placed in the frame.
    Own,
    /// Placed in the turned view the north-up frame stands in for ([`render_turned`]).
    Turned(&'a View),
    /// Handed back unplaced, at their places in a tilted view ([`render_tilted`]), whose
    /// bands share one read of the streamed layers.
    Tilted(&'a View, &'a mut Vec<Candidate>),
    /// Placed in the frame, a band of a tilted view's drape ([`render_tilt_drape`]), which
    /// shares the view's read of the streamed layers.
    Shared,
}

/// Labels farther than this (as a fraction of the centre's scale) are left out of a tilted view.
const LABEL_MIN_DEPTH_SCALE: f64 = 0.3;

/// A tilted view, on the CPU: the ground it shows is drawn flat in bands
/// ([`Camera::tilt_bands`]), sharp near and coarser toward the horizon (so far tiles come at
/// lower zoom), then each screen pixel takes its colour from the band under it, fading into fog
/// and the sky past the view's reach. Labels are placed in the tilted view, upright.
fn render_tilted(map: &mut Map, view: &View, full_res_rasters: bool, sources_ready: bool) -> Option<Rendered> {
    let bands_of = view.camera.tilt_bands(view.width, view.height);
    let data_pending = !sources_ready && sync_tilted(map, view, &bands_of, full_res_rasters);
    // Labels are placed in the tilted view: a lock on a flat view's labels doesn't apply.
    let mut candidates = Vec::new();
    let (w, h) = view.pixel_size();
    let mut pm = Pixmap::new(w, h)?;
    let lock = map.label_lock.take();
    let mut warp = Warp::new(view, map);
    // On terrain the bands are kept, to lay over the ground's relief once all are drawn: every
    // triangle of every band is then ordered far to near together. That holds all the bands at
    // once (at most eight, `Camera::tilt_bands`, each capped at `MAX_BAND_PX` a side) where the flat warp holds
    // one at a time: the cost of hiding what slopes stand in front of.
    let ground = terrain::Ground::of(map, &view.camera);
    let mut kept: Vec<(View, (f64, f64), Pixmap)> = Vec::new();
    let last = bands_of.len().saturating_sub(1);
    let (mut attribution, mut tiles_pending, mut tile_errors) = (Vec::<String>::new(), false, Vec::<String>::new());
    for (i, b) in bands_of.into_iter().enumerate() {
        let v = band_view(view, &b);
        let Some(RenderOutput::Cpu(r)) = render_frame(map, &v, full_res_rasters, Backend::Cpu, Labels::Tilted(view, &mut candidates))
        else {
            let scale = map.ground_res_at(&v.camera) / rustgis_geom::PIXEL_M;
            let complete = sources_ready
                || map.layers.iter().filter(|l| map.draws_at(l, scale)).any(|l| {
                    l.source.as_ref().and_then(|s| s.chunk_source()).is_some()
                        || l.mask
                            .as_deref()
                            .and_then(|name| {
                                map.layers.iter().find(|m| m.name.eq_ignore_ascii_case(name) && m.geometry_type() == GeometryType::Polygon)
                            })
                            .and_then(|m| m.source.as_ref())
                            .and_then(|s| s.chunk_source())
                            .is_some()
                });
            if complete {
                map.label_lock = lock;
                return None;
            }
            // Preserve the upstream best-effort band path for ordinary providers.
            continue;
        };
        for a in r.attribution {
            if !attribution.contains(&a) {
                attribution.push(a);
            }
        }
        for e in r.tile_errors {
            if !tile_errors.contains(&e) {
                tile_errors.push(e);
            }
        }
        tiles_pending |= r.tiles_pending;
        if ground.is_some() {
            // The nearest band reaches the bottom of the view, the farthest the sky.
            let depth = (if i == 0 { 0.0 } else { b.depth.0 }, if i == last { f64::INFINITY } else { b.depth.1 });
            kept.push((v, depth, r.pixmap));
        } else {
            // Into the view at once, nearest band first, so only one band is held at a time.
            warp.band(&mut pm, &v, &r.pixmap, view.camera.depth_y(b.depth.1, view.height), view.camera.depth_y(b.depth.0, view.height));
        }
    }
    map.label_lock = lock;
    match &ground {
        Some(g) => {
            drape_terrain(&mut pm, view, map, &kept, g);
            tiles_pending |= g.pending();
        }
        None => warp.finish(&mut pm),
    }
    drop(kept);
    extrusions_tilted(map, view, &mut pm);
    markers_tilted(map, view, &mut pm);
    let placed = place_tilted_labels(map, view, candidates, ground.as_ref());
    for l in &placed.labels {
        if let Some(icon) = &l.icon {
            draw_icon(&mut pm, icon, view.pixels_per_point as f32);
        }
    }
    let unplaced = if map.label_display.show_unplaced { placed.unplaced } else { Vec::new() };
    Some(Rendered { pixmap: pm, labels: placed.labels, unplaced, attribution, tiles_pending, data_pending, tile_errors })
}

/// Lays a tilted view's bands, drawn flat, over the ground's relief: each band as a grid of
/// triangles raised to the ground's height and mapped from its picture, every band keeping to
/// its own stretch of ground, all drawn far to near so nearer slopes hide what lies behind
/// them; fog with distance, and the sky behind. (Without terrain, [`Warp`] does it per pixel.)
fn drape_terrain(pm: &mut Pixmap, view: &View, map: &Map, bands: &[(View, (f64, f64), Pixmap)], ground: &terrain::Ground) {
    let (sky, fog) = tilt_sky(map);
    let cam = view.camera;
    let (w, h) = (view.width, view.height);
    let k = f64::from(pm.width()) / w.max(1e-9);
    let far = rustgis_map::camera::FAR;
    // The sky shading into the fog at the reach's row; fog below it, where no slope stands.
    let sky_y = (cam.sky_y(h).unwrap_or(0.0).max(1.0) * k) as f32;
    let c = |c: Rgba| tiny_skia::Color::from_rgba8(c.r(), c.g(), c.b(), 255);
    match tiny_skia::LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(0.0, sky_y),
        vec![GradientStop::new(0.0, c(sky)), GradientStop::new(1.0, c(fog))],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        Some(shader) => {
            if let Some(r) = tiny_skia::Rect::from_xywh(0.0, 0.0, pm.width() as f32, pm.height() as f32) {
                pm.fill_rect(r, &Paint { shader, ..Paint::default() }, Transform::identity(), None);
            }
        }
        None => pm.fill(c(fog)),
    }
    // Every band's triangles: (depth, band, screen and picture corners).
    type Tri = (f64, usize, [(f64, f64); 3], [(f64, f64); 3]);
    let mut tris: Vec<Tri> = Vec::new();
    let n = terrain::CELLS;
    for (bi, (v, (d0, d1), pic)) in bands.iter().enumerate() {
        let kb = f64::from(pic.width()) / v.width.max(1e-9);
        // A band's grid: screen point, picture point and depth of each vertex.
        let grid: Vec<Option<((f64, f64), (f64, f64), f64)>> = (0..=n)
            .flat_map(|j| (0..=n).map(move |i| (i, j)))
            .map(|(i, j)| {
                let (lx, ly) = (v.width * i as f64 / n as f64, v.height * j as f64 / n as f64);
                let m = v.camera.to_map(lx, ly, v.width, v.height);
                let depth = 1.0 / cam.depth_scale(m, h)?;
                let (u, gv) = cam.ground_px(m);
                let (sx, sy) = cam.project_raised(u, gv, ground.up_px(m, &cam), w, h)?;
                (sx.is_finite() && sy.is_finite()).then_some(((sx * k, sy * k), (lx * kb, ly * kb), depth))
            })
            .collect();
        // Its own stretch of ground, a little over at the seams.
        let margin = ((d1.min(far) - d0) * 0.02).max(0.0);
        for j in 0..n {
            for i in 0..n {
                let at = |i: usize, j: usize| grid.get(j * (n + 1) + i).copied().flatten();
                let (Some(a), Some(b), Some(c), Some(d)) = (at(i, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1)) else { continue };
                for [p, q, r] in [[a, b, d], [a, d, c]] {
                    let depth = (p.2 + q.2 + r.2) / 3.0;
                    if depth < d0 - margin || depth > d1 + margin || p.2.min(q.2).min(r.2) > far {
                        continue;
                    }
                    // Wholly off one side of the picture: nothing to draw.
                    let (pw, ph) = (f64::from(pm.width()), f64::from(pm.height()));
                    let xs = [p.0.0, q.0.0, r.0.0];
                    let ys = [p.0.1, q.0.1, r.0.1];
                    if xs.iter().all(|x| *x < 0.0)
                        || xs.iter().all(|x| *x > pw)
                        || ys.iter().all(|y| *y < 0.0)
                        || ys.iter().all(|y| *y > ph)
                    {
                        continue;
                    }
                    tris.push((depth, bi, [p.0, q.0, r.0], [p.1, q.1, r.1]));
                }
            }
        }
    }
    tris.sort_by(|a, b| b.0.total_cmp(&a.0));
    let fog_paint = |t: f64| paint(fog, t as f32);
    for (depth, bi, dst, src) in tris {
        let Some((_, _, pic)) = bands.get(bi) else { continue };
        let Some(t) = affine(src, dst) else { continue };
        // A hair larger all round, so neighbours leave no seam.
        let (cx, cy) = ((dst[0].0 + dst[1].0 + dst[2].0) / 3.0, (dst[0].1 + dst[1].1 + dst[2].1) / 3.0);
        let mut pb = PathBuilder::new();
        for (i, (x, y)) in dst.iter().enumerate() {
            let (dx, dy) = (x - cx, y - cy);
            let len = dx.hypot(dy).max(1e-9);
            let (x, y) = ((x + dx / len * 0.6) as f32, (y + dy / len * 0.6) as f32);
            if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
        }
        pb.close();
        let Some(path) = pb.finish() else { continue };
        let shader = tiny_skia::Pattern::new(pic.as_ref(), SpreadMode::Pad, tiny_skia::FilterQuality::Bilinear, 1.0, t);
        pm.fill_path(&path, &Paint { shader, anti_alias: true, ..Paint::default() }, FillRule::Winding, Transform::identity(), None);
        // Fog from halfway to the reach.
        let f = ((depth - far * 0.5) / (far * 0.5)).clamp(0.0, 1.0);
        let f = f * f * (3.0 - 2.0 * f);
        if f > 0.004 {
            pm.fill_path(&path, &fog_paint(f), FillRule::Winding, Transform::identity(), None);
        }
    }
}

/// The affine map taking the triangle `src` onto `dst` (physical pixels); `None` for a
/// triangle with no area.
fn affine(src: [(f64, f64); 3], dst: [(f64, f64); 3]) -> Option<Transform> {
    let (s1, s2) = ((src[1].0 - src[0].0, src[1].1 - src[0].1), (src[2].0 - src[0].0, src[2].1 - src[0].1));
    let (d1, d2) = ((dst[1].0 - dst[0].0, dst[1].1 - dst[0].1), (dst[2].0 - dst[0].0, dst[2].1 - dst[0].1));
    let det = s1.0 * s2.1 - s2.0 * s1.1;
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    let a = (d1.0 * s2.1 - d2.0 * s1.1) / det;
    let b = (d2.0 * s1.0 - d1.0 * s2.0) / det;
    let c = (d1.1 * s2.1 - d2.1 * s1.1) / det;
    let d = (d2.1 * s1.0 - d1.1 * s2.0) / det;
    let tx = dst[0].0 - a * src[0].0 - b * src[0].1;
    let ty = dst[0].1 - c * src[0].0 - d * src[0].1;
    [a, b, c, d, tx, ty]
        .iter()
        .all(|v| v.is_finite())
        .then(|| Transform::from_row(a as f32, c as f32, b as f32, d as f32, tx as f32, ty as f32))
}

/// Lifts a tilted view's labels onto the ground's relief: each to the height of the ground
/// under it.
fn lift_labels(view: &View, ground: &terrain::Ground, candidates: &mut [Candidate]) {
    let cam = view.camera;
    let (w, h) = (view.width, view.height);
    for (l, _, _) in candidates.iter_mut() {
        let Some(m) = cam.try_to_map(f64::from(l.x), f64::from(l.y), w, h) else { continue };
        let (u, v) = cam.ground_px(m);
        let Some((sx, sy)) = cam.project_raised(u, v, ground.up_px(m, &cam), w, h) else { continue };
        let (dx, dy) = (sx as f32 - l.x, sy as f32 - l.y);
        if !(dx.is_finite() && dy.is_finite()) {
            continue;
        }
        (l.x, l.y) = (sx as f32, sy as f32);
        if let Some(i) = &mut l.icon {
            (i.x, i.y) = (i.x + dx, i.y + dy);
        }
    }
}

/// A turned or tilted view as a GPU scene, drawn by the vertex shader's full perspective: a
/// turned view's north-up frame, or a tilted view's bands, each clipped to its own ground (as
/// [`render_tilted`] warps them), with point markers upright over them and labels placed in
/// the view. `None` when the view needs pictures drawn on the CPU (tiles, rasters, layers drawn
/// as pictures, icons or extrusions), or the scene isn't plain: the caller draws it on the CPU.
fn gpu_turned(map: &mut Map, view: &View, full_res_rasters: bool, sources_ready: bool) -> Option<gpu_scene::GpuScene> {
    use gpu_scene::{CmdClip, DrawCmd, GpuScene, SceneTilt};
    if !gpu_may_turn(map, view) {
        return None;
    }
    let pictures = |cmds: &[DrawCmd]| cmds.iter().any(|c| matches!(c, DrawCmd::Image { .. }));
    let (sky, fog) = tilt_sky(map);
    let tilt = Some(SceneTilt { camera: view.camera, sky: mesh::premul(sky), fog: mesh::premul(fog) });
    if view.camera.pitch == 0.0 {
        let (bw, bh) = view.camera.rotated_size(view.width, view.height);
        let north = View { width: bw.ceil() + 2.0, height: bh.ceil() + 2.0, camera: Camera { bearing: 0.0, ..view.camera }, ..*view };
        let Some(RenderOutput::Gpu(scene)) = render_frame(map, &north, full_res_rasters, Backend::Gpu, Labels::Turned(view)) else {
            return None;
        };
        if pictures(&scene.cmds) || scene.labels.iter().any(|l| l.icon.is_some()) {
            return None;
        }
        return Some(GpuScene { tilt, ..scene });
    }
    let bands = view.camera.tilt_bands(view.width, view.height);
    let data_pending = !sources_ready && sync_tilted(map, view, &bands, full_res_rasters);
    let lock = map.label_lock.take();
    let mut candidates = Vec::new();
    let (mut cmds, mut clips) = (Vec::new(), Vec::new());
    let (mut attribution, mut tiles_pending, mut tile_errors, mut clear) = (Vec::<String>::new(), false, Vec::<String>::new(), [0.0; 4]);
    let mut plain = true;
    let last = bands.len().saturating_sub(1);
    for (i, b) in bands.iter().enumerate() {
        let v = band_view(view, b);
        let Some(RenderOutput::Gpu(scene)) = render_frame(map, &v, full_res_rasters, Backend::Gpu, Labels::Tilted(view, &mut candidates))
        else {
            plain = false;
            break;
        };
        if pictures(&scene.cmds) {
            plain = false;
            break;
        }
        clear = scene.clear;
        // A long band is drawn at a lower pixel density (`band_view`): its strokes and markers,
        // sized in its pixels, are brought to the view's.
        let k = (view.pixels_per_point / v.pixels_per_point.max(1e-9)) as f32;
        let mut scene = scene;
        if (k - 1.0).abs() > 1e-6 {
            for c in &mut scene.cmds {
                match c {
                    DrawCmd::Mesh { vertices, .. } => vertices.iter_mut().for_each(|v| v.stroke_px *= k),
                    DrawCmd::Points { instances, .. } => instances.iter_mut().for_each(|p| p.radius_px *= k),
                    DrawCmd::Image { .. }
                    | DrawCmd::Extrusion { .. }
                    | DrawCmd::Tiles { .. }
                    | DrawCmd::LiveTiles { .. }
                    | DrawCmd::TileMeshes { .. } => {}
                }
            }
        }
        for a in scene.attribution {
            if !attribution.contains(&a) {
                attribution.push(a);
            }
        }
        for e in scene.tile_errors {
            if !tile_errors.contains(&e) {
                tile_errors.push(e);
            }
        }
        tiles_pending |= scene.tiles_pending;
        // The nearest band reaches the bottom of the view and the farthest the sky, so no ground
        // falls between them.
        let near = if i == 0 { 0.0 } else { b.depth.0 as f32 };
        let far = if i == last { f32::MAX } else { b.depth.1 as f32 };
        let clip = CmdClip { depth: [near, far], upright: false };
        clips.extend(std::iter::repeat_n(clip, scene.cmds.len()));
        cmds.extend(scene.cmds);
    }
    map.label_lock = lock;
    if !plain {
        return None;
    }
    let placed = place_tilted_labels(map, view, candidates, None);
    if placed.labels.iter().any(|l| l.icon.is_some()) {
        return None;
    }
    let origin = [view.camera.center.x, view.camera.center.y];
    // Extruded features stand up over the ground, their shadows under them; more than the GPU
    // budget takes, and the view is drawn on the CPU.
    for cmd in extrusion_cmds(&extruded_shapes(map, view), origin)? {
        cmds.push(cmd);
        clips.push(CmdClip::ALL);
    }
    let ppp = view.pixels_per_point as f32;
    let instances: Vec<gpu_scene::PointInstance> = upright_markers(map, view, 0.0)
        .iter()
        .flat_map(|m| mesh::marker_instances(m.at, origin, &m.symbol, ppp, m.selected.then_some(SELECTION)))
        .collect();
    if !instances.is_empty() {
        cmds.push(DrawCmd::Points { origin, instances });
        clips.push(CmdClip { upright: true, ..CmdClip::ALL });
    }
    let unplaced = if map.label_display.show_unplaced { placed.unplaced } else { Vec::new() };
    Some(GpuScene { clear, cmds, labels: placed.labels, unplaced, attribution, tiles_pending, data_pending, tile_errors, tilt, clips })
}

/// Places the labels a tilted view's bands offered. A feature drawn in two bands offers its
/// label twice: the nearer band's (offered first) is kept. Tile labels (keyed by their place in a
/// band's list) are told apart by their text and where they land, to a few points. Extruded
/// features' labels go up onto their roofs, and on terrain every label onto the ground's relief.
fn place_tilted_labels(map: &Map, view: &View, mut candidates: Vec<Candidate>, ground: Option<&terrain::Ground>) -> Placement {
    let mut seen = std::collections::HashSet::new();
    let mut seen_tile = std::collections::HashSet::new();
    candidates.retain(|c| match c.2.0 {
        u32::MAX => seen_tile.insert((c.0.text.clone(), (c.0.x / 4.0).round() as i64, (c.0.y / 4.0).round() as i64)),
        _ => seen.insert(c.2),
    });
    raise_labels(map, view, &mut candidates);
    if let Some(g) = ground {
        lift_labels(view, g, &mut candidates);
    }
    place_labels(candidates, Vec::new(), view.width as f32, view.height as f32, false)
}

/// Whether a turned or tilted `view` of `map` may be drawn on the GPU ([`gpu_turned`]): only
/// what the GPU draws itself shows (no tiles, rasters, see-through, masked, swiped or blended
/// layers, aggregations, graphics or stacked line strokes; no view effects; extrusions only
/// stand up in a tilted one). A cheap check made before any drawing; labels with icons can still
/// decline the scene once it is drawn.
pub fn gpu_may_turn(map: &Map, view: &View) -> bool {
    if map.projection.is_globe()
        || !map.view.effects.is_none()
        || map.view.color_vision != rustgis_map::view::ColorVision::Normal
        || map.tiles.iter().any(|t| t.draws())
        // The GPU draws the ground flat: a view over terrain stays on the CPU.
        || terrain::Ground::of(map, &view.camera).is_some()
    {
        return false;
    }
    let scale = map.ground_res_at(&view.camera) / rustgis_geom::PIXEL_M;
    let held = map.held_by_group(scale);
    // Layers in a see-through group (none, mostly: then nothing is collected).
    let faded: std::collections::HashSet<u32> = if map.groups.iter().any(|g| g.transparency > 0.0) {
        map.groups.iter().filter(|g| g.transparency > 0.0).flat_map(|g| g.children.iter().copied()).collect()
    } else {
        Default::default()
    };
    map.layers.iter().filter(|l| l.visible && l.in_scale_range(scale) && !held.contains(&l.id)).all(|l| {
        l.raster.is_none()
            && l.opacity() >= 0.999
            && !faded.contains(&l.id)
            && l.mask.is_none()
            && l.blend == rustgis_map::effects::Blend::Normal
            && map.swipe.is_none_or(|(id, _)| id != l.id)
            && l.aggregation.is_none()
            && !l.graphics
            && l.line_strokes.is_empty()
            && (l.extrusion.is_none() || view.camera.pitch != 0.0)
    })
}

/// The colours of a tilted view's sky (at the top) and of the fog it fades into at the
/// horizon, from the basemap. A body without the Earth's atmosphere has neither: the ground
/// fades into the black of space (#223).
pub fn tilt_sky(map: &Map) -> (Rgba, Rgba) {
    if !map.body().is_earth() {
        let space = globe_space(true);
        return (space, space);
    }
    let (water, land, _, _) = map.palette();
    let white = Rgba::rgb(0xFF, 0xFF, 0xFF);
    let sky = water.lerp(white, 0.35);
    (sky, sky.lerp(land, 0.5).lerp(white, 0.3))
}

/// Lifts the labels of extruded features onto their roofs in a tilted view (as MapLibre
/// does), where the extrusions drawn over the ground would otherwise hide them.
fn raise_labels(map: &Map, view: &View, candidates: &mut [Candidate]) {
    let flat = map.flat();
    let cam = view.camera;
    let (w, h) = (view.width, view.height);
    for (l, _, (id, row)) in candidates.iter_mut() {
        let Some(layer) = map.layer(*id).filter(|l| l.geometry_type() == GeometryType::Polygon) else { continue };
        let Some(x) = &layer.extrusion else { continue };
        let Some(m) = cam.try_to_map(f64::from(l.x), f64::from(l.y), w, h) else { continue };
        // At the label's place (Web Mercator stretches the ground toward the poles).
        let up = (effects::extrusion_metres(&layer.dataset, x, *row)
            / (flat.ground_per_unit(m) * map.body().ratio()).max(1e-9)
            / cam.res.max(1e-9))
        .clamp(0.0, 1e7);
        let (u, v) = cam.ground_px(m);
        let Some((sx, sy)) = cam.project_raised(u, v, up, w, h) else { continue };
        let (dx, dy) = (sx as f32 - l.x, sy as f32 - l.y);
        (l.x, l.y) = (sx as f32, sy as f32);
        if let Some(i) = &mut l.icon {
            (i.x, i.y) = (i.x + dx, i.y + dy);
        }
    }
}

/// An extruded feature tessellated once, for a moving tilted view to stand up in perspective
/// every frame (as MapLibre's fill extrusions are built once and only re-projected).
pub struct ExtrudedShape {
    /// The point (the camera's coordinates) the roof's triangles are relative to.
    pub origin: Coord,
    /// The middle of its outline (the camera's coordinates): where it is, for ordering and fog.
    pub center: Coord,
    /// The roof's triangles: vertices relative to `origin` (map units) and their indices.
    pub roof: Vec<[f32; 2]>,
    pub roof_indices: Vec<u32>,
    /// The outer rings the walls stand on, and the holes' (the camera's coordinates).
    pub rings: Vec<Vec<Coord>>,
    pub holes: Vec<Vec<Coord>>,
    /// Its height: in metres, and in the camera's units at its place (Web Mercator stretches the
    /// ground toward the poles).
    pub metres: f64,
    pub height: f64,
    pub fill: Rgba,
    pub stroke: Rgba,
    /// The layer's extrusion settings: walls facing away culled, walls lit, shadows cast.
    pub face_culling: bool,
    pub lighting: bool,
    pub shadows: bool,
}

/// The extruded features of a tilted `view` of `map`, tessellated for [`ExtrudedShape`]s, from
/// geometry thinned for the view's scale, bottom layer first.
pub fn extruded_shapes(map: &mut Map, view: &View) -> Vec<ExtrudedShape> {
    let scale = map.ground_res_at(&view.camera) / rustgis_geom::PIXEL_M;
    // Layers a hidden or out-of-scale group holds aren't drawn, as in `render_frame`.
    let held = map.held_by_group(scale);
    // Only the features on the ground the view shows (grown by a moving drape's margin).
    let cull = view.camera.extent(view.width, view.height).padded(0.3, 0.0);
    let (flat, key) = (map.flat(), map.projection.code());
    // Extrusion heights are metres on the map's body.
    let body = map.body().ratio();
    let tol = view.camera.res * rustgis_map::lod::THIN_PX;
    let mut order: Vec<usize> = (0..map.layers.len()).rev().collect();
    order.sort_by_key(|i| map.layers.get(*i).map_or(0, |l| l.priority));
    let mut out = Vec::new();
    for li in order {
        let Some(layer) = map.layers.get_mut(li) else { continue };
        if map.projection.is_globe()
            || !layer.visible
            || !layer.in_scale_range(scale)
            || held.contains(&layer.id)
            || layer.raster.is_some()
            || layer.geometry_type() != GeometryType::Polygon
        {
            continue;
        }
        let Some(x) = layer.extrusion.clone() else { continue };
        let rows = layer.drawn_rows(scale);
        layer.thinned_in(&flat, &key, tol);
        let layer: &rustgis_map::Layer = layer;
        let extents = layer.projected_extents();
        for (row, g) in layer.thinned_cache(tol).iter().enumerate() {
            let Some(Geometry::Polygon(polys)) = g else { continue };
            if rows.get(row) != Some(&true) {
                continue;
            }
            let Some(origin) = polys.iter().flatten().flatten().find(|c| c.is_finite()).copied() else { continue };
            let metres = effects::extrusion_metres(&layer.dataset, &x, row);
            let height = metres / (flat.ground_per_unit(origin) * body).max(1e-9);
            // Off the ground the view shows, even by its height: a tall building just off screen
            // still stands up into view.
            let reach = if height.is_finite() { height.clamp(0.0, 1e9) } else { 0.0 };
            if extents.get(row).is_some_and(|e| !e.padded(0.0, reach).intersects(&cull)) {
                continue;
            }
            let roof = mesh::fill_polygon(polys, [origin.x, origin.y], [0.0; 4], view.camera.res * 0.5);
            let Ok(Some((verts, indices))) = roof else { continue };
            let sym = layer.symbol(row);
            let pts: Vec<Coord> = polys.iter().filter_map(|p| p.first()).flatten().copied().filter(|c| c.is_finite()).collect();
            let n = pts.len().max(1) as f64;
            let center = Coord::new(pts.iter().map(|c| c.x).sum::<f64>() / n, pts.iter().map(|c| c.y).sum::<f64>() / n);
            out.push(ExtrudedShape {
                origin,
                center,
                roof: verts.iter().map(|v| v.position).collect(),
                roof_indices: indices,
                rings: polys.iter().filter_map(|p| p.first()).filter(|r| r.len() >= 3).cloned().collect(),
                holes: polys.iter().flat_map(|p| p.iter().skip(1)).filter(|r| r.len() >= 3).cloned().collect(),
                metres,
                height,
                fill: sym.fill.with_alpha(255),
                stroke: sym.stroke,
                face_culling: x.face_culling,
                lighting: x.lighting,
                shadows: x.shadows,
            });
        }
    }
    out
}

pub use effects::LIGHT as EXTRUSION_LIGHT;
pub use effects::{TiltedWall, clip_near, shadow_shift, tilted_shadow_quads, tilted_walls};

/// Extruded features as GPU commands, relative to `origin`: the shadows they cast (when any
/// do), then their roofs and walls, which the depth buffer sorts out. Walls carry their outward
/// normals, lit in the shader so they stay lit right as the map turns.
fn extrusion_cmds(shapes: &[ExtrudedShape], origin: [f64; 2]) -> Option<Vec<gpu_scene::DrawCmd>> {
    // The most vertices the roofs, walls and shadows take together (about 220 MB), well before
    // a device's buffer limit or u32 indices could be reached: past it, `None` (the view is
    // drawn on the CPU rather than with features missing).
    const MAX_VERTICES: usize = 4_000_000;
    use gpu_scene::{EXTRUDE_ROOF, EXTRUDE_SHADOW, EXTRUDE_WALL, ExtrudeVertex};
    let rel = |c: Coord, z: f64| [(c.x - origin[0]) as f32, (c.y - origin[1]) as f32, z as f32];
    let vertex =
        |position: [f32; 3], normal: [f32; 2], color: [f32; 4], info: [f32; 4]| ExtrudeVertex { position, normal, color, info, _pad: 0.0 };
    let (mut solid, mut solid_i) = (Vec::new(), Vec::<u32>::new());
    let (mut shade, mut shade_i) = (Vec::new(), Vec::<u32>::new());
    let shadow = mesh::premul(Rgba::new(0, 0, 0, 60));
    for s in shapes {
        let more =
            s.roof.len().saturating_mul(2).saturating_add(s.rings.iter().chain(&s.holes).map(Vec::len).sum::<usize>().saturating_mul(8));
        if solid.len().saturating_add(shade.len()).saturating_add(more) > MAX_VERTICES {
            return None;
        }
        // A height that is no number (hostile attributes) stands no higher than the ground.
        let h = if s.height.is_finite() { s.height.clamp(0.0, 1e9) } else { 0.0 };
        // Roof indices beyond its vertices (never from the tessellator) would draw garbage.
        if s.roof_indices.iter().any(|i| *i as usize >= s.roof.len()) {
            continue;
        }
        let color = mesh::premul(s.fill);
        let lit = if s.lighting { 1.0 } else { 0.0 };
        let roof_at = |p: &[f32; 2]| Coord::new(s.origin.x + f64::from(p[0]), s.origin.y + f64::from(p[1]));
        // The roof.
        let base = solid.len() as u32;
        solid.extend(s.roof.iter().map(|p| vertex(rel(roof_at(p), h), [0.0; 2], color, [1.0, EXTRUDE_ROOF, lit, 0.0])));
        solid_i.extend(s.roof_indices.iter().map(|i| base + i));
        if s.shadows {
            // The footprint moved away from the light (in the shader).
            let base = shade.len() as u32;
            shade.extend(s.roof.iter().map(|p| vertex(rel(roof_at(p), 0.0), [0.0; 2], shadow, [0.0, EXTRUDE_SHADOW, 0.0, h as f32])));
            shade_i.extend(s.roof_indices.iter().map(|i| base + i));
        }
        // The walls; a hole's face into it (a courtyard).
        for (r, hole) in s.rings.iter().map(|r| (r, false)).chain(s.holes.iter().map(|r| (r, true))) {
            let edges = || (0..r.len()).filter_map(|i| Some((*r.get(i)?, *r.get((i + 1) % r.len())?)));
            let area2: f64 = edges().map(|(a, b)| a.x * b.y - b.x * a.y).sum();
            for (a, b) in edges() {
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                let len = dx.hypot(dy);
                if !(len > 0.0 && len.is_finite()) {
                    continue;
                }
                let n =
                    if (area2 > 0.0) != hole { [(dy / len) as f32, (-dx / len) as f32] } else { [(-dy / len) as f32, (dx / len) as f32] };
                let k = solid.len() as u32;
                for (c, z, t) in [(a, 0.0, 0.0), (b, 0.0, 0.0), (b, h, 1.0), (a, h, 1.0)] {
                    solid.push(vertex(rel(c, z), n, color, [t, EXTRUDE_WALL, lit, 0.0]));
                }
                solid_i.extend([k, k + 1, k + 2, k, k + 2, k + 3]);
                if s.shadows && !hole {
                    let k = shade.len() as u32;
                    for (c, fall) in [(a, 0.0), (b, 0.0), (b, h), (a, h)] {
                        shade.push(vertex(rel(c, 0.0), [0.0; 2], shadow, [0.0, EXTRUDE_SHADOW, 0.0, fall as f32]));
                    }
                    shade_i.extend([k, k + 1, k + 2, k, k + 2, k + 3]);
                }
            }
        }
    }
    let mut out = Vec::new();
    if !shade_i.is_empty() {
        out.push(gpu_scene::DrawCmd::Extrusion { origin, vertices: shade, indices: shade_i, shadows: true });
    }
    if !solid_i.is_empty() {
        out.push(gpu_scene::DrawCmd::Extrusion { origin, vertices: solid, indices: solid_i, shadows: false });
    }
    Some(out)
}

/// Point layers with at most this many features stand their markers up in a tilted view; a
/// bigger one keeps them drawn on the ground.
const MAX_UPRIGHT_POINTS: usize = 50_000;

/// How much bigger or smaller than its symbol an upright marker is drawn, at the most, as it
/// nears or recedes from the camera.
const UPRIGHT_SCALE: (f64, f64) = (0.25, 2.0);

/// Whether a tilted view of `map` stands `layer`'s point markers up, facing the viewer (as
/// MapLibre's circles do), rather than drawing them flat on the ground. A masked, swiped or
/// blended layer, an aggregation or graphics keep theirs on the ground.
fn stands_up(map: &Map, layer: &rustgis_map::Layer) -> bool {
    // Complete sources must use their total, never the pan-dependent interaction sample.
    // An unknown total conservatively uses complete ground-marker traversal.
    let rows = match layer.source.as_ref().and_then(|s| s.chunk_source()) {
        Some(source) => source.info().count,
        None => Some(layer.dataset.features.len() as u64),
    };
    layer.raster.is_none()
        && layer.geometry_type() == GeometryType::Point
        && layer.aggregation.is_none()
        && !layer.graphics
        && layer.mask.is_none()
        && layer.blend == rustgis_map::effects::Blend::Normal
        && map.swipe.is_none_or(|(id, _)| id != layer.id)
        && rows.is_some_and(|n| n <= MAX_UPRIGHT_POINTS as u64)
}

/// A point marker standing upright in a tilted view, facing the viewer.
#[derive(Clone, Debug)]
pub struct UprightMarker {
    /// Where it stands (the camera's coordinates).
    pub at: Coord,
    /// The ground's height under it (map units), on terrain; 0 without.
    pub height: f64,
    /// Its symbol, with the layer's transparency in its colours.
    pub symbol: Symbol,
    pub selected: bool,
}

impl UprightMarker {
    /// Where it stands in a tilted `w × h` view of `cam` (logical px), how much bigger or
    /// smaller than its symbol it is drawn there (above or below 1), and how far it has faded
    /// into the fog (0 to 1); `None` behind the camera or past the view's reach.
    pub fn place(&self, cam: &Camera, w: f64, h: f64) -> Option<(f64, f64, f64, f64)> {
        let k = cam.depth_scale(self.at, h).filter(|k| *k * rustgis_map::camera::FAR >= 1.0)?;
        let (u, v) = cam.ground_px(self.at);
        let up = (self.height / cam.res.max(1e-9)).clamp(-1e7, 1e7);
        let (x, y) = cam.project_raised(u, v, up, w, h).filter(|(x, y)| x.is_finite() && y.is_finite())?;
        // Fog from halfway to the reach, as on the ground.
        let far = rustgis_map::camera::FAR;
        let t = ((1.0 / k - far * 0.5) / (far * 0.5)).clamp(0.0, 1.0);
        Some((x, y, k.clamp(UPRIGHT_SCALE.0, UPRIGHT_SCALE.1), t * t * (3.0 - 2.0 * t)))
    }

    /// Its symbol drawn `k` times its size, faded `t` of the way into `fog`.
    pub fn look(&self, k: f64, fog: Rgba, t: f64) -> Symbol {
        let s = &self.symbol;
        let fade = |c: Rgba| c.lerp(fog.with_alpha(c.a()), t);
        Symbol {
            fill: fade(s.fill),
            stroke: fade(s.stroke),
            stroke_width: s.stroke_width * k as f32,
            size: s.size * k as f32,
            marker: s.marker,
        }
    }
}

/// The outline of a `marker` of radius `r` centred at (`x`, `y`), as [`draw_marker`] draws it
/// (a circle as a polygon): for drawing upright markers as meshes. Every outline is seen whole
/// from its centre, so a fan from the centre fills it.
pub fn marker_outline(marker: Marker, x: f32, y: f32, r: f32) -> Vec<(f32, f32)> {
    match marker {
        Marker::Circle => (0..24)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / 24.0;
                (x + r * a.cos(), y + r * a.sin())
            })
            .collect(),
        Marker::Square => {
            vec![(x - r * 0.9, y - r * 0.9), (x + r * 0.9, y - r * 0.9), (x + r * 0.9, y + r * 0.9), (x - r * 0.9, y + r * 0.9)]
        }
        Marker::Triangle => vec![(x, y - r * 1.1), (x + r, y + r * 0.75), (x - r, y + r * 0.75)],
        Marker::Diamond => vec![(x, y - r * 1.15), (x + r * 0.9, y), (x, y + r * 1.15), (x - r * 0.9, y)],
        Marker::Star => (0..10)
            .map(|i| {
                let a = std::f32::consts::PI * (i as f32) / 5.0 - std::f32::consts::FRAC_PI_2;
                let rr = if i % 2 == 0 { r * 1.2 } else { r * 0.5 };
                (x + rr * a.cos(), y + rr * a.sin())
            })
            .collect(),
    }
}

/// The point markers a tilted `view` of `map` stands upright ([`stands_up`]), bottom layer
/// first and each layer's far to near, so nearer markers draw over farther ones. Those on the
/// ground it shows, grown by `margin` of its size on every side (for a view that moves).
pub fn upright_markers(map: &mut Map, view: &View, margin: f64) -> Vec<UprightMarker> {
    if map.projection.is_globe() {
        return Vec::new();
    }
    let scale = map.ground_res_at(&view.camera) / rustgis_geom::PIXEL_M;
    let (flat, key) = (map.flat(), map.projection.code());
    let cull = view.camera.extent(view.width, view.height).padded(margin.clamp(CULL_MARGIN, 4.0), 0.0);
    let clip = map.view.clip_distance.map(|d| (view.camera.center_lonlat(), d));
    let held = map.held_by_group(scale);
    let group_opacity: std::collections::HashMap<u32, f32> =
        map.groups.iter().filter(|g| g.transparency > 0.0).flat_map(|g| g.children.iter().map(|c| (*c, g.opacity()))).collect();
    let upright: std::collections::HashSet<u32> = map.layers.iter().filter(|l| stands_up(map, l)).map(|l| l.id).collect();
    let mut order: Vec<usize> = (0..map.layers.len()).rev().collect();
    order.sort_by_key(|i| map.layers.get(*i).map_or(0, |l| l.priority));
    let (cam, h) = (view.camera, view.height);
    let ground = terrain::Ground::of(map, &view.camera);
    let mut out = Vec::new();
    for li in order {
        let Some(layer) = map.layers.get_mut(li) else { continue };
        if !layer.visible || !layer.in_scale_range(scale) || held.contains(&layer.id) || !upright.contains(&layer.id) {
            continue;
        }
        let opacity = layer.opacity() * group_opacity.get(&layer.id).copied().unwrap_or(1.0);
        let rows = layer.drawn_rows(scale);
        // Refreshes the projected copy for this projection.
        layer.projected_in(&flat, &key);
        let layer: &rustgis_map::Layer = layer;
        let mut here: Vec<(f64, UprightMarker)> = Vec::new();
        for (row, g) in layer.projected_cache().iter().enumerate() {
            if rows.get(row) != Some(&true) {
                continue;
            }
            let pts: &[Coord] = match g {
                Some(Geometry::Point(c)) => std::slice::from_ref(c),
                Some(Geometry::MultiPoint(v)) => v,
                _ => continue,
            };
            let lonlat = clip.and_then(|_| layer.dataset.features.get(row).and_then(|f| f.geometry.as_ref()).map(|g| g.extent()));
            if let (Some((centre, d)), Some(e)) = (clip, lonlat) {
                let near = Coord::new(centre.x.clamp(e.min_x, e.max_x), centre.y.clamp(e.min_y, e.max_y));
                if e.is_empty() || rustgis_geom::haversine_m(centre, near) > d {
                    continue;
                }
            }
            let mut symbol = layer.symbol(row);
            let see = |c: Rgba| c.with_alpha((f32::from(c.a()) * opacity).round().clamp(0.0, 255.0) as u8);
            (symbol.fill, symbol.stroke) = (see(symbol.fill), see(symbol.stroke));
            let selected = layer.selection.contains(&row);
            for c in pts.iter().filter(|c| c.is_finite() && cull.contains(**c)) {
                let Some(k) = cam.depth_scale(*c, h) else { continue };
                // On terrain, standing on the ground (its height in map units there).
                let height = ground.as_ref().and_then(|g| g.height(*c).map(|m| (m / g.per_unit(*c)).clamp(-1e9, 1e9))).unwrap_or(0.0);
                here.push((k, UprightMarker { at: *c, height, symbol, selected }));
            }
        }
        here.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.extend(here.into_iter().map(|(_, m)| m));
    }
    out
}

/// The point markers of a tilted view, stood up over its ground and extrusions.
fn markers_tilted(map: &mut Map, view: &View, pm: &mut Pixmap) {
    let (_, fog) = tilt_sky(map);
    let ppp = (f64::from(pm.width()) / view.width.max(1e-9)) as f32;
    let (pw, ph) = (pm.width() as f32, pm.height() as f32);
    for m in upright_markers(map, view, 0.0) {
        let Some((x, y, k, t)) = m.place(&view.camera, view.width, view.height) else { continue };
        let s = m.look(k, fog, t);
        let (x, y) = (x as f32 * ppp, y as f32 * ppp);
        let reach = (s.size.max(1.0) * 0.6 + s.stroke_width.max(0.0) + 4.0) * ppp;
        if x < -reach || y < -reach || x > pw + reach || y > ph + reach {
            continue;
        }
        draw_marker(pm, x, y, &s, 1.0, m.selected.then_some(SELECTION), ppp);
    }
}

/// The extruded layers of a tilted view, stood up in perspective over its ground, bottom layer
/// first (a drape of the moving view shows them flat).
fn extrusions_tilted(map: &mut Map, view: &View, pm: &mut Pixmap) {
    let scale = map.ground_res_at(&view.camera) / rustgis_geom::PIXEL_M;
    // Layers a hidden or out-of-scale group holds aren't drawn, as in `render_frame`.
    let held = map.held_by_group(scale);
    let (flat, key) = (map.flat(), map.projection.code());
    // Heights are metres on the map's body.
    let body = map.body().ratio();
    let (_, fog) = tilt_sky(map);
    let mut order: Vec<usize> = (0..map.layers.len()).rev().collect();
    order.sort_by_key(|i| map.layers.get(*i).map_or(0, |l| l.priority));
    for li in order {
        let Some(layer) = map.layers.get_mut(li) else { continue };
        if !layer.visible
            || !layer.in_scale_range(scale)
            || held.contains(&layer.id)
            || layer.raster.is_some()
            || layer.geometry_type() != GeometryType::Polygon
        {
            continue;
        }
        let Some(x) = layer.extrusion.clone() else { continue };
        let rows = layer.drawn_rows(scale);
        // Refreshes the projected copy for this projection.
        layer.thinned_in(&flat, &key, view.camera.res * rustgis_map::lod::THIN_PX);
        let layer: &rustgis_map::Layer = layer;
        let geoms: Vec<Option<&Geometry>> = layer.projected_cache().iter().map(Option::as_ref).collect();
        effects::extrude_tilted(pm, view, &layer.dataset, &geoms, &rows, &x, |c| flat.ground_per_unit(c) * body, fog, |r| layer.symbol(r));
    }
}

/// The most physical pixels a side of a tilted view's band is drawn with: past it, at a lower
/// pixel density. (`Camera::tilt_bands` limits a band's logical size the same way.)
const MAX_BAND_PX: f64 = 4096.0;

/// The view a band of a tilted `view` is drawn with: at the view's pixel density, or coarser
/// for a long band.
fn band_view(view: &View, b: &rustgis_map::camera::Band) -> View {
    let ppp = view.pixels_per_point.min(MAX_BAND_PX / b.width.max(b.height).max(1.0));
    View { width: b.width, height: b.height, camera: b.camera, pixels_per_point: ppp, ..*view }
}

/// Reads what a tilted view shows of the map's streamed layers, without waiting, as drawing it
/// does: a copy of the map drawn off the UI thread then finds the data already in place, and the
/// map itself holds what the view shows. True while reads are still on their way.
pub fn sync_tilted_view(map: &mut Map, view: &View) -> bool {
    let bands = view.camera.tilt_bands(view.width, view.height);
    sync_tilted(map, view, &bands, false)
}

/// Reads the streamed layers once for a tilted `view`: every layer drawn in one of its `bands`
/// (each drawn at its own scale, coarser far away). Reads are kept per extent, so a layer drawn
/// in two bands asks for the same extent twice and reads once. Returns whether a read is still
/// running.
fn sync_tilted(map: &mut Map, view: &View, bands: &[rustgis_map::camera::Band], block: bool) -> bool {
    let lonlat = view_lonlat(map, view);
    let mut pending = false;
    for b in bands {
        let scale = map.ground_res_at(&b.camera) / rustgis_geom::PIXEL_M;
        pending |= map.sync_sources(&lonlat, scale, block);
    }
    pending
}

/// Fills a tilted view's picture band by band: each pixel takes its colour from the nearest
/// band under it, fading into the fog toward the view's reach; then the sky above the horizon
/// shades from the fog to the sky colour at the top.
struct Warp {
    look: rustgis_map::camera::GroundLookup,
    /// Physical pixels per logical point, across and down.
    k: (f64, f64),
    sky_y: f64,
    sky: [f32; 4],
    fog: [f32; 4],
    /// The pixels a band has given a colour.
    filled: Vec<bool>,
}

impl Warp {
    fn new(view: &View, map: &Map) -> Warp {
        let (pw, ph) = view.pixel_size();
        let (sky, fog) = tilt_sky(map);
        let rgba = |c: Rgba| [f32::from(c.r()), f32::from(c.g()), f32::from(c.b()), 255.0];
        Warp {
            look: view.camera.ground_lookup(view.width, view.height),
            k: (f64::from(pw) / view.width.max(1e-9), f64::from(ph) / view.height.max(1e-9)),
            sky_y: view.camera.sky_y(view.height).unwrap_or(0.0).max(1e-9),
            sky: rgba(sky),
            fog: rgba(fog),
            filled: vec![false; pw as usize * ph as usize],
        }
    }

    /// The logical point at the centre of pixel (px, py).
    fn point(&self, px: usize, py: usize) -> (f64, f64) {
        ((px as f64 + 0.5) / self.k.0, (py as f64 + 0.5) / self.k.1)
    }

    /// Colours the pixels still empty that band picture `pic` (drawn for `v`) has under them,
    /// on the screen rows between `top` and `bottom` (logical points) where its ground lies.
    fn band(&mut self, pm: &mut Pixmap, v: &View, pic: &Pixmap, top: f64, bottom: f64) {
        let (pw, ph) = (pm.width() as usize, pm.height() as usize);
        let far = rustgis_map::camera::FAR;
        // A row's worth of slack each way for sampling at the edges.
        let row = |y: f64| ((y * self.k.1).floor().max(0.0) as usize).min(ph);
        let (r0, r1) = (row(top - 1.0 / self.k.1), (row(bottom + 2.0 / self.k.1)).min(ph));
        let out = pm.pixels_mut();
        let span = r0 * pw..r1.max(r0) * pw;
        for (i, filled) in self.filled.iter_mut().enumerate().take(span.end).skip(span.start) {
            if *filled {
                continue;
            }
            let (x, y) = ((i % pw) as f64 + 0.5, (i / pw) as f64 + 0.5);
            let Some((m, depth)) = self.look.at(x / self.k.0, y / self.k.1) else { continue };
            let Some(ground) = sample_band(v, pic, m) else { continue };
            // Fog from halfway to the reach.
            let t = ((depth - far * 0.5) / (far * 0.5)).clamp(0.0, 1.0) as f32;
            if let Some(dst) = out.get_mut(i) {
                *dst = premultiplied(mix(ground, self.fog, t * t * (3.0 - 2.0 * t)));
            }
            *filled = true;
        }
    }

    /// Fills what no band covered: the sky, and fog on ground past every band.
    fn finish(&self, pm: &mut Pixmap) {
        let pw = pm.width() as usize;
        let out = pm.pixels_mut();
        for (i, dst) in out.iter_mut().enumerate() {
            if self.filled.get(i).copied().unwrap_or(true) {
                continue;
            }
            let (x, y) = self.point(i % pw.max(1), i / pw.max(1));
            *dst = premultiplied(match self.look.at(x, y) {
                Some(_) => self.fog,
                None => mix(self.sky, self.fog, (y / self.sky_y).clamp(0.0, 1.0) as f32),
            });
        }
    }
}

fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// A premultiplied pixel from 0–255 premultiplied channels.
fn premultiplied(c: [f32; 4]) -> tiny_skia::PremultipliedColorU8 {
    let a = c[3].clamp(0.0, 255.0);
    let ch = |v: f32| v.clamp(0.0, a).round() as u8;
    tiny_skia::PremultipliedColorU8::from_rgba(ch(c[0]), ch(c[1]), ch(c[2]), a.round() as u8)
        .unwrap_or(tiny_skia::PremultipliedColorU8::TRANSPARENT)
}

/// The colour (premultiplied, 0–255) of band picture `pm`, drawn for `v`, at Mercator point
/// `m`, sampled bilinearly; `None` outside it.
fn sample_band(v: &View, pm: &Pixmap, m: Coord) -> Option<[f32; 4]> {
    let (bw, bh) = (pm.width() as f64, pm.height() as f64);
    let (sx, sy) = v.camera.to_screen(m, v.width, v.height);
    let (fx, fy) = (sx * bw / v.width.max(1e-9) - 0.5, sy * bh / v.height.max(1e-9) - 0.5);
    if !(fx >= -0.5 && fy >= -0.5 && fx <= bw - 0.5 && fy <= bh - 0.5) {
        return None;
    }
    let (x0, y0) = (fx.floor().max(0.0), fy.floor().max(0.0));
    let (tx, ty) = ((fx - x0).clamp(0.0, 1.0) as f32, (fy - y0).clamp(0.0, 1.0) as f32);
    let (x0, y0) = (x0 as usize, y0 as usize);
    let (x1, y1) = ((x0 + 1).min(pm.width() as usize - 1), (y0 + 1).min(pm.height() as usize - 1));
    let px = pm.pixels();
    let at = |x: usize, y: usize| px.get(y * pm.width() as usize + x).map(|p| [p.red(), p.green(), p.blue(), p.alpha()]);
    let (a, b, c, d) = (at(x0, y0)?, at(x1, y0)?, at(x0, y1)?, at(x1, y1)?);
    Some(std::array::from_fn(|i| {
        let top = f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * tx;
        let bottom = f32::from(c[i]) + (f32::from(d[i]) - f32::from(c[i])) * tx;
        top + (bottom - top) * ty
    }))
}

/// A view turned to a bearing, on the CPU: the map is drawn facing north over the box around
/// the turned view, then turned into place. Labels are placed in the turned view and drawn
/// upright, as MapLibre keeps them.
fn render_turned(map: &mut Map, view: &View, full_res_rasters: bool) -> Option<Rendered> {
    let (bw, bh) = view.camera.rotated_size(view.width, view.height);
    let north = View { width: bw.ceil() + 2.0, height: bh.ceil() + 2.0, camera: Camera { bearing: 0.0, ..view.camera }, ..*view };
    let Some(RenderOutput::Cpu(r)) = render_frame(map, &north, full_res_rasters, Backend::Cpu, Labels::Turned(view)) else { return None };
    let (w, h) = view.pixel_size();
    let mut pm = Pixmap::new(w, h)?;
    // Physical pixels per logical point of each frame (the north-up one may have been capped
    // at MAX_SIDE).
    let k_out = w as f64 / view.width.max(1e-9);
    let k_in = r.pixmap.width() as f64 / north.width.max(1e-9);
    let s = k_out / k_in.max(1e-9);
    let b = view.camera.bearing.to_radians();
    let (c, n) = (b.cos() * s, b.sin() * s);
    let (iw, ih) = (r.pixmap.width() as f64 / 2.0, r.pixmap.height() as f64 / 2.0);
    let (tx, ty) = (w as f64 / 2.0 - (c * iw + n * ih), h as f64 / 2.0 - (c * ih - n * iw));
    let t = Transform::from_row(c as f32, -n as f32, n as f32, c as f32, tx as f32, ty as f32);
    let paint = PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() };
    pm.draw_pixmap(0, 0, r.pixmap.as_ref(), &paint, t, None);
    for l in &r.labels {
        if let Some(icon) = &l.icon {
            draw_icon(&mut pm, icon, view.pixels_per_point as f32);
        }
    }
    Some(Rendered { pixmap: pm, ..r })
}

/// Draws one north-up frame. `labels_to` says where its labels go: a turned or tilted view the
/// frame stands in for places them itself (and draws their icons).
fn render_frame(map: &mut Map, view: &View, full_res_rasters: bool, backend: Backend, mut labels_to: Labels<'_>) -> Option<RenderOutput> {
    let (w, h) = view.pixel_size();
    let mut pm = Pixmap::new(w, h)?;
    // Streamed layers read the view: in the background for the screen, in place for an export
    // (the full-resolution render), which waits for them; only the layers drawn at the view's
    // scale read. A tilted view's bands share its read.
    let scale = map.ground_res_at(&view.camera) / rustgis_geom::PIXEL_M;
    let data_pending = match labels_to {
        Labels::Tilted(..) | Labels::Shared => false,
        _ => map.sync_sources(&view_lonlat(map, view), scale, full_res_rasters),
    };
    let globe = map.projection.is_globe().then(|| Globe::new(&view.camera, view.width, view.height));
    let (flat, key) = (map.flat(), map.projection.code());
    let custom = matches!(flat, rustgis_map::projection::Flat::Custom(_)) && globe.is_none();
    let _antialias = Antialias::set(!map.view.fast);
    let ctx = Ctx {
        view,
        ppp: view.pixels_per_point as f32,
        wireframe: map.view.wireframe,
        cull: cull_extent(view, CULL_MARGIN),
        globe,
        sprites: Default::default(),
    };
    // The Gpu backend draws flat, effect-free frames; anything else stays on the CPU pixmap.
    let gpu = backend.is_gpu()
        && globe.is_none()
        && map.view.effects.is_none()
        && map.view.color_vision == rustgis_map::view::ColorVision::Normal;
    // The effective backend: the Gpu arm at the end only runs when the frame is compatible.
    let backend = if gpu { backend } else { Backend::Cpu };
    // GPU draw commands; between them, tiny-skia-only content accumulates in `pm` and is
    // flushed as ordered Image commands (`flush_acc`, gated by `cpu_dirty`).
    let mut cmds: Vec<gpu_scene::DrawCmd> = Vec::new();
    // Map units per physical pixel: the decimation threshold scale (`Ctx::ring_path`'s 0.35 px
    // rule is applied in screen space) and the stroke shader's width scale.
    let map_per_px = view.camera.res / view.pixels_per_point;
    // On the globe nothing is culled by the flat extent: the horizon clips instead.
    let visible = |e: &Extent| match &globe {
        Some(g) => g.may_see(e),
        None => e.intersects(&ctx.cull),
    };
    // GPU meshes reach further (`GPU_CULL_MARGIN`), so the camera fast path can move them.
    let mesh_cull = cull_extent(view, GPU_CULL_MARGIN);
    let mesh_visible = |e: &Extent| match &globe {
        Some(g) => g.may_see(e),
        None => e.intersects(&mesh_cull),
    };

    // Basemap.
    let (water, land, outline, grat) = map.palette();
    match &globe {
        Some(g) => globe_backdrop(&mut pm, g, water, space(view, map), view.pixels_per_point as f32),
        None if !gpu => pm.fill(tiny_skia::Color::from_rgba8(water.r(), water.g(), water.b(), 255)),
        // The Gpu scene carries the water backdrop as its clear colour instead — an
        // always-full-screen Image here would poison `has_images` and kill the camera
        // fast path.
        None => {}
    }
    // Set when tiny-skia actually rasterizes into the accumulator.
    let mut cpu_dirty = false;
    if land != water {
        let fill = paint(land, 1.0);
        let stroke = (outline.a() > 0).then(|| (paint(outline, 1.0), 0.6));
        let custom_world = custom.then(|| world_in(&flat, &key));
        let outlines: &Vec<Geometry> = match (&globe, &custom_world) {
            (Some(_), _) => world_lonlat(),
            (None, Some(w)) => w,
            (None, None) => world(),
        };
        for g in outlines {
            if let Geometry::Polygon(polys) = g
                && visible(&g.extent())
            {
                let mut tessellated = false;
                if gpu {
                    flush_acc(&mut pm, &mut cmds, &mut cpu_dirty)?;
                    let e = g.extent();
                    tessellated =
                        mesh::collect_basemap(&mut cmds, polys, [e.center().x, e.center().y], land, outline, ctx.ppp, map_per_px).is_ok();
                }
                if gpu && !tessellated {
                    // Lyon refused: tiny-skia still draws the outline.
                    ctx.polygon(&mut pm, polys, Some(fill.clone()), stroke.clone());
                    cpu_dirty = true;
                } else if !gpu {
                    ctx.polygon(&mut pm, polys, Some(fill.clone()), stroke.clone());
                }
            }
        }
    }
    if map.graticule && grat.a() > 0 {
        let p = paint(grat, 1.0);
        let step = if view.camera.res > 8000.0 {
            30.0
        } else if view.camera.res > 2000.0 {
            15.0
        } else if view.camera.res > 400.0 {
            5.0
        } else {
            1.0
        };
        let (lat_max, lat_min) = if globe.is_some() { (90.0, -90.0) } else { (85.0, -85.0) };
        let mut lines = Vec::new();
        let mut lon = -180.0;
        while lon <= 180.0 {
            lines.push((0..=36).map(|i| Coord::new(lon, lat_min + (lat_max - lat_min) * i as f64 / 36.0)).collect::<Vec<_>>());
            lon += step;
        }
        let mut lat = -90.0 + step;
        while lat < 90.0 {
            lines.push((0..=72).map(|i| Coord::new(-180.0 + i as f64 * 5.0, lat)).collect::<Vec<_>>());
            lat += step;
        }
        let lines = if globe.is_some() {
            lines
        } else if custom {
            // Parallels are cut at the map's edge like any line (#209).
            match flat.project(&Geometry::Line(lines)) {
                Geometry::Line(parts) => parts,
                _ => Vec::new(),
            }
        } else {
            lines.into_iter().map(|l| l.into_iter().map(lonlat_to_merc).collect()).collect()
        };
        if gpu {
            let mut e = Extent::EMPTY;
            for l in &lines {
                for c in l {
                    e.expand(*c);
                }
            }
            flush_acc(&mut pm, &mut cmds, &mut cpu_dirty)?;
            let tessellated = !e.is_empty()
                && mesh::collect_graticule(&mut cmds, &lines, [e.center().x, e.center().y], grat, ctx.ppp, map_per_px).is_ok();
            if !tessellated {
                ctx.line(&mut pm, &lines, &p, 0.6);
                cpu_dirty = true;
            }
        } else {
            ctx.line(&mut pm, &lines, &p, 0.6);
        }
    }

    // Web tile layers, over the built-in basemap and its graticule.
    let tile_place = match &globe {
        Some(g) => raster::Place::Globe(g),
        None => raster::Place::Flat(&flat, &key),
    };
    // Hidden tile layers draw nothing: marking dirty anyway would emit an empty full-screen
    // Image command (a pointless upload, and it poisons the camera fast path's `has_images`).
    // On the GPU raster tile layers are placed as tiles, which move with the camera.
    let tile_gpu = (gpu && map.tiles.iter().any(|t| t.draws() && t.opacity > 0.0)).then_some(tiles::Gpu {
        cmds: &mut cmds,
        dirty: &mut cpu_dirty,
        // A tilted view's bands are drawn at their own densities: their tiles as pictures.
        meshes: !matches!(labels_to, Labels::Tilted(..)),
        // Only a frame for the view itself (not a turned or tilted view's north-up stand-in).
        live: backend == Backend::GpuLive && matches!(labels_to, Labels::Own),
    });
    let tile_out = tiles::draw(&mut pm, map, view, tile_place, !map.label_display.paused && map.label_lock.is_none(), tile_gpu);
    if !gpu {
        cpu_dirty |= map.tiles.iter().any(|t| t.draws());
    }

    // Layers, bottom first.
    let mut label_candidates: Vec<Candidate> = Vec::new();
    let display = map.label_display;
    if !display.locked {
        map.label_lock = None;
    }
    let lock: Option<std::collections::HashSet<(u32, usize)>> = map.label_lock.as_ref().map(|v| v.iter().copied().collect());
    let distance = viewing_distance(map.ground_res_at(&view.camera), view.height);
    // Graphics text: always drawn, and other labels keep clear of it.
    let mut fixed_labels: Vec<PlacedLabel> = Vec::new();
    let tokens = rustgis_map::graphics::TokenValues::of(map, scale);
    let n_layers = map.layers.len();
    // Masks: layers shown only inside another layer's polygons, or left of the swipe divider.
    let masks = layer_masks(map, &ctx, w, h, &flat, &key, scale)?;
    // Bottom first; a higher depth priority draws later (on top) whatever the order.
    let mut order: Vec<usize> = (0..n_layers).rev().collect();
    order.sort_by_key(|i| map.layers.get(*i).map_or(0, |l| l.priority));
    let clip = map.view.clip_distance.map(|d| (view.camera.center_lonlat(), d));
    // Each layer group's visibility, scale range and transparency apply to its children (#413).
    let held = map.held_by_group(scale);
    let group_opacity: std::collections::HashMap<u32, f32> =
        map.groups.iter().filter(|g| g.transparency > 0.0).flat_map(|g| g.children.iter().map(|c| (*c, g.opacity()))).collect();
    // The point layers whose markers stand up over a tilted view (`upright_markers`) rather than
    // lie in its bands.
    let upright: std::collections::HashSet<u32> = match labels_to {
        Labels::Tilted(..) | Labels::Shared => map.layers.iter().filter(|l| stands_up(map, l)).map(|l| l.id).collect(),
        _ => Default::default(),
    };
    // Extrusion heights are metres on the map's body.
    let body = map.body().ratio();
    for li in order {
        let Some(layer) = map.layers.get_mut(li) else { continue };
        if !layer.visible || !layer.in_scale_range(scale) || held.contains(&layer.id) {
            continue;
        }
        let opacity = layer.opacity() * group_opacity.get(&layer.id).copied().unwrap_or(1.0);
        let mask = masks.iter().find(|(id, _)| *id == layer.id).map(|(_, m)| m);
        let blend = blend_mode(layer.blend);
        // Drawn on its own and composited when it is see-through, masked or blended.
        let offscreen = opacity < 0.999 || mask.is_some() || layer.blend != rustgis_map::effects::Blend::Normal;
        let composite = |pm: &mut Pixmap, target: &Pixmap| {
            pm.draw_pixmap(
                0,
                0,
                target.as_ref(),
                &PixmapPaint { opacity, blend_mode: blend, ..PixmapPaint::default() },
                Transform::identity(),
                mask,
            );
        };
        if let Some(rl) = &layer.raster {
            if let Some(data) = &rl.data {
                let place = match &globe {
                    Some(g) => raster::Place::Globe(g),
                    None => raster::Place::Flat(&flat, &key),
                };
                let stats = rl.masked_stats();
                let stats = stats.as_ref().map(|s| s.as_slice());
                if offscreen {
                    let mut target = Pixmap::new(w, h)?;
                    raster::draw(&mut target, data, &rl.style, stats, view, 1.0, full_res_rasters, place);
                    composite(&mut pm, &target);
                } else {
                    raster::draw(&mut pm, data, &rl.style, stats, view, opacity, full_res_rasters, place);
                }
                cpu_dirty = true;
            }
            continue;
        }
        let source = layer.source.as_ref().and_then(|s| s.chunk_source());
        let complete_source = source.is_some();
        // LOD is chosen for the entire layer, not separately for each chunk. Both the
        // activation threshold and the required saving must match an ordinary whole read.
        let thin_tol = view.camera.res / view.pixels_per_point.max(0.01) * rustgis_map::lod::THIN_PX;
        let mut thin_complete = false;
        if globe.is_none()
            && layer.geometry_type() != GeometryType::Point
            && let (Some(source), Some(level)) = (&source, rustgis_map::lod::level(thin_tol))
        {
            let (mut eligible, mut all, mut kept) = (0usize, 0usize, 0usize);
            let result = visit_complete(source.as_ref(), &mut |features| {
                for feature in features {
                    if let Some(g) = feature.geometry {
                        let g = flat.project(&g);
                        eligible = eligible.saturating_add(rustgis_map::lod::thinnable(&g));
                        all = all.saturating_add(rustgis_map::lod::vertices(&g));
                        kept = kept
                            .saturating_add(rustgis_map::lod::vertices(&rustgis_map::lod::thin(&g, rustgis_map::lod::tolerance(level))));
                    }
                }
                Ok(())
            });
            if let Err(e) = result {
                if let Some(s) = &mut layer.source {
                    s.error = Some(e.to_string());
                }
                return None;
            }
            thin_complete = eligible > rustgis_map::lod::THIN_ABOVE && kept as f64 <= all as f64 * (1.0 - rustgis_map::lod::MIN_SAVING);
        }

        let mut target = if offscreen { Pixmap::new(w, h)? } else { Pixmap::new(1, 1)? };
        let direct = !offscreen;
        let mut bins = effects::AggregationBins::default();
        let mut aggregate_symbol = None;
        // Only selected geometry is retained; paint directly after the whole layer to avoid
        // a second full-size canvas and transparent-intermediate rounding differences.
        let selected_order: std::collections::HashMap<u64, usize> = if complete_source {
            layer.selection.iter().enumerate().filter_map(|(rank, row)| layer.dataset.features.get(*row).map(|f| (f.id, rank))).collect()
        } else {
            Default::default()
        };
        let mut highlights: Vec<(usize, Geometry, Symbol)> = Vec::new();
        let mut extrusions = if complete_source
            && layer.geometry_type() == GeometryType::Polygon
            && layer.extrusion.is_some()
            && globe.is_none()
            && !matches!(labels_to, Labels::Tilted(..) | Labels::Shared)
        {
            match effects::ExtrusionSpool::new() {
                Ok(spool) => Some(spool),
                Err(e) => {
                    if let Some(source) = &mut layer.source {
                        source.error = Some(e.to_string());
                    }
                    return None;
                }
            }
        } else {
            None
        };
        let mut draw_chunk = |layer: &mut rustgis_map::Layer| -> rustgis_data::Result<()> {
            // The definition query and the display filters that apply at this scale (a feature draws
            // only if it matches all of them).
            let mut rows = layer.drawn_rows(scale);
            // View clipping: features entirely beyond the distance from the view's centre are left
            // out; one reaching into it (its box's nearest point within the distance) still draws.
            if let Some((centre, d)) = clip {
                for (r, keep) in rows.iter_mut().enumerate() {
                    if *keep {
                        let e =
                            layer.dataset.features.get(r).and_then(|f| f.geometry.as_ref()).map(|g| g.extent()).filter(|e| !e.is_empty());
                        *keep = e.is_some_and(|e| {
                            let near = Coord::new(centre.x.clamp(e.min_x, e.max_x), centre.y.clamp(e.min_y, e.max_y));
                            rustgis_geom::haversine_m(centre, near) <= d
                        });
                    }
                }
            }
            // Drawn from a copy thinned to under a third of a pixel: a detailed layer seen whole
            // otherwise walks many vertices per pixel on every frame.
            if globe.is_none() {
                if complete_source {
                    layer.projected_in(&flat, &key);
                } else {
                    layer.thinned_in(&flat, &key, thin_tol);
                }
            }
            // Everything below only reads the layer: borrow rather than copy its data each frame.
            let layer: &rustgis_map::Layer = layer;
            let label_row = |row: usize| if complete_source { layer.dataset.features.get(row).map_or(row, |f| f.id as usize) } else { row };
            let (labels, selection, ds_ptr) = (&layer.labels, &layer.selection, &layer.dataset);
            // The globe draws the lon/lat geometry itself; the flat map its Web Mercator copy.
            let geoms: Vec<Option<&Geometry>> = if globe.is_some() {
                ds_ptr.features.iter().map(|f| f.geometry.as_ref()).collect()
            } else {
                layer.projected_cache().iter().map(Option::as_ref).collect()
            };
            // What the features draw from, and their extents for culling (on the flat map, cached).
            // Only the fills and strokes below draw thinned: the selection, labels, aggregation and
            // extrusion keep `geoms`, the full geometry, for their exact positions.
            let chunk_thinned: Vec<Option<Geometry>> = if thin_complete {
                let tolerance = rustgis_map::lod::level(thin_tol).map_or(thin_tol, rustgis_map::lod::tolerance);
                geoms.iter().map(|g| g.map(|g| rustgis_map::lod::thin(g, tolerance))).collect()
            } else {
                Vec::new()
            };
            let drawn: Vec<Option<&Geometry>> = if globe.is_some() {
                geoms.clone()
            } else if complete_source {
                if thin_complete { chunk_thinned.iter().map(Option::as_ref).collect() } else { geoms.clone() }
            } else {
                layer.thinned_cache(thin_tol).iter().map(Option::as_ref).collect()
            };
            let extents = if globe.is_some() { &[][..] } else { layer.projected_extents() };
            // GPU-eligible: a plain vector layer drawn at full opacity with no mask, blend,
            // aggregation, extrusion or per-row graphics (an inert aggregation/extrusion on the
            // "wrong" geometry type still falls back — conservative and correct).
            // Stacked line strokes are offset in screen space, which the GPU meshes don't do.
            let gpu_layer = gpu
                && !offscreen
                && layer.aggregation.is_none()
                && layer.extrusion.is_none()
                && !layer.graphics
                && layer.line_strokes.is_empty()
                && !upright.contains(&layer.id);
            // Lyon refuses degenerate geometry tiny-skia draws fine: collect_layer reports it,
            // and the layer runs the CPU branch below like any ineligible one.
            let mut gpu_done = false;
            if gpu_layer {
                flush_acc(&mut pm, &mut cmds, &mut cpu_dirty)
                    .ok_or_else(|| rustgis_data::DataError::Format("cannot allocate render target".into()))?;
                gpu_done = mesh::collect_layer(&mut cmds, layer, &geoms, &rows, &mesh_visible, ctx.wireframe, ctx.ppp, map_per_px).is_ok();
            }
            // False for a layer drawn elsewhere: nothing goes into the picture for it.
            let mut drawn_here = true;
            if !gpu_done {
                let dest = if direct { &mut pm } else { &mut target };
                let aggregate = layer.aggregation.filter(|_| layer.geometry_type() == GeometryType::Point);
                let extrude = layer.extrusion.as_ref().filter(|_| layer.geometry_type() == GeometryType::Polygon && globe.is_none());
                if let Some(a) = aggregate {
                    bins.add((w, h), &ctx, &geoms, &rows, a);
                    aggregate_symbol.get_or_insert_with(|| layer.symbol(0));
                } else if extrude.is_some() && matches!(labels_to, Labels::Tilted(..)) {
                    // Stood up in perspective over the tilted view (`render_tilted`); its labels still
                    // come from here.
                    drawn_here = false;
                } else if upright.contains(&layer.id) {
                    // Stood up facing the viewer over the tilted view (`upright_markers`); its labels
                    // still come from here.
                    drawn_here = false;
                } else if let Some(x) = extrude.filter(|_| !matches!(labels_to, Labels::Shared)) {
                    let px_per_m = view.pixels_per_point / (map_ground_res(view, &flat) * body);
                    if let Some(spool) = &mut extrusions {
                        spool.add(&ctx, ds_ptr, &geoms, &rows, x, px_per_m, |r| layer.symbol(r))?;
                    } else {
                        effects::extrude(dest, &ctx, ds_ptr, &geoms, &rows, x, px_per_m, |r| layer.symbol(r));
                    }
                } else {
                    for (row, g) in drawn.iter().enumerate() {
                        let Some(g) = g else { continue };
                        if rows.get(row) != Some(&true) || !visible(&extents.get(row).copied().unwrap_or_else(|| g.extent())) {
                            continue;
                        }
                        // A picture draws in its row's turn, so graphics keep their order. (On the globe
                        // the geometry and `px` are both longitude/latitude, so the corners land right.)
                        if layer.graphics && rustgis_map::graphics::kind(ds_ptr, row) == "picture" {
                            let e = g.extent();
                            let (x0, y0) = ctx.px(Coord::new(e.min_x, e.max_y));
                            let (x1, y1) = ctx.px(Coord::new(e.max_x, e.min_y));
                            let (pw, ph) = (dest.width() as f32, dest.height() as f32);
                            if x0.max(x1) >= 0.0 && x0.min(x1) <= pw && y0.max(y1) >= 0.0 && y0.min(y1) <= ph {
                                picture::draw(dest, ds_ptr, row, (x0, y0, x1, y1), if direct { opacity } else { 1.0 });
                            }
                            continue;
                        }
                        let sym = layer.symbol(row);
                        // A feature under a pixel across: blend its coverage into the pixels it touches
                        // (a dense layer seen whole is mostly these, and a path each is slow).
                        // Not stacked strokes: a splat would draw them unshifted, in one colour.
                        if globe.is_none()
                            && !layer.graphics
                            && layer.line_strokes.is_empty()
                            && let Some(e) = extents.get(row)
                            && ctx.splat(dest, e, g, &sym)
                        {
                            continue;
                        }
                        match g {
                            Geometry::Line(parts) if !layer.line_strokes.is_empty() && !layer.graphics => {
                                ctx.stacked_line(dest, parts, &sym, &layer.line_strokes)
                            }
                            _ => ctx.feature(dest, g, &sym),
                        }
                    }
                }
                cpu_dirty |= drawn_here;
            }
            if !complete_source {
                if let (Some(a), Some(base)) = (layer.aggregation, aggregate_symbol) {
                    bins.draw(if direct { &mut pm } else { &mut target }, &ctx, a, &base);
                }
                if !direct {
                    composite(&mut pm, &target);
                }
            }
            // An upright marker shows its own selection.
            for row in selection.iter().filter(|_| !upright.contains(&layer.id)) {
                if let Some(Some(g)) = geoms.get(*row)
                    && rows.get(*row) == Some(&true)
                {
                    // A text graphic's point is invisible: its label carries the frame (`framed`),
                    // drawn with the text wherever that lands (turned, tilted or on the globe).
                    if layer.graphics && rustgis_map::graphics::label(ds_ptr, *row).is_some() {
                        continue;
                    }
                    // Follow the layer's own backend: when it fell back to the CPU pixmap, the
                    // highlight must too — tessellated here, the layer's later-flushed Image would
                    // draw over it (the CPU path draws highlights over the layer).
                    if complete_source {
                        let rank = ds_ptr.features.get(*row).and_then(|f| selected_order.get(&f.id)).copied().unwrap_or(usize::MAX);
                        highlights.push((rank, (**g).clone(), layer.symbol(*row)));
                    } else if gpu_done && mesh::collect_highlight(&mut cmds, g, &layer.symbol(*row), ctx.ppp, map_per_px).is_ok() {
                        // highlight tessellated into the scene, right after its layer
                    } else {
                        ctx.highlight(&mut pm, g, &layer.symbol(*row), if layer.graphics { &[] } else { &layer.line_strokes });
                        cpu_dirty = true;
                    }
                }
            }
            if layer.graphics {
                for (row, g) in geoms.iter().enumerate() {
                    let Some(g) = g else { continue };
                    if rows.get(row) != Some(&true) {
                        continue;
                    }
                    if let (Some(t), Some(c)) = (rustgis_map::graphics::label(ds_ptr, row), g.label_point())
                        && opacity > 0.0
                        && let Some((x, y)) = ctx.screen(c)
                    {
                        let base = rustgis_map::graphics::text_color(ds_ptr, row);
                        let color = base.with_alpha((base.a() as f32 * opacity).round() as u8);
                        fixed_labels.push(PlacedLabel {
                            text: rustgis_map::graphics::expand(t, &tokens),
                            x: x as f32,
                            y: y as f32,
                            size: rustgis_map::graphics::font_size(ds_ptr, row),
                            color,
                            halo: Some(halo_for(color)),
                            bold: rustgis_map::graphics::bold(ds_ptr, row),
                            icon: None,
                            align: 0.0,
                            framed: selection.contains(&row),
                            italic: rustgis_map::graphics::italic(ds_ptr, row),
                            spacing: rustgis_map::graphics::letter_spacing(ds_ptr, row),
                            line_align: rustgis_map::graphics::align(ds_ptr, row),
                            angle: None,
                            collide: Default::default(),
                        });
                    }
                }
            }
            // Each feature is labelled by the first label class whose clause takes it, Default last;
            // that class draws it when shown, in range and on a field the layer has (#618).
            let label_fields: Vec<Option<usize>> = if labels.enabled && !display.paused {
                labels.in_order().map(|c| ds_ptr.field_index(&c.field).filter(|_| c.visible && c.in_range(scale, distance))).collect()
            } else {
                Vec::new()
            };
            if label_fields.iter().any(Option::is_some) {
                let classes = rustgis_map::LabelMatcher::new(labels, ds_ptr);
                let lid = layer.id;
                for (row, g) in geoms.iter().enumerate() {
                    let Some(g) = g else { continue };
                    if rows.get(row) != Some(&true) || lock.as_ref().is_some_and(|k| !k.contains(&(lid, label_row(row)))) {
                        continue;
                    }
                    let Some(c) = g.label_point() else { continue };
                    if globe.is_none() && !ctx.cull.contains(c) {
                        continue;
                    }
                    let Some((x, y)) = ctx.screen(c) else { continue };
                    // The clauses are read for the features in view only.
                    let Some((ci, class)) = classes.class_of(ds_ptr, row) else { continue };
                    let Some(fi) = label_fields.get(ci).copied().flatten() else { continue };
                    let v = ds_ptr.value(row, fi);
                    if v.is_null() {
                        continue;
                    }
                    let text = v.to_string();
                    let is_point = g.kind() == GeometryType::Point;
                    let marker = if is_point { layer.symbol(row).size } else { 0.0 };
                    let (dx, dy) = class.placement.offset(
                        text::block_width(&text, class.size, class.bold),
                        text::block_height(&text, class.size),
                        marker,
                        is_point,
                    );
                    // Bigger polygons get priority.
                    let pri = match g {
                        Geometry::Polygon(_) => {
                            let e = g.extent();
                            // (An extent of no number counts as none.)
                            let a = (e.width() * e.height()).sqrt();
                            if a.is_finite() { a.clamp(0.0, 1e8) } else { 0.0 }
                        }
                        _ => 0.0,
                    } + (n_layers - li) as f64 * 1e9;
                    label_candidates.push((
                        PlacedLabel {
                            text,
                            x: x as f32 + dx,
                            y: y as f32 + dy,
                            size: class.size,
                            color: class.color,
                            halo: class.halo.then_some(halo_for(class.color)),
                            bold: class.bold,
                            icon: None,
                            align: 0.0,
                            framed: false,
                            italic: false,
                            spacing: 0.0,
                            line_align: TextAlign::Left,
                            angle: None,
                            collide: Default::default(),
                        },
                        pri,
                        (lid, label_row(row)),
                    ));
                }
            }
            if complete_source && matches!(labels_to, Labels::Own | Labels::Turned(_)) {
                compact_label_candidates(&mut label_candidates);
            }
            Ok(())
        };
        if let Some(source) = source {
            let mut chunk = layer.clone();
            chunk.source = None;
            chunk.clear_cache();
            chunk.dataset.features = Vec::new().into();
            // All coordinates are read here: filtering only to the viewport can change labels,
            // shadows and screen-space effects whose geometry lies beyond its boundary.
            let result = visit_complete(source.as_ref(), &mut |features| {
                chunk.clear_cache();
                chunk.selection =
                    features.iter().enumerate().filter_map(|(r, f)| selected_order.contains_key(&f.id).then_some(r)).collect();
                chunk.dataset.features = features.into();
                chunk.touch_geometry();
                draw_chunk(&mut chunk)?;
                chunk.clear_cache();
                chunk.dataset.features = Vec::new().into();
                Ok(())
            });
            if let Err(e) = result {
                if let Some(s) = &mut layer.source {
                    s.error = Some(e.to_string());
                }
                return None;
            }
            if let Some(source) = &mut layer.source {
                source.error = None;
            }
        } else if let Err(e) = draw_chunk(layer) {
            log::warn!("cannot render layer '{}': {e}", layer.name);
            return None;
        }
        if let (Some(spool), Some(x)) = (&mut extrusions, &layer.extrusion)
            && let Err(e) = spool.draw(if direct { &mut pm } else { &mut target }, &ctx, x)
        {
            if let Some(source) = &mut layer.source {
                source.error = Some(e.to_string());
            }
            return None;
        }
        if complete_source {
            if let (Some(a), Some(base)) = (layer.aggregation, aggregate_symbol) {
                bins.draw(if direct { &mut pm } else { &mut target }, &ctx, a, &base);
            }
            if !direct {
                composite(&mut pm, &target);
            }
        }
        highlights.sort_by_key(|(rank, _, _)| *rank);
        for (_, geometry, symbol) in highlights {
            ctx.highlight(&mut pm, &geometry, &symbol, if layer.graphics { &[] } else { &layer.line_strokes });
            cpu_dirty = true;
        }
    }
    if let Some(g) = &globe {
        globe_shading(&mut pm, g, water, space(view, map), view.pixels_per_point as f32);
    }
    // Tile labels, after every layer's own.
    if lock.is_none() {
        for (i, (l, pri)) in tile_out.labels.into_iter().enumerate() {
            label_candidates.push((l, pri - 1e13, (u32::MAX, i)));
        }
    }
    if let Labels::Tilted(to, out) = &mut labels_to {
        // At their places in the tilted view; those behind the camera, under the sky or too far
        // to read are left out.
        let (tw, th) = (to.width, to.height);
        let sky_y = to.camera.sky_y(th).unwrap_or(0.0) as f32;
        let at = |x: f32, y: f32| -> Option<(f32, f32)> {
            let m = view.camera.to_map(f64::from(x), f64::from(y), view.width, view.height);
            let near = to.camera.depth_scale(m, th).is_some_and(|k| k >= LABEL_MIN_DEPTH_SCALE);
            let (sx, sy) = to.camera.try_to_screen(m, tw, th).filter(|_| near)?;
            (sy as f32 > sky_y).then_some((sx as f32, sy as f32))
        };
        let fixed = fixed_labels.into_iter().map(|l| (l, f64::MAX, (u32::MAX, usize::MAX)));
        for (mut l, pri, key) in fixed.chain(label_candidates) {
            turn_with(&mut l, &at);
            let Some((x, y)) = at(l.x, l.y) else { continue };
            (l.x, l.y) = (x, y);
            if let Some(i) = &mut l.icon {
                let Some((ix, iy)) = at(i.x, i.y) else { continue };
                (i.x, i.y) = (ix, iy);
            }
            out.push((l, pri, key));
        }
        label_candidates = Vec::new();
        fixed_labels = Vec::new();
    }
    let turned = match &labels_to {
        Labels::Own | Labels::Shared => None,
        Labels::Turned(to) | Labels::Tilted(to, _) => Some(*to),
    };
    let (lw, lh) = match turned {
        Some(to) => {
            // Into the turned view's screen, where they stay upright.
            let at = |x: f32, y: f32| {
                let (sx, sy) = to.to_screen(view.camera.to_map(f64::from(x), f64::from(y), view.width, view.height));
                (sx as f32, sy as f32)
            };
            for l in label_candidates.iter_mut().map(|c| &mut c.0).chain(fixed_labels.iter_mut()) {
                turn_with(l, &|x, y| Some(at(x, y)));
                (l.x, l.y) = at(l.x, l.y);
                if let Some(i) = &mut l.icon {
                    (i.x, i.y) = at(i.x, i.y);
                }
            }
            (to.width, to.height)
        }
        None => (view.width, view.height),
    };
    let placed = place_labels(label_candidates, fixed_labels, lw as f32, lh as f32, lock.is_some());
    if display.locked && map.label_lock.is_none() && !matches!(labels_to, Labels::Tilted(..)) {
        map.label_lock = Some(placed.keys);
    }
    let unplaced = if display.show_unplaced { placed.unplaced } else { Vec::new() };
    // The placed labels' icons go into the map itself; their text is drawn over it later. A
    // turned view draws them once it is turned.
    for l in placed.labels.iter().filter(|_| turned.is_none()) {
        if let Some(icon) = &l.icon {
            draw_icon(&mut pm, icon, view.pixels_per_point as f32);
            cpu_dirty = true;
        }
    }
    // Effects, and (on screen only) the colour vision simulation.
    let vision = if full_res_rasters { rustgis_map::view::ColorVision::Normal } else { map.view.color_vision };
    if !map.view.effects.is_none() || vision != rustgis_map::view::ColorVision::Normal {
        let ppp = view.pixels_per_point;
        let lonlat = |x: f64, y: f64| -> Option<Coord> {
            let (sx, sy) = (x / ppp, y / ppp);
            match &globe {
                Some(g) => g.inverse(sx, sy),
                None => flat.inverse(view.camera.to_map(sx, sy, view.width, view.height)),
            }
        };
        post::apply(&mut pm, &map.view.effects, vision, &lonlat);
    }
    let labels = placed.labels;
    let attribution = tile_out.attribution;
    let tiles_pending = tile_out.pending;
    let tile_errors = tile_out.errors;
    match backend {
        Backend::Cpu => {
            Some(RenderOutput::Cpu(Rendered { pixmap: pm, labels, unplaced, attribution, tiles_pending, data_pending, tile_errors }))
        }
        Backend::Gpu | Backend::GpuLive => {
            flush_acc(&mut pm, &mut cmds, &mut cpu_dirty)?;
            // Pathological CPU/GPU alternation (rasters, masks, blends between every vector
            // layer) costs more full-screen uploads than a plain CPU rasterization.
            let images = cmds.iter().filter(|c| matches!(c, gpu_scene::DrawCmd::Image { .. })).count();
            if images > 4 {
                return render_frame(map, view, full_res_rasters, Backend::Cpu, labels_to);
            }
            Some(RenderOutput::Gpu(gpu_scene::GpuScene {
                clear: mesh::premul(water),
                cmds,
                labels,
                unplaced,
                attribution,
                tiles_pending,
                data_pending,
                tile_errors,
                tilt: None,
                clips: Vec::new(),
            }))
        }
    }
}

/// Draws an icon, scaled to its size in logical points.
fn draw_icon(pm: &mut Pixmap, icon: &PlacedIcon, ppp: f32) {
    let (iw, ih) = (icon.image.width().max(1) as f32, icon.image.height().max(1) as f32);
    let (sx, sy) = (icon.w * ppp / iw, icon.h * ppp / ih);
    let (cx, cy) = (icon.x * ppp, icon.y * ppp);
    if !(sx.is_finite() && sy.is_finite() && cx.is_finite() && cy.is_finite()) {
        return;
    }
    // Scaled about its centre, turned there, then put in place.
    let turn = icon.angle.filter(|a| a.is_finite()).map_or(0.0, f32::to_degrees);
    let t = Transform::from_translate(-iw / 2.0, -ih / 2.0).post_scale(sx, sy).post_rotate(turn).post_translate(cx, cy);
    let paint = PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() };
    pm.draw_pixmap(0, 0, Pixmap::as_ref(&icon.image), &paint, t, None);
}

/// Metres on the ground per logical point at the view's centre.
fn map_ground_res(view: &View, flat: &rustgis_map::projection::Flat) -> f64 {
    (view.camera.res * flat.ground_per_unit(view.camera.center)).max(1e-9)
}

pub(crate) fn blend_mode(b: rustgis_map::effects::Blend) -> tiny_skia::BlendMode {
    use rustgis_map::effects::Blend;
    match b {
        Blend::Normal => tiny_skia::BlendMode::SourceOver,
        Blend::Multiply => tiny_skia::BlendMode::Multiply,
        Blend::Screen => tiny_skia::BlendMode::Screen,
        Blend::Overlay => tiny_skia::BlendMode::Overlay,
        Blend::Darken => tiny_skia::BlendMode::Darken,
        Blend::Lighten => tiny_skia::BlendMode::Lighten,
        Blend::Difference => tiny_skia::BlendMode::Difference,
    }
}

/// The mask of every masked or swiped layer: inside its mask layer's polygons and left of the
/// swipe divider.
fn layer_masks(
    map: &mut Map,
    ctx: &Ctx,
    w: u32,
    h: u32,
    flat: &rustgis_map::projection::Flat,
    key: &str,
    scale: f64,
) -> Option<Vec<(u32, tiny_skia::Mask)>> {
    let wanted: Vec<(u32, Option<String>, Option<f64>)> = map
        .layers
        .iter()
        .filter(|l| map.draws_at(l, scale))
        .filter_map(|l| {
            let swipe = map.swipe.filter(|(id, _)| *id == l.id).map(|(_, x)| x.clamp(0.0, 1.0));
            (l.mask.is_some() || swipe.is_some()).then(|| (l.id, l.mask.clone(), swipe))
        })
        .collect();
    let mut out = Vec::new();
    let mut paths = std::collections::HashMap::new();
    for (id, by, swipe) in wanted {
        let Some(mut m) = tiny_skia::Mask::new(w, h) else { continue };
        match by
            .as_deref()
            .and_then(|n| map.layers.iter().position(|l| l.name.eq_ignore_ascii_case(n) && l.geometry_type() == GeometryType::Polygon))
        {
            Some(mi) => {
                if let std::collections::hash_map::Entry::Vacant(entry) = paths.entry(mi) {
                    let mut pb = PathBuilder::new();
                    // The mask's shapes go straight into the path, without copying them.
                    let mut add = |g: &Geometry| {
                        if let Geometry::Polygon(polys) = g {
                            for r in polys.iter().flatten() {
                                ctx.ring_path(&mut pb, r, true);
                            }
                        }
                    };
                    if let Some(source) = map.layers.get(mi).and_then(|l| l.source.as_ref()).and_then(|s| s.chunk_source()) {
                        let result = visit_complete(source.as_ref(), &mut |features| {
                            for feature in features {
                                if let Some(g) = feature.geometry {
                                    let g = if ctx.globe.is_some() { g } else { flat.project(&g) };
                                    add(&g);
                                }
                            }
                            Ok(())
                        });
                        if let Err(e) = result {
                            if let Some(s) = map.layers.get_mut(mi).and_then(|l| l.source.as_mut()) {
                                s.error = Some(e.to_string());
                            }
                            return None;
                        }
                    } else if let Some(ml) = map.layers.get_mut(mi) {
                        if ctx.globe.is_some() {
                            ml.dataset.features.iter().filter_map(|f| f.geometry.as_ref()).for_each(&mut add);
                        } else {
                            ml.projected_in(flat, key).iter().flatten().for_each(&mut add);
                        }
                    }
                    entry.insert(pb.finish());
                }
                if let Some(p) = paths.get(&mi).and_then(Option::as_ref) {
                    m.fill_path(p, FillRule::EvenOdd, true, Transform::identity());
                }
            }
            // No (or no usable) mask layer: everything shows.
            None => {
                if let Some(r) = tiny_skia::Rect::from_xywh(0.0, 0.0, w as f32, h as f32) {
                    m.fill_path(&PathBuilder::from_rect(r), FillRule::Winding, false, Transform::identity());
                }
            }
        }
        if let Some(x) = swipe
            && let Some(r) = tiny_skia::Rect::from_xywh(0.0, 0.0, (w as f64 * x) as f32, h as f32)
        {
            m.intersect_path(&PathBuilder::from_rect(r), FillRule::Winding, false, Transform::identity());
        }
        out.push((id, m));
    }
    Some(out)
}

/// The colour of the space around the globe: the dark sky, or the light space of exports.
pub fn globe_space(dark: bool) -> Rgba {
    if dark { Rgba::rgb(0x08, 0x0B, 0x10) } else { Rgba::rgb(0xEC, 0xEF, 0xF3) }
}

/// How the space around the globe looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Space {
    dark: bool,
    stars: bool,
    /// The glow of an atmosphere around the disc: the Earth's (#223).
    atmosphere: bool,
}

/// The space around the globe for `view`: the interactive view's starry sky, or for exports a
/// light space (dark on the Midnight basemap) without stars.
fn space(view: &View, map: &Map) -> Space {
    let dark = view.sky || map.basemap == rustgis_map::Basemap::Midnight;
    Space { dark, stars: view.sky && map.view.effects.stars, atmosphere: map.body().is_earth() }
}

/// Side of the starfield tile in logical points: the stars repeat this far apart.
pub const STAR_TILE: f32 = 512.0;

/// The stars of the space around the globe: a square `STAR_TILE` points a side at `ppp` pixels
/// per point, transparent between the stars, to be laid edge to edge from the view's top left
/// corner. The sky is the same every time, and the stars stay put while the globe turns.
pub fn starfield(ppp: f32) -> Option<Pixmap> {
    let ppp = if ppp.is_finite() { ppp.clamp(0.25, 4.0) } else { 1.0 };
    let side = (STAR_TILE * ppp).round().max(1.0);
    let mut pm = Pixmap::new(side as u32, side as u32)?;
    let ppp = side / STAR_TILE;
    // A small fixed-seed generator (SplitMix64), so every view gets the same sky.
    let mut seed: u64 = 0x5EED_57A2_F1E1_D000;
    let mut next = || {
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
    };
    // Bluish, white and warm stars; most of them faint.
    let tints = [(0xCF, 0xDD, 0xFF), (0xFF, 0xFF, 0xFF), (0xFF, 0xEA, 0xCC)];
    for _ in 0..180 {
        let (x, y) = (2.0 + next() * (STAR_TILE - 4.0), 2.0 + next() * (STAR_TILE - 4.0));
        let bright = next().powi(3);
        let tint = tints.get((next() * 3.0) as usize).copied().unwrap_or((0xFF, 0xFF, 0xFF));
        let (cx, cy) = (x * ppp, y * ppp);
        let r = (0.3 + 0.8 * bright) * ppp;
        if bright > 0.6
            && let Some(path) = PathBuilder::from_circle(cx, cy, r * 2.5)
        {
            // A faint halo around the brightest.
            let halo = paint(Rgba::rgb(tint.0, tint.1, tint.2), 0.12);
            pm.fill_path(&path, &halo, FillRule::Winding, Transform::identity(), None);
        }
        if let Some(path) = PathBuilder::from_circle(cx, cy, r.max(0.35)) {
            let p = paint(Rgba::rgb(tint.0, tint.1, tint.2), 0.3 + 0.7 * bright);
            pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
        }
    }
    Some(pm)
}

thread_local! {
    /// The starfield tile last drawn, and its pixels per point.
    static STARS: std::cell::RefCell<Option<(u32, Pixmap)>> = const { std::cell::RefCell::new(None) };
}

/// Fills `pm` with the starfield, laid edge to edge from its top left corner.
fn draw_stars(pm: &mut Pixmap, ppp: f32) {
    STARS.with(|cell| {
        let mut cache = cell.borrow_mut();
        if cache.as_ref().is_none_or(|c| c.0 != ppp.to_bits()) {
            *cache = starfield(ppp).map(|t| (ppp.to_bits(), t));
        }
        let Some((_, tile)) = cache.as_ref() else { return };
        let shader =
            tiny_skia::Pattern::new(tile.as_ref(), SpreadMode::Repeat, tiny_skia::FilterQuality::Nearest, 1.0, Transform::identity());
        let p = Paint { shader, ..Paint::default() };
        if let Some(r) = tiny_skia::Rect::from_xywh(0.0, 0.0, pm.width() as f32, pm.height() as f32) {
            pm.fill_rect(r, &p, Transform::identity(), None);
        }
    });
}

/// The globe's backdrop and lighting depend only on the view size, zoom, ocean and stars, not on
/// the rotation, so they are drawn once and reused while the globe turns.
struct GlobeLayers {
    key: (u32, u32, u32, u32, u32, u32, u32, [u8; 4], Space),
    backdrop: Pixmap,
    shading: Pixmap,
}

thread_local! {
    static GLOBE_LAYERS: std::cell::RefCell<Option<GlobeLayers>> = const { std::cell::RefCell::new(None) };
}

fn with_globe_layers(pm: &mut Pixmap, g: &Globe, water: Rgba, space: Space, ppp: f32, f: impl FnOnce(&mut Pixmap, &GlobeLayers)) {
    let (cx, cy) = g.center();
    let key = (
        pm.width(),
        pm.height(),
        ppp.to_bits(),
        (cx as f32).to_bits(),
        (cy as f32).to_bits(),
        (g.disc_radius() as f32).to_bits(),
        (g.disc_height() as f32).to_bits(),
        water.0,
        space,
    );
    GLOBE_LAYERS.with(|cell| {
        let mut cache = cell.borrow_mut();
        if cache.as_ref().is_none_or(|c| c.key != key) {
            let (Some(mut backdrop), Some(mut shading)) = (Pixmap::new(pm.width(), pm.height()), Pixmap::new(pm.width(), pm.height()))
            else {
                return;
            };
            draw_globe_backdrop(&mut backdrop, g, water, space, ppp);
            draw_globe_shading(&mut shading, g, ppp);
            *cache = Some(GlobeLayers { key, backdrop, shading });
        }
        if let Some(c) = cache.as_ref() {
            f(pm, c);
        }
    });
}

fn globe_backdrop(pm: &mut Pixmap, g: &Globe, water: Rgba, space: Space, ppp: f32) {
    with_globe_layers(pm, g, water, space, ppp, |pm, c| pm.data_mut().copy_from_slice(c.backdrop.data()));
}

fn globe_shading(pm: &mut Pixmap, g: &Globe, water: Rgba, space: Space, ppp: f32) {
    with_globe_layers(pm, g, water, space, ppp, |pm, c| {
        pm.draw_pixmap(0, 0, c.shading.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    });
}

/// The globe's surroundings and ocean: space (dark or light, starry when it says so), an
/// atmosphere glow, and the ocean disc.
fn draw_globe_backdrop(pm: &mut Pixmap, g: &Globe, water: Rgba, space: Space, ppp: f32) {
    let bg = globe_space(space.dark);
    pm.fill(tiny_skia::Color::from_rgba8(bg.r(), bg.g(), bg.b(), 255));
    if space.stars {
        draw_stars(pm, ppp);
    }
    let (cx, cy) = g.center();
    let disc = (cx as f32 * ppp, cy as f32 * ppp, g.disc_radius() as f32 * ppp);
    draw_globe_disc(pm, disc, disc_stretch(g), water, space.dark, space.atmosphere);
}

/// How much taller than wide a globe's disc is (1 unless it is tilted).
fn disc_stretch(g: &Globe) -> f32 {
    let k = (g.disc_height() / g.disc_radius().max(1e-9)) as f32;
    if k.is_finite() && k > 0.0 { k } else { 1.0 }
}

/// Stretches drawing up and down by `k` about the row `cy`: a disc into a tilted globe's ellipse.
fn stretch_about(cy: f32, k: f32) -> Transform {
    Transform::from_row(1.0, 0.0, 0.0, k, 0.0, cy * (1.0 - k))
}

/// The atmosphere glow (with `atmosphere`) and the ocean disc, for a disc of radius `r` pixels
/// at (`cx`, `cy`), stretched `k` times up and down (a tilted globe's ellipse).
fn draw_globe_disc(pm: &mut Pixmap, (cx, cy, r): (f32, f32, f32), k: f32, water: Rgba, dark: bool, atmosphere: bool) {
    if !(r.is_finite() && r > 0.5 && cx.is_finite() && cy.is_finite()) {
        return;
    }
    let t = stretch_about(cy, k);
    // Atmosphere: a glow fading out just beyond the limb.
    let glow_r = r * 1.07;
    // A softer, paler glow on light space.
    let glow = if dark { (0x5A, 0x9B, 0xF0, 150) } else { (0x7F, 0xB4, 0xE8, 120) };
    if atmosphere
        && let Some(shader) = RadialGradient::new(
            Point::from_xy(cx, cy),
            0.0,
            Point::from_xy(cx, cy),
            glow_r,
            vec![
                GradientStop::new(0.0, tiny_skia::Color::from_rgba8(glow.0, glow.1, glow.2, glow.3)),
                GradientStop::new(r / glow_r, tiny_skia::Color::from_rgba8(glow.0, glow.1, glow.2, glow.3)),
                GradientStop::new(1.0, tiny_skia::Color::from_rgba8(glow.0, glow.1, glow.2, 0)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        )
        && let Some(path) = PathBuilder::from_circle(cx, cy, glow_r)
    {
        let p = Paint { shader, anti_alias: true, ..Paint::default() };
        pm.fill_path(&path, &p, FillRule::Winding, t, None);
    }
    if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
        pm.fill_path(&path, &paint(water, 1.0), FillRule::Winding, t, None);
    }
}

/// Light from the upper left: a soft highlight and a darker limb, so the globe reads as a sphere.
fn draw_globe_shading(pm: &mut Pixmap, g: &Globe, ppp: f32) {
    let (cx, cy) = g.center();
    draw_globe_light(pm, cx as f32 * ppp, cy as f32 * ppp, g.disc_radius() as f32 * ppp, disc_stretch(g));
}

/// The lighting of a disc of radius `r` pixels at (`cx`, `cy`), stretched `k` times up and down.
fn draw_globe_light(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, k: f32) {
    if !(r.is_finite() && r > 0.5 && cx.is_finite() && cy.is_finite()) {
        return;
    }
    let t = stretch_about(cy, k);
    let light = Point::from_xy(cx - r * 0.35, cy - r * 0.4);
    if let Some(shader) = RadialGradient::new(
        light,
        0.0,
        Point::from_xy(cx, cy),
        r * 1.02,
        vec![
            GradientStop::new(0.0, tiny_skia::Color::from_rgba8(255, 255, 255, 46)),
            GradientStop::new(0.45, tiny_skia::Color::from_rgba8(255, 255, 255, 0)),
            GradientStop::new(0.8, tiny_skia::Color::from_rgba8(0, 0, 0, 18)),
            GradientStop::new(1.0, tiny_skia::Color::from_rgba8(0, 0, 0, 70)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) && let Some(path) = PathBuilder::from_circle(cx, cy, r)
    {
        let p = Paint { shader, anti_alias: true, ..Paint::default() };
        pm.fill_path(&path, &p, FillRule::Winding, t, None);
    }
}

/// How far the atmosphere glow reaches, in disc radii.
pub const GLOBE_GLOW: f32 = 1.07;

/// The globe's surroundings as two square sprites for a disc of radius `r` pixels, centred in
/// them (side `2 × GLOBE_GLOW × r`): the atmosphere (with `atmosphere`) and the `water` disc drawn
/// under the map, and the lighting drawn over it. Neither changes as the globe turns, so a view
/// can scale them while it moves instead of drawing them again.
pub fn globe_sprites(water: Rgba, atmosphere: bool, r: f32) -> Option<(Pixmap, Pixmap)> {
    if !(r.is_finite() && r >= 1.0) {
        return None;
    }
    // A whole number of pixels a side, with the disc exactly in proportion to it.
    let side = (2.0 * GLOBE_GLOW * r.min(2048.0)).round().max(2.0);
    let (r, c) = (side / (2.0 * GLOBE_GLOW), side / 2.0);
    let (mut under, mut over) = (Pixmap::new(side as u32, side as u32)?, Pixmap::new(side as u32, side as u32)?);
    draw_globe_disc(&mut under, (c, c, r), 1.0, water, true, atmosphere);
    draw_globe_light(&mut over, c, c, r, 1.0);
    Some((under, over))
}

/// A part of the map drawn flat, in Web Mercator, to lay over the globe while it turns.
pub struct DrapePatch {
    pub pixmap: Pixmap,
    /// The Web Mercator box the picture shows.
    pub merc: Extent,
}

/// Draws the longitude/latitude `boxes` (from `Globe::drape_extents`) of a globe map flat, at
/// the view's scale, each picture at most `max_side` pixels a side (fewer pixels per point
/// beyond that). The globe view then drapes them over the sphere
/// ([`rustgis_map::Globe::drape_mesh`]) while it turns, with no drawing per frame. Labels are
/// left out (the view hides them while it moves).
pub fn render_globe_drape(map: &mut Map, view: &View, boxes: &[Extent], max_side: u32) -> Vec<DrapePatch> {
    // A coarse drape may have far fewer pixels than points: it shows the map blurrier, but at
    // the view's scale.
    let ppp = if view.pixels_per_point.is_finite() { view.pixels_per_point.clamp(1.0 / 64.0, 4.0) } else { 1.0 };
    let res = view.camera.res;
    if !(res.is_finite() && res > 0.0) {
        return Vec::new();
    }
    let max_side = f64::from(max_side.clamp(64, 8192));
    let projection = std::mem::replace(&mut map.projection, rustgis_map::Projection::WebMercator);
    let (display, lock) = (map.label_display, map.label_lock.take());
    map.label_display.paused = true;
    let mut out = Vec::new();
    for b in boxes {
        let merc = extent_to_merc(b);
        if merc.is_empty() || !(merc.width() > 0.0 && merc.height() > 0.0) {
            continue;
        }
        // Whole points at the view's scale. Fitting the size limit lowers the pixels per point,
        // never the scale: symbols and strokes are sized in points, so a coarser scale would
        // draw them larger on the ground than the settled frame does.
        let (w, h) = ((merc.width() / res).floor().max(1.0), (merc.height() / res).floor().max(1.0));
        let ppp = ppp.min(max_side / w.max(h));
        if w * ppp < 2.0 || h * ppp < 2.0 {
            continue;
        }
        let camera = Camera { center: merc.center(), res, bearing: 0.0, pitch: 0.0 };
        let v = View { width: w, height: h, pixels_per_point: ppp, camera, sky: false };
        // The picture is given the box asked for, though rounding to whole points made it show
        // up to a point less: stretched by under a point, its edges stay exactly where asked, so
        // one at the Web Mercator limit is still stretched on to the pole (drape_mesh).
        if let Some(r) = render(map, &v) {
            out.push(DrapePatch { pixmap: r.pixmap, merc });
        }
    }
    map.projection = projection;
    map.label_display = display;
    map.label_lock = lock;
    out
}

/// A band of a tilted view drawn flat, to lay over the tilted ground while the view moves.
pub struct TiltPatch {
    pub pixmap: Pixmap,
    /// The flat, north-up camera and size (logical points) it was drawn for.
    pub band: rustgis_map::camera::Band,
}

/// Draws the bands of a tilted view ([`Camera::tilt_bands`]) flat, each grown by `margin` (a
/// fraction of its size) on every side so the view can move a little before they no longer
/// cover it, and at most `max_side` pixels a side (coarser beyond that). The map view lays them
/// over the tilted ground as meshes while it moves, with no drawing per frame. Labels are left
/// out (the view keeps the last frame's while it moves).
pub fn render_tilt_drape(map: &mut Map, view: &View, margin: f64, max_side: u32) -> Vec<TiltPatch> {
    let ppp = if view.pixels_per_point.is_finite() { view.pixels_per_point.clamp(0.25, 4.0) } else { 1.0 };
    let margin = if margin.is_finite() { margin.clamp(0.0, 2.0) } else { 0.0 };
    let max_side = f64::from(max_side.clamp(64, 8192));
    let bands = view.camera.tilt_bands(view.width, view.height);
    sync_tilted(map, view, &bands, false);
    let (display, lock) = (map.label_display, map.label_lock.take());
    map.label_display.paused = true;
    let mut out = Vec::new();
    for b in bands {
        let (mut w, mut h) = (b.width * (1.0 + 2.0 * margin), b.height * (1.0 + 2.0 * margin));
        let mut camera = b.camera;
        // Coarser to fit the size limit.
        let k = (w.max(h) * ppp / max_side).max(1.0);
        camera.res *= k;
        (w, h) = ((w / k).floor().max(1.0), (h / k).floor().max(1.0));
        let v = View { width: w, height: h, pixels_per_point: ppp, camera, sky: false };
        // The bands share the read above.
        if let Some(RenderOutput::Cpu(r)) = render_frame(map, &v, false, Backend::Cpu, Labels::Shared) {
            out.push(TiltPatch { pixmap: r.pixmap, band: rustgis_map::camera::Band { camera, width: w, height: h, depth: b.depth } });
        }
    }
    map.label_display = display;
    map.label_lock = lock;
    out
}

/// Turns a label turned with the map (and its icon) as `at` moves it into another view (a
/// turned or tilted one): its angle there is that of a short step along it, the text kept
/// upright. Called before the label itself is moved.
fn turn_with(l: &mut PlacedLabel, at: &impl Fn(f32, f32) -> Option<(f32, f32)>) {
    let turned = |x: f32, y: f32, a: f32| -> Option<f32> {
        let ((x0, y0), (x1, y1)) = (at(x, y)?, at(x + 4.0 * a.cos(), y + 4.0 * a.sin())?);
        let b = (y1 - y0).atan2(x1 - x0);
        b.is_finite().then_some(b)
    };
    if let Some(a) = l.angle {
        let x = l.center_x();
        l.angle = Some(turned(x, l.y, a).map_or(a, rustgis_tiles::symbols::upright));
    }
    if let Some(i) = &mut l.icon
        && let Some(a) = i.angle
    {
        i.angle = Some(turned(i.x, i.y, a).unwrap_or(a));
    }
}

/// A halo that contrasts with the text colour.
fn halo_for(text: Rgba) -> Rgba {
    let lum = 0.299 * text.r() as f32 + 0.587 * text.g() as f32 + 0.114 * text.b() as f32;
    if lum < 140.0 { Rgba::new(255, 255, 255, 225) } else { Rgba::new(20, 22, 26, 210) }
}

struct Placement {
    labels: Vec<PlacedLabel>,
    unplaced: Vec<PlacedLabel>,
    /// (layer id, row) of each placed label.
    keys: Vec<(u32, usize)>,
}

/// Greedy collision-free placement, highest priority first. Locked labels are all placed (they
/// were placed without collisions when the lock was taken).
///
/// As MapLibre places its symbols: a label's text and its icon each have a box (a turned label's
/// text a chain of them along it) kept `collide`'s padding clear of the others; a label is placed
/// when they fit, or the part that is optional is dropped when only the other fits, and a part
/// allowed to overlap is shown whatever is under it. A part ignoring placement is shown without
/// keeping the others off it. Labels reaching past the view's edge are placed (only those wholly
/// outside it are left out), and the boxes are kept in a grid so a crowded view stays quick.
fn place_labels(mut cands: Vec<Candidate>, fixed: Vec<PlacedLabel>, w: f32, h: f32, locked: bool) -> Placement {
    // A priority of no number places last, not first (`total_cmp` puts NaN above everything).
    let rank = |p: f64| if p.is_nan() { f64::NEG_INFINITY } else { p };
    cands.sort_by(|a, b| rank(b.1).total_cmp(&rank(a.1)));
    let mut grid = LabelGrid::new(w, h);
    for l in &fixed {
        let (text, icon) = label_boxes(l);
        text.iter().chain(&icon).for_each(|b| grid.insert(*b));
    }
    let mut out = Placement { labels: fixed, unplaced: Vec::new(), keys: Vec::new() };
    let outside = |b: &[f32; 4]| b[2] < 0.0 || b[3] < 0.0 || b[0] > w || b[1] > h;
    for (l, _, key) in cands.into_iter().take(20_000) {
        let (text, icon) = label_boxes(&l);
        if text.iter().chain(&icon).all(outside) {
            continue;
        }
        let c = l.collide;
        let (has_text, has_icon) = (!text.is_empty(), !icon.is_empty());
        let fits = |boxes: &[[f32; 4]], overlap: bool| locked || overlap || !boxes.iter().any(|b| grid.hits(b));
        let mut show_text = has_text && fits(&text, c.text_allow_overlap);
        let mut show_icon = has_icon && fits(&icon, c.icon_allow_overlap);
        // Which part may show without the other (MapLibre's rules).
        let icon_alone = c.text_optional || !has_text;
        let text_alone = c.icon_optional || !has_icon;
        if !icon_alone && !text_alone {
            let both = show_text && show_icon;
            (show_text, show_icon) = (both, both);
        } else if !text_alone {
            show_text = show_text && show_icon;
        } else if !icon_alone {
            show_icon = show_icon && show_text;
        }
        if !show_text && !show_icon {
            if out.unplaced.len() < 2000 && has_text {
                out.unplaced.push(PlacedLabel { color: UNPLACED, halo: Some(halo_for(UNPLACED)), icon: None, ..l });
            }
            continue;
        }
        if show_text && !c.text_ignore_placement {
            text.iter().for_each(|b| grid.insert(*b));
        }
        if show_icon && !c.icon_ignore_placement {
            icon.iter().for_each(|b| grid.insert(*b));
        }
        let mut l = l;
        if !show_text {
            l.text.clear();
        }
        if !show_icon {
            l.icon = None;
        }
        out.labels.push(l);
        out.keys.push(key);
    }
    out
}

/// A label's collision boxes, [left, top, right, bottom] in points, padded: its text's (one box,
/// or a chain of squares along a turned label) and its icon's.
fn label_boxes(l: &PlacedLabel) -> (Vec<[f32; 4]>, Vec<[f32; 4]>) {
    let mut text = Vec::new();
    if !l.text.is_empty() {
        let tw = text::label_width(l);
        let th = text::block_height(&l.text, l.size);
        let p = l.collide.text_padding.clamp(0.0, 100.0);
        let cx = l.center_x();
        match l.angle.filter(|a| a.is_finite() && a.abs() > 1e-3) {
            Some(a) => {
                // Squares as tall as the text, end to end along it.
                // (A width of no number is one square, so the label still collides.)
                let n = (tw / th.max(1.0)).ceil();
                let n = if n.is_finite() { n.clamp(1.0, 200.0) } else { 1.0 };
                let step = tw / n;
                let r = step.max(th) / 2.0 + p;
                let (dx, dy) = (a.cos(), a.sin());
                for i in 0..n as usize {
                    let t = (i as f32 + 0.5) * step - tw / 2.0;
                    let (x, y) = (cx + dx * t, l.y + dy * t);
                    text.push([x - r, y - r, x + r, y + r]);
                }
            }
            None => text.push([cx - tw / 2.0 - p, l.y - th / 2.0 - p, cx + tw / 2.0 + p, l.y + th / 2.0 + p]),
        }
    }
    let icon = l
        .icon
        .iter()
        .map(|i| {
            let p = l.collide.icon_padding.clamp(0.0, 100.0);
            // A turned icon: the box round it as turned.
            let (c, s) = i.angle.map_or((1.0, 0.0), |a| (a.cos().abs(), a.sin().abs()));
            let (hw, hh) = ((i.w * c + i.h * s) / 2.0, (i.w * s + i.h * c) / 2.0);
            [i.x - hw - p, i.y - hh - p, i.x + hw + p, i.y + hh + p]
        })
        .collect();
    (text, icon)
}

/// Placed labels' boxes in a grid of cells over the view (and a margin round it), so a box is
/// tested against those near it only.
struct LabelGrid {
    boxes: Vec<[f32; 4]>,
    cells: Vec<Vec<u32>>,
    cols: usize,
    rows: usize,
    /// The grid's top-left corner and the side of a cell, in points.
    x0: f32,
    y0: f32,
    cell: f32,
}

impl LabelGrid {
    const MARGIN: f32 = 512.0;

    fn new(w: f32, h: f32) -> LabelGrid {
        let (w, h) = (if w.is_finite() { w.clamp(1.0, 1e5) } else { 1.0 }, if h.is_finite() { h.clamp(1.0, 1e5) } else { 1.0 });
        let cell = ((w + 2.0 * Self::MARGIN).max(h + 2.0 * Self::MARGIN) / 128.0).max(32.0);
        let cols = ((w + 2.0 * Self::MARGIN) / cell).ceil() as usize + 1;
        let rows = ((h + 2.0 * Self::MARGIN) / cell).ceil() as usize + 1;
        LabelGrid { boxes: Vec::new(), cells: vec![Vec::new(); cols * rows], cols, rows, x0: -Self::MARGIN, y0: -Self::MARGIN, cell }
    }

    /// The cells a box falls in (those past the grid's edge clamped to it): columns, rows.
    fn span(&self, b: &[f32; 4]) -> Option<(std::ops::RangeInclusive<usize>, std::ops::RangeInclusive<usize>)> {
        if !b.iter().all(|v| v.is_finite()) {
            return None;
        }
        let at = |v: f32, o: f32, n: usize| (((v - o) / self.cell).floor().max(0.0) as usize).min(n.saturating_sub(1));
        Some((at(b[0], self.x0, self.cols)..=at(b[2], self.x0, self.cols), at(b[1], self.y0, self.rows)..=at(b[3], self.y0, self.rows)))
    }

    fn insert(&mut self, b: [f32; 4]) {
        let Some((xs, ys)) = self.span(&b) else { return };
        let i = self.boxes.len() as u32;
        self.boxes.push(b);
        for y in ys {
            for x in xs.clone() {
                if let Some(c) = self.cells.get_mut(y * self.cols + x) {
                    c.push(i);
                }
            }
        }
    }

    /// Whether a box overlaps one already placed.
    fn hits(&self, b: &[f32; 4]) -> bool {
        let Some((xs, ys)) = self.span(b) else { return true };
        ys.into_iter().any(|y| {
            xs.clone().any(|x| {
                self.cells.get(y * self.cols + x).is_some_and(|c| {
                    c.iter().any(|&i| self.boxes.get(i as usize).is_some_and(|o| b[0] < o[2] && o[0] < b[2] && b[1] < o[3] && o[1] < b[3]))
                })
            })
        })
    }
}

/// Renders at full resolution once the map's tiles have arrived: each pass fetches what it is
/// missing (a style, then its sources, then their tiles) and waits for it, at most a minute in
/// all. In the browser, which can't wait, it draws what has arrived.
pub(crate) fn render_complete(map: &mut Map, view: &View) -> Option<Rendered> {
    let mut r = render_with(map, view, true)?;
    let start = web_time::Instant::now();
    let budget = web_time::Duration::from_secs(60);
    let mut passes = 0;
    while r.tiles_pending && passes < 6 && start.elapsed() < budget && cfg!(not(target_arch = "wasm32")) {
        // Each wait within what is left of the minute.
        let left = budget.saturating_sub(start.elapsed());
        rustgis_tiles::cache::wait(left.min(web_time::Duration::from_secs(30)));
        r = render_with(map, view, true)?;
        passes += 1;
    }
    Some(r)
}

/// Renders a map with its labels, a scale bar and a north arrow to PNG bytes.
pub fn render_png(map: &mut Map, view: &View) -> Result<Vec<u8>, String> {
    let mut r = render_complete(map, view).ok_or("could not allocate the image")?;
    for l in r.unplaced.iter().chain(&r.labels) {
        text::draw(&mut r.pixmap, l, view.pixels_per_point as f32);
    }
    text::scale_bar(&mut r.pixmap, view, map.ground_res_at(&view.camera));
    draw_attribution(&mut r.pixmap, &r.attribution, view.pixels_per_point as f32);
    r.pixmap.encode_png().map_err(|e| e.to_string())
}

/// The tile layers' credits, small, in the bottom-right corner.
pub(crate) fn draw_attribution(pm: &mut Pixmap, attribution: &[String], ppp: f32) {
    if attribution.is_empty() {
        return;
    }
    let text = attribution.join(" | ");
    let size = 9.0;
    let (w, h) = (pm.width() as f32 / ppp, pm.height() as f32 / ppp);
    let tw = text::measure(&text, size, false).min(w);
    let label = PlacedLabel {
        text,
        x: w - tw / 2.0 - 4.0,
        y: h - size * 0.9,
        size,
        color: Rgba::rgb(0x33, 0x33, 0x33),
        halo: Some(Rgba::rgb(0xFF, 0xFF, 0xFF)),
        bold: false,
        icon: None,
        align: 0.0,
        framed: false,
        italic: false,
        spacing: 0.0,
        line_align: TextAlign::Left,
        angle: None,
        collide: Default::default(),
    };
    text::draw(pm, &label, ppp);
}

#[cfg(test)]
mod tests {

    fn label(text: &str, x: f32, icon: Option<PlacedIcon>, align: f32) -> PlacedLabel {
        PlacedLabel {
            text: text.into(),
            x,
            y: 20.0,
            size: 10.0,
            color: Rgba::rgb(0, 0, 0),
            halo: None,
            bold: false,
            icon,
            align,
            framed: false,
            italic: false,
            spacing: 0.0,
            line_align: TextAlign::Left,
            angle: None,
            collide: Default::default(),
        }
    }

    #[test]
    fn places_icons_with_their_labels() {
        let icon = |x: f32| {
            let mut p = Pixmap::new(4, 4).unwrap();
            p.fill(tiny_skia::Color::from_rgba8(0, 0, 255, 255));
            PlacedIcon { image: std::sync::Arc::new(p), x, y: 20.0, w: 10.0, h: 10.0, angle: None }
        };
        // Two icons on the same spot collide even with no text; one far away doesn't.
        let cands = vec![
            (label("", 50.0, Some(icon(50.0)), 0.0), 2.0, (u32::MAX, 0)),
            (label("", 52.0, Some(icon(52.0)), 0.0), 1.0, (u32::MAX, 1)),
            (label("", 150.0, Some(icon(150.0)), 0.0), 0.0, (u32::MAX, 2)),
        ];
        let placed = place_labels(cands, Vec::new(), 200.0, 40.0, false);
        assert_eq!(placed.labels.iter().map(|l| l.x).collect::<Vec<_>>(), vec![50.0, 150.0]);
        // Left-aligned text starts at x.
        let l = label("Station", 100.0, None, -0.5);
        assert!((l.center_x() - (100.0 + text::measure("Station", 10.0, false) / 2.0)).abs() < 1e-3);
        // An icon is drawn into the map at its size.
        let mut pm = Pixmap::new(200, 40).unwrap();
        draw_icon(&mut pm, &icon(50.0), 1.0);
        let px = pm.pixel(50, 20).unwrap();
        assert_eq!((px.blue(), px.alpha()), (255, 255));
        assert_eq!(pm.pixel(60, 20).unwrap().alpha(), 0);
    }

    fn icon_at(x: f32, y: f32, w: f32, h: f32, angle: Option<f32>) -> PlacedIcon {
        let mut p = Pixmap::new(4, 4).unwrap();
        p.fill(tiny_skia::Color::from_rgba8(0, 0, 255, 255));
        PlacedIcon { image: std::sync::Arc::new(p), x, y, w, h, angle }
    }

    fn texts(p: &Placement) -> Vec<&str> {
        p.labels.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn labels_keep_their_padding_above_and_below() {
        // Two labels a point apart vertically: their padding (2 on every side) keeps them apart.
        let a = PlacedLabel { y: 20.0, ..label("Upper", 100.0, None, 0.0) };
        let gap = text::block_height("Upper", 10.0) + 1.0;
        let b = PlacedLabel { y: 20.0 + gap, ..label("Lower", 100.0, None, 0.0) };
        let placed = place_labels(vec![(a.clone(), 2.0, (u32::MAX, 0)), (b.clone(), 1.0, (u32::MAX, 1))], Vec::new(), 200.0, 80.0, false);
        assert_eq!(texts(&placed), ["Upper"]);
        // Without padding they fit.
        let none = rustgis_tiles::Collide { text_padding: 0.0, ..Default::default() };
        let (a, b) = (PlacedLabel { collide: none, ..a }, PlacedLabel { collide: none, ..b });
        let placed = place_labels(vec![(a, 2.0, (u32::MAX, 0)), (b, 1.0, (u32::MAX, 1))], Vec::new(), 200.0, 80.0, false);
        assert_eq!(texts(&placed), ["Upper", "Lower"]);
    }

    #[test]
    fn optional_text_and_icons_drop_alone() {
        use rustgis_tiles::Collide;
        // A word in the way of a station's name, not of its icon (to the left of it).
        let blocker = || (label("Blocker", 100.0, None, 0.0), 9.0, (u32::MAX, 0));
        let station =
            |collide: Collide| PlacedLabel { collide, ..label("Station", 100.0, Some(icon_at(40.0, 20.0, 10.0, 10.0, None)), 0.0) };
        let run = |collide: Collide| place_labels(vec![blocker(), (station(collide), 1.0, (u32::MAX, 1))], Vec::new(), 200.0, 40.0, false);
        // Neither optional: the station isn't shown at all.
        let p = run(Collide::default());
        assert_eq!(p.labels.len(), 1);
        // Its text optional: the icon shows on its own.
        let p = run(Collide { text_optional: true, ..Default::default() });
        assert_eq!(p.labels.len(), 2);
        assert!(p.labels[1].text.is_empty() && p.labels[1].icon.is_some());
        // Its icon optional, and the icon in the way instead: the text shows on its own.
        let icon_blocked = vec![
            (label("", 40.0, Some(icon_at(40.0, 20.0, 10.0, 10.0, None)), 0.0), 9.0, (u32::MAX, 0)),
            (station(Collide { icon_optional: true, ..Default::default() }), 1.0, (u32::MAX, 1)),
        ];
        let p = place_labels(icon_blocked, Vec::new(), 200.0, 40.0, false);
        assert_eq!(p.labels.len(), 2);
        assert!(p.labels[1].text == "Station" && p.labels[1].icon.is_none());
        // Allowed to overlap, the text shows over the blocker; ignoring placement, a later label
        // goes over it.
        let p = run(Collide { text_allow_overlap: true, ..Default::default() });
        assert_eq!(p.labels.len(), 2);
        assert!(p.labels[1].text == "Station" && p.labels[1].icon.is_some());
        let ghost =
            PlacedLabel { collide: Collide { text_ignore_placement: true, ..Default::default() }, ..label("Ghost", 100.0, None, 0.0) };
        let p = place_labels(
            vec![(ghost, 9.0, (u32::MAX, 0)), (label("Real", 100.0, None, 0.0), 1.0, (u32::MAX, 1))],
            Vec::new(),
            200.0,
            40.0,
            false,
        );
        assert_eq!(texts(&p), ["Ghost", "Real"]);
    }

    #[test]
    fn labels_reaching_past_the_edge_are_placed() {
        let half_out = label("Edge of the map", 0.0, None, 0.0);
        let gone = label("Gone", -500.0, None, 0.0);
        let p = place_labels(vec![(half_out, 1.0, (u32::MAX, 0)), (gone, 0.0, (u32::MAX, 1))], Vec::new(), 200.0, 40.0, false);
        assert_eq!(texts(&p), ["Edge of the map"]);
        assert!(p.unplaced.is_empty(), "a label wholly off the view isn't unplaced either");
    }

    #[test]
    fn turned_labels_collide_along_their_text() {
        // A long label at 45° and a short one in the corner of its bounding box, clear of the text.
        let long = "A long street name along the line";
        let w = text::measure(long, 10.0, false);
        let turned = PlacedLabel { x: 100.0, y: 100.0, angle: Some(std::f32::consts::FRAC_PI_4), ..label(long, 100.0, None, 0.0) };
        let corner = PlacedLabel { x: 100.0 + w * 0.3, y: 100.0 - w * 0.3, ..label("Hi", 0.0, None, 0.0) };
        let across = PlacedLabel { x: 100.0 + w * 0.3, y: 100.0 + w * 0.3, ..label("Hi", 0.0, None, 0.0) };
        let p = place_labels(
            vec![(turned, 3.0, (u32::MAX, 0)), (corner, 2.0, (u32::MAX, 1)), (across, 1.0, (u32::MAX, 2))],
            Vec::new(),
            400.0,
            400.0,
            false,
        );
        assert_eq!(p.labels.len(), 2, "the corner one fits, the one on the text doesn't");
        assert!(p.labels[1].y < 100.0);
    }

    #[test]
    fn places_many_labels_quickly() {
        // A grid of 20,000 candidates, a third of them fitting: placed without testing each
        // against every other.
        let cands: Vec<Candidate> = (0..20_000)
            .map(|i| {
                let (x, y) = ((i % 200) as f32 * 10.0, (i / 200) as f32 * 10.0);
                (PlacedLabel { x, y, ..label("ab", x, None, 0.0) }, -f64::from(i), (u32::MAX, i as usize))
            })
            .collect();
        let start = web_time::Instant::now();
        let p = place_labels(cands, Vec::new(), 2000.0, 1000.0, false);
        assert!(!p.labels.is_empty() && p.labels.len() < 20_000);
        assert!(start.elapsed() < web_time::Duration::from_secs(5));
    }

    #[test]
    fn tile_labels_are_sized_and_wrapped_as_maplibre() {
        let l = rustgis_tiles::Label {
            text: "West 42nd Street & Avenue of the Americas\nOuest".into(),
            x: 100.0,
            y: 100.0,
            size: 12.0,
            color: [0, 0, 0, 255],
            halo: None,
            bold: false,
            italic: true,
            letter_spacing: 0.1,
            max_width: 9.0,
            justify: 0.0,
            priority: 7.5,
            anchor: [0.0, 0.0],
            offset: [0.0, 0.0],
            angle: None,
            icon: None,
            collide: Default::default(),
        };
        let (p, pri) = tiles::tile_label(l.clone()).unwrap();
        // text-size 12 is a 12-pixel font.
        assert!((p.size * 1.33 - 12.0).abs() < 1e-3);
        assert_eq!(pri, 7.5);
        assert!(p.italic && (p.spacing - 0.1 * p.size).abs() < 1e-4 && p.line_align == TextAlign::Center);
        // Wrapped to about 9 ems (108 pixels), the line break of its own kept.
        let lines: Vec<&str> = text::lines(&p.text).collect();
        assert!(lines.len() >= 3, "{lines:?}");
        assert_eq!(lines.last(), Some(&"Ouest"));
        for line in &lines {
            assert!(text::measure(line, p.size, false) < 108.0 * 1.4, "{line} is too long");
        }
        // Along a line, never wrapped; turned with it.
        let along =
            rustgis_tiles::Label { max_width: 0.0, angle: Some(0.5), text: "West 42nd Street & Avenue of the Americas".into(), ..l };
        let (p, _) = tiles::tile_label(along).unwrap();
        assert_eq!((text::line_count(&p.text), p.angle), (1, Some(0.5)));
    }

    #[test]
    fn wraps_balanced_at_spaces_and_dashes() {
        let width = |s: &str| text::measure(s, 9.0, false);
        let w = width("Times Square–42nd Street");
        let lines = text::wrap("Times Square–42nd Street", 9.0, false, 0.0, w * 0.6);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines.join(" ").replace("– ", "–") == "Times Square–42nd Street", "{lines:?}");
        // Balanced: neither line much longer than the other.
        let (a, b) = (width(&lines[0]), width(&lines[1]));
        assert!(a.max(b) / a.min(b) < 2.0, "{lines:?}");
        // A space after an "&" is kept.
        let lines = text::wrap("West 42nd Street & 8th Avenue", 9.0, false, 0.0, width("West 42nd Street"));
        assert!(lines.iter().all(|l| !l.contains("&8")), "{lines:?}");
        // What fits, or has nowhere to break, is left alone; junk is safe.
        assert_eq!(text::wrap("Short", 9.0, false, 0.0, 1000.0), ["Short"]);
        assert_eq!(text::wrap("Unbreakable", 9.0, false, 0.0, 1.0), ["Unbreakable"]);
        assert!(!text::wrap("a b c", f32::NAN, false, f32::INFINITY, -1.0).is_empty());
    }

    #[test]
    fn turned_text_and_icons_are_drawn_turned() {
        // Ink from a long label at 90° runs down the pixmap, not across it.
        let ink = |pm: &Pixmap| {
            let (mut x0, mut x1, mut y0, mut y1) = (u32::MAX, 0, u32::MAX, 0);
            for y in 0..pm.height() {
                for x in 0..pm.width() {
                    if pm.pixel(x, y).is_some_and(|p| p.alpha() > 0) {
                        (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
                    }
                }
            }
            (x1.saturating_sub(x0), y1.saturating_sub(y0))
        };
        let l = PlacedLabel { x: 100.0, y: 100.0, size: 12.0, ..label("Broadway Broadway", 100.0, None, 0.0) };
        let mut flat = Pixmap::new(200, 200).unwrap();
        text::draw(&mut flat, &l, 1.0);
        let mut turned = Pixmap::new(200, 200).unwrap();
        text::draw(&mut turned, &PlacedLabel { angle: Some(std::f32::consts::FRAC_PI_2), ..l }, 1.0);
        let ((fw, fh), (tw, th)) = (ink(&flat), ink(&turned));
        if fw > 0 {
            assert!(fw > fh && th > tw, "flat {fw}×{fh}, turned {tw}×{th}");
            assert!(th.abs_diff(fw) <= 3, "the same length either way");
        }
        // An arrow 20 long and 4 wide, turned to point down.
        let mut pm = Pixmap::new(40, 40).unwrap();
        draw_icon(&mut pm, &icon_at(20.0, 20.0, 20.0, 4.0, Some(std::f32::consts::FRAC_PI_2)), 1.0);
        assert!(pm.pixel(20, 28).unwrap().alpha() > 0 && pm.pixel(28, 20).unwrap().alpha() == 0);
    }

    #[test]
    fn labels_turn_with_a_turned_view() {
        // A view turned by 30°: a label along a line turns with it, kept upright; one upright on
        // the screen stays so.
        let a = 30f32.to_radians();
        let at = |x: f32, y: f32| Some((x * a.cos() - y * a.sin(), x * a.sin() + y * a.cos()));
        let mut along =
            PlacedLabel { angle: Some(0.0), icon: Some(icon_at(0.0, 0.0, 4.0, 4.0, Some(3.0))), ..label("Road", 50.0, None, 0.0) };
        turn_with(&mut along, &at);
        assert!(along.angle.is_some_and(|b| (b - a).abs() < 1e-3));
        assert!(
            along
                .icon
                .as_ref()
                .and_then(|i| i.angle)
                .is_some_and(
                    |b| (b - 3.0 - a).rem_euclid(std::f32::consts::TAU).min((3.0 + a - b).rem_euclid(std::f32::consts::TAU)) < 1e-3
                )
        );
        let mut flipped = PlacedLabel { angle: Some(80f32.to_radians()), ..label("Road", 50.0, None, 0.0) };
        turn_with(&mut flipped, &at);
        assert!(flipped.angle.is_some_and(|b| (b - (110f32 - 180.0).to_radians()).abs() < 1e-3), "{:?}", flipped.angle);
        let mut upright = label("Town", 50.0, None, 0.0);
        turn_with(&mut upright, &at);
        assert_eq!(upright.angle, None);
    }

    use super::*;
    use rustgis_data::{Field, FieldType, Value};

    #[test]
    fn worldview_detection_and_data() {
        use sample_data::Worldview::{China, Default};
        let detect = |tz: Option<&str>, locales: &[&str]| sample_data::Worldview::detect(tz, locales.iter().copied());
        assert_eq!(detect(Some("Asia/Shanghai"), &["en-US"]), China);
        assert_eq!(detect(Some("Asia/Urumqi"), &[]), China);
        assert_eq!(detect(Some("Europe/Berlin"), &["zh-CN", "en"]), China);
        assert_eq!(detect(None, &["zh_CN.UTF-8"]), China);
        assert_eq!(detect(None, &["zh-Hans-CN"]), China);
        assert_eq!(detect(Some("Asia/Taipei"), &["zh-TW"]), Default);
        assert_eq!(detect(Some("Asia/Hong_Kong"), &["zh-HK"]), Default);
        assert_eq!(detect(Some("America/New_York"), &["en-US", "zh-CN"]), Default);
        assert_eq!(detect(None, &[]), Default);
        assert_eq!(sample_data::Worldview::from_code(" China "), Some(China));
        assert_eq!(sample_data::Worldview::from_code("default"), Some(Default));
        assert_eq!(sample_data::Worldview::from_code("auto"), None);
        // Both files have the same fields; in China's view Taiwan is part of China.
        let read = |text| rustgis_data::io::read_geojson(text, "Countries").unwrap().0;
        let (def, chn) = (read(sample_data::COUNTRIES), read(sample_data::COUNTRIES_CHN));
        let fields = |d: &Dataset| d.fields.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
        assert_eq!(fields(&def), fields(&chn));
        let names = |d: &Dataset| {
            let i = d.fields.iter().position(|f| f.name == "name").unwrap();
            d.features
                .iter()
                .filter_map(|f| if let Some(Value::Text(t)) = f.attrs.get(i) { Some(t.clone()) } else { None })
                .collect::<Vec<_>>()
        };
        assert!(names(&def).iter().any(|n| n == "Taiwan"));
        assert!(!names(&chn).iter().any(|n| n == "Taiwan"));
        assert!(names(&chn).iter().any(|n| n == "China"));
    }

    #[test]
    fn the_globe_drape_is_the_flat_map_of_its_box() {
        let mut m = Map::new("Map");
        let mut d = Dataset::new("pts", GeometryType::Point, vec![Field::new("name", FieldType::Text)]);
        d.push(Some(Geometry::Point(Coord::new(2.35, 48.85))), vec![Value::Text("Paris".into())]);
        m.add_dataset(d);
        m.projection = rustgis_map::Projection::Globe;
        let paris = lonlat_to_merc(Coord::new(2.35, 48.85));
        let camera = Camera { center: paris, res: 2_000.0, bearing: 0.0, pitch: 0.0 };
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera, sky: false };
        let boxes = Globe::new(&camera, 400.0, 300.0).drape_extents(400.0, 300.0, 0.25);
        let patches = render_globe_drape(&mut m, &view, &boxes, 4096);
        assert_eq!(patches.len(), 1);
        // The map is left as it was.
        assert!(m.projection.is_globe() && m.label_lock.is_none() && !m.label_display.paused);
        let p = &patches[0];
        assert!(p.merc.contains(paris) && p.merc.contains(lonlat_to_merc(boxes[0].center())));
        // Paris's marker is where the box puts it, as the flat map draws it.
        let px = |pm: &Pixmap, x: f64, y: f64| pm.pixel(x as u32, y as u32).map(|c| (c.red(), c.green(), c.blue()));
        let res = p.merc.width() / f64::from(p.pixmap.width());
        let at = px(&p.pixmap, (paris.x - p.merc.min_x) / res, (p.merc.max_y - paris.y) / res);
        m.projection = rustgis_map::Projection::WebMercator;
        let flat = render(&mut m, &View { camera: Camera { center: paris, res, bearing: 0.0, pitch: 0.0 }, ..view }).unwrap();
        assert_eq!(at, px(&flat.pixmap, 200.0, 150.0));
        // Too big a box is drawn coarser, within the limit; junk draws nothing.
        let big = render_globe_drape(&mut m, &view, &[Extent::new(-180.0, -80.0, 180.0, 80.0)], 512);
        assert!(big.iter().all(|p| p.pixmap.width() <= 512 && p.pixmap.height() <= 512));
        // A box at the Web Mercator limit stays exactly there however coarse the picture, so its
        // mesh still reaches the pole.
        let lim = rustgis_geom::MAX_LAT;
        // (30 km points leave a remainder: the picture is 1335 points tall, 0.8 of a point short.)
        let coarse = View { camera: Camera { center: paris, res: 30_000.0, bearing: 0.0, pitch: 0.0 }, ..view };
        let world = render_globe_drape(&mut m, &coarse, &[Extent::new(-180.0, -lim, 180.0, lim)], 4096);
        let edge = std::f64::consts::PI * rustgis_geom::MERCATOR_R * 0.9999;
        assert!(
            world.iter().all(|p| p.merc.max_y >= edge && p.merc.min_y <= -edge),
            "{:?}",
            world.iter().map(|p| p.merc).collect::<Vec<_>>()
        );
        assert!(
            render_globe_drape(
                &mut m,
                &View { camera: Camera { center: paris, res: f64::NAN, bearing: 0.0, pitch: 0.0 }, ..view },
                &boxes,
                4096
            )
            .is_empty()
        );
        assert!(render_globe_drape(&mut m, &view, &[Extent::EMPTY], 4096).is_empty());
        // The sprites: a square around the disc, or nothing for a junk radius.
        let (under, over) = globe_sprites(rustgis_map::Basemap::Paper.palette().0, true, 100.0).unwrap();
        assert_eq!((under.width(), over.height()), (214, 214));
        assert_eq!(under.pixel(107, 107).map(|c| c.alpha()), Some(255));
        // Off the Earth no atmosphere glows past the limb (#223).
        assert!(under.pixel(107 + 103, 107).is_some_and(|c| c.alpha() > 0));
        let (airless, _) = globe_sprites(rustgis_map::Basemap::Paper.palette().0, false, 100.0).unwrap();
        assert_eq!(airless.pixel(107 + 103, 107).map(|c| c.alpha()), Some(0));
        assert_eq!(airless.pixel(107, 107).map(|c| c.alpha()), Some(255));
        assert!(globe_sprites(rustgis_map::Basemap::Paper.palette().0, true, f32::NAN).is_none());
    }

    #[test]
    fn the_starfield_is_the_same_sky_every_time() {
        let a = starfield(2.0).unwrap();
        assert_eq!((a.width(), a.height()), (1024, 1024));
        assert_eq!(a.data(), starfield(2.0).unwrap().data());
        // Stars on a transparent sky.
        let lit = a.pixels().iter().filter(|p| p.alpha() > 0).count();
        assert!(lit > 100 && lit < a.pixels().len() / 20, "{lit}");
        // Junk scales still give a tile.
        assert!(starfield(f32::NAN).is_some_and(|t| t.width() == 512));
        assert!(starfield(1e9).is_some_and(|t| t.width() == 2048));
    }

    /// Each feature is labelled by the first label class whose clause takes it, Default last; a
    /// hidden class keeps its features unlabelled, and a clause that no longer reads takes none
    /// (#618).
    #[test]
    fn label_classes_label_their_features() {
        let mut m = Map::new("Map");
        let fields = vec![Field::new("name", FieldType::Text), Field::new("capital", FieldType::Integer)];
        let mut d = Dataset::new("pts", GeometryType::Point, fields);
        d.push(Some(Geometry::Point(Coord::new(2.35, 48.85))), vec![Value::Text("Paris".into()), Value::Int(1)]);
        d.push(Some(Geometry::Point(Coord::new(4.83, 45.76))), vec![Value::Text("Lyon".into()), Value::Int(0)]);
        let id = m.add_dataset(d);
        let capitals = |l: &mut rustgis_map::Layer, w: &str| {
            let base = l.labels.default.clone();
            l.labels.classes = vec![rustgis_map::LabelClass { name: "Capitals".into(), where_: Some(w.into()), size: 20.0, ..base }];
        };
        if let Some(l) = m.layer_mut(id) {
            l.labels.enabled = true;
            capitals(l, "capital = 1");
        }
        let camera = Camera { center: lonlat_to_merc(Coord::new(3.6, 47.3)), res: 5000.0, bearing: 0.0, pitch: 0.0 };
        let view = View { width: 800.0, height: 600.0, pixels_per_point: 1.0, camera, sky: false };
        let sizes = |m: &mut Map| {
            let mut v: Vec<(String, f32)> = render(m, &view).unwrap().labels.into_iter().map(|l| (l.text, l.size)).collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v
        };
        let default = m.layer(id).unwrap().labels.default.size;
        assert_eq!(sizes(&mut m), [("Lyon".to_string(), default), ("Paris".to_string(), 20.0)], "Paris is a capital, Lyon the rest");
        // Hidden, the class still has Paris: it goes unlabelled, not to Default.
        m.layer_mut(id).unwrap().labels.classes[0].visible = false;
        assert_eq!(sizes(&mut m), [("Lyon".to_string(), default)]);
        // A clause that doesn't read takes no feature: both go to Default.
        capitals(m.layer_mut(id).unwrap(), "gone = 1");
        assert_eq!(sizes(&mut m), [("Lyon".to_string(), default), ("Paris".to_string(), default)]);
    }

    #[test]
    fn renders_world_and_layers() {
        let mut m = Map::new("Map");
        let mut d = Dataset::new("pts", GeometryType::Point, vec![Field::new("name", FieldType::Text)]);
        d.push(Some(Geometry::Point(Coord::new(2.35, 48.85))), vec![Value::Text("Paris".into())]);
        let id = m.add_dataset(d);
        if let Some(l) = m.layer_mut(id) {
            l.labels.enabled = true;
            l.selection = vec![0];
        }
        let camera = Camera { center: lonlat_to_merc(Coord::new(2.35, 48.85)), res: 5000.0, bearing: 0.0, pitch: 0.0 };
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera, sky: false };
        let r = render(&mut m, &view).unwrap();
        assert_eq!(r.labels.len(), 1);
        assert_eq!(r.labels[0].text, "Paris");
        let png = render_png(&mut m, &view).unwrap();
        assert!(png.len() > 1000);
        assert!(!world().is_empty());
    }

    #[test]
    fn a_selected_text_graphic_is_framed_with_its_label() {
        let mut m = Map::new("Map");
        let id = m.add_graphics("Graphics");
        let at = Coord::new(2.35, 48.85);
        if let Some(l) = m.layer_mut(id) {
            l.dataset.push(Some(Geometry::Point(at)), vec![Value::Text("text".into()), Value::Text("Selected text".into())]);
        }
        let camera = Camera { center: lonlat_to_merc(at), res: 5000.0, bearing: 0.0, pitch: 0.0 };
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera, sky: false };
        let framed = |m: &mut Map| render(m, &view).unwrap().labels.iter().find(|l| l.text == "Selected text").map(|l| l.framed);
        assert_eq!(framed(&mut m), Some(false));
        if let Some(l) = m.layer_mut(id) {
            l.selection = vec![0];
        }
        assert_eq!(framed(&mut m), Some(true));
        // Drawn into an image (an export), the frame goes around the text in the selection colour.
        let r = render(&mut m, &view).unwrap();
        let mut pm = Pixmap::new(400, 300).unwrap();
        for l in &r.labels {
            text::draw(&mut pm, l, 1.0);
        }
        let edge = (200.0 - text::measure("Selected text", 14.0, false) / 2.0 - 3.0).round() as u32;
        let cyan = |x: u32, y: u32| pm.pixel(x, y).is_some_and(|c| c.alpha() > 200 && c.red() < 60 && c.green() > 180 && c.blue() > 200);
        assert!((0..300).any(|y| (edge - 1..=edge + 1).any(|x| cyan(x, y))), "no frame at x = {edge}");
    }

    /// A text of several lines is drawn in an image (an export) on as many lines, as the map
    /// view shows it, not run together on one (#577); its box and frame hold every line.
    #[test]
    fn a_text_of_several_lines_is_drawn_on_as_many() {
        let at = |text: &str, framed: bool| PlacedLabel { y: 150.0, size: 20.0, framed, ..label(text, 200.0, None, 0.0) };
        // The rows and columns with ink.
        let ink = |l: &PlacedLabel| {
            let mut pm = Pixmap::new(400, 300).unwrap();
            text::draw(&mut pm, l, 1.0);
            let dark = |x: u32, y: u32| pm.pixel(x, y).is_some_and(|c| c.alpha() > 128 && c.red() < 60);
            let rows: Vec<u32> = (0..300).filter(|y| (0..400).any(|x| dark(x, *y))).collect();
            let cols: Vec<u32> = (0..400).filter(|x| (0..300).any(|y| dark(*x, y))).collect();
            (rows, cols)
        };
        let (rows, cols) = ink(&at("Atlantic\nOcean", false));
        let (one_rows, one_cols) = ink(&at("AtlanticOcean", false));
        // Two bands of ink with a gap between, each the height of one line.
        let bands = rows.windows(2).filter(|w| w[1] > w[0] + 1).count() + 1;
        assert_eq!(bands, 2, "rows with ink: {rows:?}");
        let span = |v: &[u32]| v.last().copied().unwrap_or(0).saturating_sub(v.first().copied().unwrap_or(0));
        assert!(span(&rows) > span(&one_rows) * 3 / 2, "taller than one line");
        assert!(span(&cols) < span(&one_cols) * 3 / 4, "narrower than the lines run together");
        // Lines start at the block's left, the block centred on the point.
        let (_, alone) = ink(&at("Atlantic", false));
        assert!(cols.first().zip(alone.first()).is_some_and(|(a, b)| a.abs_diff(*b) <= 2), "the longer line keeps its place");
        // As wide as the widest line, as tall as its lines; a trailing newline is an empty line.
        assert_eq!(text::block_width("ab\ncdef", 10.0, false), text::measure("cdef", 10.0, false));
        // Text drawn on one line (a table cell, a chart's name) is still measured whole, so it is
        // cut to its column as it is drawn.
        assert_eq!(text::measure("ab\ncdef", 10.0, false), text::measure("abcdef", 10.0, false));
        assert_eq!((text::line_count("a"), text::line_count("a\nb"), text::line_count("a\n")), (1, 2, 2));
        assert_eq!(text::line_count(&"\n".repeat(10_000)), 200, "lines are bounded");
        // The collision box spaces its lines as they are drawn, a line pitch apart, not one
        // line's height (1.25 × size) each: a label whose top is just past where two such
        // heights would end, but still over the drawn second line, is left out.
        assert!((text::block_height("a\nb\nc", 20.0) - text::block_height("a", 20.0) - 2.0 * text::line_pitch(20.0)).abs() < 1e-3);
        let one_line = 20.0 * 1.25;
        let (old_bottom, gap) = (150.0 + one_line, 2.0);
        let below = PlacedLabel { y: old_bottom + gap + one_line / 2.0, ..at("Below", false) };
        let p = place_labels(vec![(below, 0.0, (1, 0))], vec![at("Atlantic\nOcean", false)], 400.0, 300.0, false);
        assert!(p.labels.iter().all(|l| l.text != "Below"), "placed over the second line");
        // The selection frame goes round both lines.
        let mut pm = Pixmap::new(400, 300).unwrap();
        text::draw(&mut pm, &at("Atlantic\nOcean", true), 1.0);
        let cyan = |x: u32, y: u32| pm.pixel(x, y).is_some_and(|c| c.alpha() > 200 && c.red() < 60 && c.green() > 180 && c.blue() > 200);
        let framed: Vec<u32> = (0..300).filter(|y| (0..400).any(|x| cyan(x, *y))).collect();
        assert!(framed.first().zip(rows.first()).is_some_and(|(f, r)| f < r), "frame above the first line");
        assert!(framed.last().zip(rows.last()).is_some_and(|(f, r)| f > r), "frame below the last line");
    }

    /// A text graphic's lines are aligned in their block, slanted and letter-spaced as it says
    /// (#535), in an image as on screen; plain labels are drawn as before.
    #[test]
    fn a_text_graphic_is_drawn_aligned_slanted_and_spaced() {
        let at = |text: &str| PlacedLabel { y: 150.0, size: 20.0, ..label(text, 200.0, None, 0.0) };
        // Each band of ink (a line) as (top, bottom, left, right), top to bottom.
        let bands = |l: &PlacedLabel| {
            let mut pm = Pixmap::new(400, 300).unwrap();
            text::draw(&mut pm, l, 1.0);
            let dark = |x: u32, y: u32| pm.pixel(x, y).is_some_and(|c| c.alpha() > 128 && c.red() < 60);
            let rows: Vec<u32> = (0..300).filter(|y| (0..400).any(|x| dark(x, *y))).collect();
            let mut out: Vec<(u32, u32, u32, u32)> = Vec::new();
            for y in rows {
                let cols: Vec<u32> = (0..400).filter(|x| dark(*x, y)).collect();
                let (lo, hi) = (cols.first().copied().unwrap_or(0), cols.last().copied().unwrap_or(0));
                match out.last_mut() {
                    Some(b) if y == b.1 + 1 => *b = (b.0, y, b.2.min(lo), b.3.max(hi)),
                    _ => out.push((y, y, lo, hi)),
                }
            }
            out
        };
        // Lines of clearly different widths, so where the short one sits shows its alignment.
        let two = "North Atlantic\nOcean";
        // Left, as labels have always been: both lines start at the block's left.
        let left = bands(&at(two));
        assert_eq!(left.len(), 2);
        assert!(left[0].2.abs_diff(left[1].2) <= 2, "left: {left:?}");
        // Centred: the short line sits in the middle of the long one; right: their right edges meet.
        let centred = bands(&PlacedLabel { line_align: TextAlign::Center, ..at(two) });
        let (a, b) = (centred[0], centred[1]);
        let gap = |x: u32, y: u32| i64::from(x) - i64::from(y);
        assert!((gap(b.2, a.2) - gap(a.3, b.3)).abs() <= 3, "centred: {centred:?}");
        assert!(b.2 > a.2 + 20, "the short line moved in: {centred:?}");
        let right = bands(&PlacedLabel { line_align: TextAlign::Right, ..at(two) });
        assert!(right[0].3.abs_diff(right[1].3) <= 2, "right: {right:?}");
        // The block stays centred on its point whichever way its lines are aligned.
        for b in [&left, &centred, &right] {
            let (l, r) = (b.iter().map(|x| x.2).min().unwrap_or(0), b.iter().map(|x| x.3).max().unwrap_or(0));
            assert!(((l + r) as f32 / 2.0 - 200.0).abs() <= 3.0, "block centred at {}", (l + r) / 2);
        }
        // Letter-spaced: wider by the spacing between each letter, still centred; measured so.
        let plain = bands(&at("Spaced"))[0];
        let spaced = PlacedLabel { spacing: 6.0, ..at("Spaced") };
        let wide = bands(&spaced)[0];
        let grew = (gap(wide.3, wide.2) - gap(plain.3, plain.2)) as f32;
        assert!((grew - 5.0 * 6.0 * 1.33).abs() <= 3.0, "grew {grew}");
        // Placed as the map view lays it out: its advance and the spacing between its letters (5
        // gaps in "Spaced"), not its ink; a slant doesn't widen it either.
        let added = text::label_width(&spaced) - text::label_width(&at("Spaced"));
        assert!((added - 5.0 * 6.0 * 1.33).abs() < 0.01, "added {added}");
        assert_eq!(text::label_width(&PlacedLabel { italic: true, ..at("Slant") }), text::label_width(&at("Slant")));
        // Spaced tighter than its letters are wide, it is still no less than nothing.
        assert!(text::label_width(&PlacedLabel { spacing: -5.0, size: 4.0, ..at("iiiiii") }) >= 0.0);
        // Slanted: an upright stem leans right at the top.
        let stem = |italic: bool| {
            let mut pm = Pixmap::new(400, 300).unwrap();
            text::draw(&mut pm, &PlacedLabel { italic, size: 60.0, ..at("l") }, 1.0);
            let dark = |x: u32, y: u32| pm.pixel(x, y).is_some_and(|c| c.alpha() > 128);
            let rows: Vec<u32> = (0..300).filter(|y| (0..400).any(|x| dark(x, *y))).collect();
            let mid = |y: u32| {
                let c: Vec<u32> = (0..400).filter(|x| dark(*x, y)).collect();
                (c.first().copied().unwrap_or(0) + c.last().copied().unwrap_or(0)) as i64 / 2
            };
            mid(rows.first().copied().unwrap_or(0) + 2) - mid(rows.last().copied().unwrap_or(0).saturating_sub(2))
        };
        assert!(stem(false).abs() <= 1 && stem(true) >= 6, "lean: upright {}, italic {}", stem(false), stem(true));
        // A text graphic's fields reach the label; a plain one's are neutral.
        let mut m = Map::new("Map");
        let id = m.add_graphics("Graphics");
        let p = Coord::new(2.35, 48.85);
        if let Some(l) = m.layer_mut(id) {
            let field = |name: &str| l.dataset.field_index(name).unwrap();
            let (fi, fa, fs) = (field("italic"), field("align"), field("letter_spacing"));
            let mut attrs = vec![Value::Null; l.dataset.fields.len()];
            attrs[0] = Value::Text("text".into());
            attrs[1] = Value::Text("Styled".into());
            (attrs[fi], attrs[fa], attrs[fs]) = (Value::Bool(true), Value::Text("right".into()), Value::Float(4.0));
            l.dataset.push(Some(Geometry::Point(p)), attrs);
            l.dataset.push(Some(Geometry::Point(Coord::new(2.45, 48.80))), vec![Value::Text("text".into()), Value::Text("Plain".into())]);
        }
        let camera = Camera { center: lonlat_to_merc(p), res: 5000.0, bearing: 0.0, pitch: 0.0 };
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera, sky: false };
        let r = render(&mut m, &view).unwrap();
        let get = |t: &str| r.labels.iter().find(|l| l.text == t).map(|l| (l.italic, l.line_align, l.spacing));
        assert_eq!(get("Styled"), Some((true, TextAlign::Right, 4.0)));
        assert_eq!(get("Plain"), Some((false, TextAlign::Left, 0.0)));
    }

    #[test]
    fn degenerate_views_do_not_panic() {
        let mut m = Map::new("Map");
        for (w, h) in [(0.0, 0.0), (1e9, 1e9), (f64::NAN, 3.0)] {
            let view = View { width: w, height: h, pixels_per_point: 1.0, camera: Camera::world(), sky: false };
            let _ = render(&mut m, &view);
        }
    }

    /// The world sample the golden pixmap test renders.
    fn sample_world() -> Map {
        let mut m = Map::new("Map");
        for (name, text) in [
            ("Countries", sample_data::COUNTRIES),
            ("Rivers", sample_data::RIVERS),
            ("Lakes", sample_data::LAKES),
            ("Cities", sample_data::CITIES),
        ] {
            if let Some(d) = world_dataset(name, text) {
                m.add_dataset(d);
            }
        }
        m
    }

    fn fnv1a(data: &[u8]) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for b in data {
            h = (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }

    /// Golden hash of the sample world render: a byte-lock proving the GPU-backend refactor
    /// (Backend/RenderOutput plumbing) never changed the CPU path's output. tiny-skia is
    /// deterministic IEEE f32 with no platform fast-math, so the hash should hold everywhere;
    /// if a platform or a tiny-skia bump ever disagrees (CI runs Linux/macOS/Windows), convert
    /// this to a handful of sampled pixels rather than deleting the lock.
    #[test]
    fn world_atlas_pixmap_is_byte_stable() {
        let mut m = sample_world();
        let view = View { width: 800.0, height: 500.0, pixels_per_point: 1.0, camera: Camera::world(), sky: false };
        let r = render(&mut m, &view).expect("render");
        assert_eq!(r.pixmap.data().len(), 800 * 500 * 4);
        let h = fnv1a(r.pixmap.data());
        // Point markers are stamped from sprites at quarter-pixel offsets (the cities' anti-aliased
        // edges moved by up to an eighth of a pixel); nothing else changed this frame.
        assert_eq!(h, 0x89e2_f6ec_5589_a46e, "golden pixmap hash changed: {h:#x}");
    }

    fn square_layer(m: &mut Map, name: &str, x: f64, y: f64, s: f64) -> u32 {
        let mut d = Dataset::new(name, GeometryType::Polygon, vec![]);
        d.push(
            Some(Geometry::Polygon(vec![vec![vec![
                Coord::new(x, y),
                Coord::new(x + s, y),
                Coord::new(x + s, y + s),
                Coord::new(x, y + s),
            ]]])),
            vec![],
        );
        m.add_dataset(d)
    }

    fn cmd_kinds(scene: &gpu_scene::GpuScene) -> Vec<&'static str> {
        scene
            .cmds
            .iter()
            .map(|c| match c {
                gpu_scene::DrawCmd::Mesh { .. } => "mesh",
                gpu_scene::DrawCmd::Points { .. } => "points",
                gpu_scene::DrawCmd::Extrusion { .. } => "extrusion",
                gpu_scene::DrawCmd::Image { .. } => "image",
                gpu_scene::DrawCmd::Tiles { .. } => "tiles",
                gpu_scene::DrawCmd::LiveTiles { .. } => "live tiles",
                gpu_scene::DrawCmd::TileMeshes { .. } => "tile meshes",
            })
            .collect()
    }

    #[test]
    fn gpu_backend_builds_scene_for_flat_world() {
        let mut m = sample_world();
        if let Some(l) = m.layers.iter_mut().find(|l| l.name == "Cities") {
            l.labels.enabled = true;
        }
        let view = View { width: 800.0, height: 500.0, pixels_per_point: 1.0, camera: Camera::world(), sky: false };
        let Some(RenderOutput::Gpu(scene)) = render_backend(&mut m, &view, Backend::Gpu) else { panic!("gpu scene") };
        let kinds = cmd_kinds(&scene);
        assert!(kinds.iter().filter(|k| **k == "mesh").count() >= 2, "countries/lakes/rivers as meshes: {kinds:?}");
        assert!(kinds.contains(&"points"), "cities as marker instances: {kinds:?}");
        // The label channel is identical to the CPU render's.
        let mut m2 = sample_world();
        if let Some(l) = m2.layers.iter_mut().find(|l| l.name == "Cities") {
            l.labels.enabled = true;
        }
        let cpu = render(&mut m2, &view).expect("cpu");
        assert_eq!(scene.labels.len(), cpu.labels.len());
        assert_eq!(scene.labels.first().map(|l| l.text.as_str()), cpu.labels.first().map(|l| l.text.as_str()));
    }

    #[test]
    fn gpu_backend_interleaves_cpu_image_for_blended_layer() {
        let mut m = Map::new("Map");
        // Layers draw bottom-up: the last one in the list first.
        square_layer(&mut m, "a", 0.0, 0.0, 10.0);
        let b = square_layer(&mut m, "b", 20.0, 0.0, 10.0);
        square_layer(&mut m, "c", 40.0, 0.0, 10.0);
        if let Some(l) = m.layer_mut(b) {
            l.blend = rustgis_map::effects::Blend::Multiply;
        }
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera: Camera::world(), sky: false };
        let Some(RenderOutput::Gpu(scene)) = render_backend(&mut m, &view, Backend::Gpu) else { panic!("gpu scene") };
        let kinds = cmd_kinds(&scene);
        let last_image = kinds.iter().rposition(|k| *k == "image").expect("the blended layer's cpu image");
        assert!(kinds[..last_image].contains(&"mesh"), "a mesh below the blended layer: {kinds:?}");
        assert!(kinds[last_image + 1..].contains(&"mesh"), "a mesh above the blended layer: {kinds:?}");
    }

    #[test]
    fn gpu_backend_skips_the_spurious_image_for_hidden_tile_layers() {
        let mut m = sample_world();
        m.tiles.push(rustgis_map::TileLayer::new("hidden", rustgis_map::TileKind::Raster, "https://example.invalid/{z}/{x}/{y}.png"));
        for t in &mut m.tiles {
            t.visible = false;
        }
        let view = View { width: 800.0, height: 500.0, pixels_per_point: 1.0, camera: Camera::world(), sky: false };
        let Some(RenderOutput::Gpu(scene)) = render_backend(&mut m, &view, Backend::Gpu) else { panic!("gpu scene") };
        assert!(!scene.cmds.iter().any(|c| matches!(c, gpu_scene::DrawCmd::Image { .. })), "hidden tiles must not emit an empty image");
    }

    #[test]
    fn gpu_backend_caps_image_commands_to_a_cpu_frame() {
        // More than four alternating CPU-only layers costs more full-screen uploads than a
        // plain CPU rasterization: the whole frame goes to the CPU backend.
        let build = |blends: usize| {
            let mut m = Map::new("Map");
            for i in 0..(blends * 2 + 1) {
                let id = square_layer(&mut m, &format!("l{i}"), i as f64 * 20.0, 0.0, 10.0);
                if i % 2 == 1
                    && let Some(l) = m.layer_mut(id)
                {
                    l.transparency = 50.0; // offscreen-composited → an image command each
                }
            }
            m
        };
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera: Camera::world(), sky: false };
        let mut three = build(3);
        let Some(RenderOutput::Gpu(scene)) = render_backend(&mut three, &view, Backend::Gpu) else { panic!("3 blends stays gpu") };
        assert_eq!(scene.cmds.iter().filter(|c| matches!(c, gpu_scene::DrawCmd::Image { .. })).count(), 3);
        let mut five = build(5);
        assert!(
            matches!(render_backend(&mut five, &view, Backend::Gpu), Some(RenderOutput::Cpu(_))),
            "5 blends falls back to the cpu frame"
        );
    }

    #[test]
    fn gpu_backend_appends_selection_highlight() {
        let mut m = Map::new("Map");
        let id = square_layer(&mut m, "s", 0.0, 0.0, 10.0);
        if let Some(l) = m.layer_mut(id) {
            l.selection = vec![0];
        }
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera: Camera::world(), sky: false };
        let Some(RenderOutput::Gpu(scene)) = render_backend(&mut m, &view, Backend::Gpu) else { panic!("gpu scene") };
        let kinds = cmd_kinds(&scene);
        assert_eq!(kinds.last(), Some(&"mesh"), "the highlight mesh follows the layer: {kinds:?}");
        assert!(kinds.iter().filter(|k| **k == "mesh").count() >= 2, "layer mesh + highlight mesh: {kinds:?}");
    }

    #[test]
    fn gpu_scene_carries_backdrop_as_clear_color_and_no_image_for_pure_vectors() {
        let mut m = sample_world();
        let view = View { width: 800.0, height: 500.0, pixels_per_point: 1.0, camera: Camera::world(), sky: false };
        let Some(RenderOutput::Gpu(scene)) = render_backend(&mut m, &view, Backend::Gpu) else { panic!("gpu scene") };
        assert!(
            !scene.cmds.iter().any(|c| matches!(c, gpu_scene::DrawCmd::Image { .. })),
            "a pure-vector scene must carry no CPU image (it would poison the camera fast path): {} cmds",
            scene.cmds.len()
        );
        let (water, ..) = m.basemap.palette();
        assert_eq!(scene.clear, mesh::premul(water), "the clear colour is the basemap's water");
    }

    #[test]
    fn a_coarse_globe_drape_keeps_point_symbols_their_size() {
        // A drape over its size limit gets fewer pixels, not a coarser scale: a marker sized in
        // points covers as much ground as in a full-density drape, so it doesn't grow on the
        // globe while it moves.
        let mut m = Map::new("Map");
        m.projection = rustgis_map::Projection::Globe;
        let mut d = Dataset::new("pts", GeometryType::Point, vec![]);
        d.push(Some(Geometry::Point(Coord::new(0.0, 0.0))), vec![]);
        let id = m.add_dataset(d);
        if let Some(l) = m.layer_mut(id) {
            l.renderer = rustgis_map::Renderer::Single(rustgis_map::Symbol {
                fill: Rgba::rgb(0, 255, 0),
                stroke: Rgba::rgb(0, 255, 0),
                stroke_width: 0.0,
                size: 20.0,
                marker: rustgis_map::Marker::Circle,
            });
        }
        let camera = Camera { center: Coord::new(0.0, 0.0), res: 20_000.0, bearing: 0.0, pitch: 0.0 };
        let view = View { width: 800.0, height: 600.0, pixels_per_point: 1.0, camera, sky: true };
        let boxes = [Extent::new(-60.0, -60.0, 60.0, 60.0)];
        // The marker's width in metres along the equator.
        let mut width = |max_side: u32| {
            let p = render_globe_drape(&mut m, &view, &boxes, max_side);
            let p = p.first().expect("a picture");
            let row = p.pixmap.height() / 2;
            let green = (0..p.pixmap.width()).filter(|&x| p.pixmap.pixel(x, row).is_some_and(|c| c.green() > 200 && c.red() < 100)).count();
            (green as f64 * p.merc.width() / f64::from(p.pixmap.width()), p.merc.width() / f64::from(p.pixmap.width()))
        };
        let (sharp, _) = width(4096);
        let (coarse, px) = width(128);
        assert!(px > 4.0 * 20_000.0, "the size limit should coarsen the picture ({px} m per pixel)");
        assert!(sharp > 0.0 && (coarse - sharp).abs() <= 2.0 * px, "a coarse drape drew the marker {coarse} m wide, not {sharp} m");
        assert_eq!(m.projection, rustgis_map::Projection::Globe);
    }

    #[test]
    fn gpu_backend_skips_non_finite_stroke_widths() {
        // A hostile symbol state (NaN stroke width) must not put NaN vertices on the GPU:
        // the line's stroke is skipped, the rest of the scene is unaffected.
        let mut m = Map::new("Map");
        let mut d = Dataset::new("l", GeometryType::Polyline, vec![]);
        d.push(Some(Geometry::Line(vec![vec![Coord::new(-50.0, 0.0), Coord::new(50.0, 0.0)]])), vec![]);
        let id = m.add_dataset(d);
        if let Some(l) = m.layer_mut(id) {
            l.renderer = rustgis_map::Renderer::Single(rustgis_map::Symbol {
                fill: rustgis_map::Rgba::rgb(0, 0, 200),
                stroke: rustgis_map::Rgba::rgb(0, 0, 200),
                stroke_width: f32::NAN,
                size: 0.0,
                marker: rustgis_map::Marker::Circle,
            });
        }
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera: Camera::world(), sky: false };
        let Some(RenderOutput::Gpu(scene)) = render_backend(&mut m, &view, Backend::Gpu) else { panic!("gpu scene") };
        for c in &scene.cmds {
            if let gpu_scene::DrawCmd::Mesh { vertices, .. } = c {
                assert!(vertices.iter().all(|v| v.position[0].is_finite() && v.position[1].is_finite() && v.stroke_px.is_finite()));
            }
        }
    }

    #[test]
    fn gpu_backend_handles_non_finite_geometry_without_panicking() {
        // A polygon with a NaN vertex (hostile file): the GPU drops the point (lyon's path
        // builder panics on NaN — never feed it), the healthy layer still tessellates, and
        // the frame completes like the CPU's.
        let build = || {
            let mut m = Map::new("Map");
            let mut d = Dataset::new("bad", GeometryType::Polygon, vec![]);
            d.push(
                Some(Geometry::Polygon(vec![vec![vec![
                    Coord::new(0.0, 0.0),
                    Coord::new(10.0, f64::NAN),
                    Coord::new(10.0, 10.0),
                    Coord::new(0.0, 10.0),
                ]]])),
                vec![],
            );
            m.add_dataset(d);
            square_layer(&mut m, "good", 30.0, 0.0, 10.0);
            m
        };
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera: Camera::world(), sky: false };
        let mut gm = build();
        let Some(RenderOutput::Gpu(scene)) = render_backend(&mut gm, &view, Backend::Gpu) else { panic!("gpu scene") };
        assert!(cmd_kinds(&scene).contains(&"mesh"), "the healthy layer tessellated");
        let mut cm = build();
        assert!(render(&mut cm, &view).is_some(), "cpu frame completes too");
    }

    #[test]
    fn gpu_backend_falls_back_for_globe_and_effects() {
        let mut m = sample_world();
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera: Camera::world(), sky: false };
        m.projection = rustgis_map::Projection::Globe;
        assert!(matches!(render_backend(&mut m, &view, Backend::Gpu), Some(RenderOutput::Cpu(_))), "globe stays on the CPU renderer");
        m.projection = rustgis_map::Projection::WebMercator;
        m.view.effects.vignette = true;
        assert!(matches!(render_backend(&mut m, &view, Backend::Gpu), Some(RenderOutput::Cpu(_))), "view effects stay on the CPU renderer");
    }

    #[test]
    fn deep_zoom_on_the_coast_does_not_panic() {
        // Zoomed in to millimetres on a coastline, the land polygon's far vertices were billions
        // of pixels away, and the rasteriser indexed past its buffer.
        let mut m = Map::new("Map");
        for res in [0.05, 0.008, 0.001, 1e-5] {
            let camera = Camera { center: lonlat_to_merc(Coord::new(-122.2585, 37.8719)), res, bearing: 0.0, pitch: 0.0 };
            let view = View { width: 1600.0, height: 1000.0, pixels_per_point: 1.0, camera, sky: false };
            assert!(render(&mut m, &view).is_some(), "{res}");
        }
    }

    #[test]
    fn clips_rings_and_lines_to_a_box() {
        let b = Extent { min_x: 0.0, min_y: 0.0, max_x: 10.0, max_y: 10.0 };
        // A square far bigger than the box clips to the box.
        let big = [(-1e9, -1e9), (1e9, -1e9), (1e9, 1e9), (-1e9, 1e9)];
        let c = clip_ring(&big, &b);
        assert_eq!(c.len(), 4);
        assert!(c.iter().all(|p| (p.0 == 0.0 || p.0 == 10.0) && (p.1 == 0.0 || p.1 == 10.0)), "{c:?}");
        // One wholly outside clips away; one inside is kept as it is.
        assert!(clip_ring(&[(20.0, 20.0), (30.0, 20.0), (30.0, 30.0)], &b).is_empty());
        let inside = [(1.0, 1.0), (5.0, 1.0), (5.0, 5.0)];
        assert_eq!(clip_ring(&inside, &b), inside.to_vec());
        // A line in and out of the box comes back as its two parts inside.
        let parts = clip_line(&[(-5.0, 2.0), (5.0, 2.0), (5.0, 20.0), (8.0, 20.0), (8.0, 5.0), (15.0, 5.0)], &b);
        assert_eq!(parts, vec![vec![(0.0, 2.0), (5.0, 2.0), (5.0, 10.0)], vec![(8.0, 10.0), (8.0, 5.0), (10.0, 5.0)]]);
        assert!(clip_line(&[(-5.0, -5.0), (-1.0, 20.0)], &b).is_empty());
    }

    #[test]
    fn stamped_markers_match_drawn_ones() {
        let view = View {
            width: 120.0,
            height: 80.0,
            pixels_per_point: 1.0,
            camera: Camera { center: Coord::new(0.0, 0.0), res: 1.0, bearing: 0.0, pitch: 0.0 },
            sky: false,
        };
        let ctx = Ctx { view: &view, ppp: 1.0, wireframe: false, cull: Extent::EMPTY, globe: None, sprites: Default::default() };
        for marker in Marker::ALL {
            let s = Symbol { fill: Rgba::new(200, 60, 30, 200), stroke: Rgba::rgb(20, 20, 20), stroke_width: 1.2, size: 9.0, marker };
            let (mut stamped, mut drawn) = (Pixmap::new(120, 80).unwrap(), Pixmap::new(120, 80).unwrap());
            // Points on and off the quarter-pixel grid, and over the image's edge.
            for c in
                [Coord::new(-30.0, 10.0), Coord::new(5.37, -12.81), Coord::new(25.25, 3.5), Coord::new(-61.0, 0.0), Coord::new(40.0, 41.0)]
            {
                ctx.stamp(&mut stamped, c, &s);
                ctx.stamp(&mut stamped, c, &s); // the second comes from the cache
                ctx.marker(&mut drawn, c, &s, 1.0, None);
                ctx.marker(&mut drawn, c, &s, 1.0, None);
            }
            let worst = stamped.data().iter().zip(drawn.data()).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
            assert!(worst < 120, "{marker:?}: {worst}");
            let total: u64 = stamped.data().iter().zip(drawn.data()).map(|(a, b)| a.abs_diff(*b) as u64).sum();
            let ink: u64 = drawn.data().iter().map(|v| *v as u64).sum();
            assert!(total * 20 < ink, "{marker:?}: {total} of {ink}");
        }
        assert!(ctx.sprites.borrow().len() <= 5 * 16);
    }

    #[test]
    fn a_turned_view_turns_the_map_and_keeps_labels_upright() {
        let mut m = Map::new("Map");
        m.basemap = rustgis_map::Basemap::Blank;
        let mut d = Dataset::new("pts", GeometryType::Point, vec![Field::new("name", FieldType::Text)]);
        // 80 px east of the centre.
        d.push(Some(Geometry::Point(Coord::new(80.0 * 1000.0 / 111_319.49, 0.0))), vec![Value::Text("East".into())]);
        let id = m.add_dataset(d);
        if let Some(l) = m.layer_mut(id) {
            l.labels.enabled = true;
        }
        // Where the point is drawn: the middle of the pixels unlike the background.
        let ink = |pm: &Pixmap| {
            let bg = pm.pixel(0, 0).unwrap();
            let (mut n, mut sx, mut sy) = (0.0, 0.0, 0.0);
            for (k, p) in pm.pixels().iter().enumerate() {
                if *p != bg {
                    n += 1.0;
                    sx += (k as u32 % pm.width()) as f64;
                    sy += (k as u32 / pm.width()) as f64;
                }
            }
            assert!(n > 0.0, "nothing drawn");
            (sx / n, sy / n)
        };
        let camera = Camera { center: Coord::new(0.0, 0.0), res: 1000.0, bearing: 0.0, pitch: 0.0 };
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 2.0, camera, sky: false };
        let north = render(&mut m, &view).unwrap();
        let (x, y) = ink(&north.pixmap);
        assert!((x - 560.0).abs() < 4.0 && (y - 300.0).abs() < 4.0, "north up: {x},{y}");
        // Facing east, east is up.
        let turned = View { camera: Camera { bearing: 90.0, ..camera }, ..view };
        let r = render(&mut m, &turned).unwrap();
        assert_eq!((r.pixmap.width(), r.pixmap.height()), (800, 600));
        let (x, y) = ink(&r.pixmap);
        assert!((x - 400.0).abs() < 4.0 && (y - 140.0).abs() < 4.0, "facing east: {x},{y}");
        // The label sits by the point in the turned view (its text is drawn upright over it).
        let l = r.labels.iter().find(|l| l.text == "East").expect("the label is placed");
        assert!((l.center_x() - 200.0).abs() < 40.0 && (l.y - 70.0).abs() < 20.0, "label at {},{}", l.x, l.y);
        // Any bearing covers the whole view: no corner is left empty.
        let r = render(&mut m, &View { camera: Camera { bearing: -33.0, ..camera }, ..view }).unwrap();
        assert!(r.pixmap.pixels().iter().all(|p| p.alpha() == 255));
        // A bearing set straight on the camera is normalised: no number faces north, 450 is 90.
        let nan = render(&mut m, &View { camera: Camera { bearing: f64::NAN, ..camera }, ..view }).unwrap();
        assert_eq!(nan.pixmap.data(), north.pixmap.data());
        let (x, y) = ink(&render(&mut m, &View { camera: Camera { bearing: 450.0, ..camera }, ..view }).unwrap().pixmap);
        assert!((x - 400.0).abs() < 4.0 && (y - 140.0).abs() < 4.0, "450°: {x},{y}");
    }

    /// A hill of a DEM 3000 m high in the middle of a 2° square around (0, 0).
    fn hill_map() -> Map {
        use rustgis_data::raster::{ColorHint, Crs, GeoTransform, Raster};
        let mut m = Map::new("Map");
        m.basemap = rustgis_map::Basemap::Blank;
        let n = 64usize;
        let heights: Vec<f32> = (0..n * n)
            .map(|i| {
                let (x, y) = ((i % n) as f32 / n as f32 - 0.5, (i / n) as f32 / n as f32 - 0.5);
                3000.0 * (-(x * x + y * y) * 30.0).exp()
            })
            .collect();
        let gt = GeoTransform { origin_x: -1.0, origin_y: 1.0, pixel_width: 2.0 / n as f64, pixel_height: -2.0 / n as f64 };
        let dem = Raster::new("dem", n as u32, n as u32, vec![heights], gt, Crs::LonLat, None, ColorHint::Gray, "Float32").unwrap();
        let id = m.add_raster("dem", rustgis_map::RasterLayer::from_path(dem, "dem.tif"));
        if let Some(l) = m.layer_mut(id) {
            l.visible = false;
        }
        let mut d = Dataset::new("peak", GeometryType::Point, vec![]);
        d.push(Some(Geometry::Point(Coord::new(0.0, 0.0))), vec![]);
        m.add_dataset(d);
        m
    }

    #[test]
    fn a_tilted_view_drapes_the_map_over_its_terrain() {
        let camera = Camera { center: lonlat_to_merc(Coord::new(0.0, -0.6)), res: 600.0, bearing: 0.0, pitch: 65.0 };
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera, sky: false };
        let mut flat = hill_map();
        let mut hilly = hill_map();
        hilly.view.terrain =
            Some(rustgis_map::view::Terrain { source: rustgis_map::view::TerrainSource::Layer("dem".into()), exaggeration: 2.0 });
        // The ground's height is read from the DEM (exaggerated), and nothing off it.
        let g = terrain::Ground::of(&hilly, &camera).expect("terrain");
        let top = g.height(lonlat_to_merc(Coord::new(0.0, 0.0))).unwrap();
        assert!((top - 6000.0).abs() < 400.0, "{top}");
        assert!(g.height(lonlat_to_merc(Coord::new(5.0, 5.0))).is_none());
        assert!(terrain::Ground::of(&flat, &camera).is_none());
        // The marker on the peak stands on it: higher up the view than on flat ground.
        let place = |m: &mut Map| {
            let up = upright_markers(m, &view, 0.0);
            let k = up.first().expect("the peak's marker");
            k.place(&camera, view.width, view.height).expect("in view").1
        };
        let (y_flat, y_hill) = (place(&mut flat), place(&mut hilly));
        // The GPU draws the ground flat: a view over terrain stays on the CPU.
        assert!(gpu_may_turn(&flat, &view) && !gpu_may_turn(&hilly, &view));
        assert!(y_hill < y_flat - 5.0, "{y_hill} vs {y_flat}");
        // The frame shows the relief: it differs from the flat one, and the sky still shows.
        let a = render(&mut flat, &view).unwrap();
        let b = render(&mut hilly, &view).unwrap();
        assert_ne!(a.pixmap.data(), b.pixmap.data());
        assert!(b.pixmap.pixels().iter().all(|p| p.alpha() == 255));
        // A DEM layer gone (renamed or removed) leaves the ground flat.
        hilly.view.terrain =
            Some(rustgis_map::view::Terrain { source: rustgis_map::view::TerrainSource::Layer("none".into()), exaggeration: 1.0 });
        assert!(terrain::Ground::of(&hilly, &camera).is_none());
        assert!(render(&mut hilly, &view).is_some());
    }

    #[test]
    fn a_tilted_view_draws_the_ground_in_perspective_under_a_sky() {
        let mut m = Map::new("Map");
        m.basemap = rustgis_map::Basemap::Blank;
        let mut d = Dataset::new("pts", GeometryType::Point, vec![Field::new("name", FieldType::Text)]);
        // 100 px north of the centre at its scale.
        d.push(Some(Geometry::Point(Coord::new(0.0, 100.0 * 1000.0 / 111_319.49))), vec![Value::Text("Ahead".into())]);
        let id = m.add_dataset(d);
        if let Some(l) = m.layer_mut(id) {
            l.labels.enabled = true;
        }
        let camera = Camera { center: Coord::new(0.0, 0.0), res: 1000.0, bearing: 0.0, pitch: 60.0 };
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 2.0, camera, sky: false };
        let r = render(&mut m, &view).unwrap();
        assert_eq!((r.pixmap.width(), r.pixmap.height()), (800, 600));
        // The point is drawn where the camera puts it: ahead, foreshortened toward the centre.
        let (_, want_y) = camera.to_screen(lonlat_to_merc(Coord::new(0.0, 100.0 * 1000.0 / 111_319.49)), 400.0, 300.0);
        assert!(want_y < 150.0 - 20.0 && want_y > 150.0 - 100.0, "{want_y}");
        let bg = r.pixmap.pixel(400, 590).unwrap();
        let (mut n, mut sy) = (0.0, 0.0);
        for (k, p) in r.pixmap.pixels().iter().enumerate() {
            let (x, y) = (k as u32 % 800, k as u32 / 800);
            if (390..410).contains(&x) && y > 150 && *p != bg {
                n += 1.0;
                sy += f64::from(y);
            }
        }
        assert!(n > 0.0 && (sy / n / 2.0 - want_y).abs() < 4.0, "drawn at {}, want {want_y}", sy / n / 2.0);
        // Its label is placed there, upright.
        let l = r.labels.iter().find(|l| l.text == "Ahead").expect("placed");
        assert!((f64::from(l.y) - want_y).abs() < 20.0, "{}", l.y);
        // Steeper, the sky shows at the top in the sky colour, and the ground at the bottom.
        let steep = View { camera: Camera { pitch: 80.0, ..camera }, ..view };
        let r = render(&mut m, &steep).unwrap();
        let (sky, _) = tilt_sky(&m);
        let top = r.pixmap.pixel(400, 0).unwrap();
        assert!((i32::from(top.red()) - i32::from(sky.r())).abs() <= 2 && (i32::from(top.blue()) - i32::from(sky.b())).abs() <= 2);
        assert!(r.pixmap.pixels().iter().all(|p| p.alpha() == 255));
        // A pitch past the limit or no number is kept to the limit or looks straight down.
        assert!(render(&mut m, &View { camera: Camera { pitch: 400.0, ..camera }, ..view }).is_some());
        let flat = render(&mut m, &View { camera: Camera { pitch: 0.0, ..camera }, ..view }).unwrap();
        let nan = render(&mut m, &View { camera: Camera { pitch: f64::NAN, ..camera }, ..view }).unwrap();
        assert_eq!(nan.pixmap.data(), flat.pixmap.data());
    }

    #[test]
    fn point_markers_stand_upright_in_a_tilted_view() {
        let mut m = Map::new("Map");
        m.basemap = rustgis_map::Basemap::Blank;
        let mut d = Dataset::new("pts", GeometryType::Point, vec![]);
        // At the centre, and 150 px ahead of it at its scale.
        let ahead = 150.0 * 1000.0 / 111_319.49;
        d.push(Some(Geometry::Point(Coord::new(0.0, 0.0))), vec![]);
        d.push(Some(Geometry::Point(Coord::new(0.0, ahead))), vec![]);
        let id = m.add_dataset(d);
        let red = Rgba::rgb(255, 0, 0);
        if let Some(l) = m.layer_mut(id) {
            l.renderer =
                rustgis_map::Renderer::Single(Symbol { fill: red, stroke: red, stroke_width: 0.0, size: 20.0, marker: Marker::Circle });
            l.selection = vec![1];
        }
        let camera = Camera { center: Coord::new(0.0, 0.0), res: 1000.0, bearing: 30.0, pitch: 70.0 };
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera, sky: false };
        let r = render(&mut m, &view).unwrap();
        // The red pixels' box in the rows `ys` (logical px).
        let bbox = |ys: std::ops::Range<u32>| {
            let (mut x0, mut x1, mut y0, mut y1) = (u32::MAX, 0, u32::MAX, 0);
            for (k, p) in r.pixmap.pixels().iter().enumerate() {
                let (x, y) = (k as u32 % 400, k as u32 / 400);
                if ys.contains(&y) && p.red() > 200 && p.green() < 60 && p.blue() < 60 {
                    (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
                }
            }
            (x1.saturating_sub(x0) + 1, y1.saturating_sub(y0) + 1)
        };
        // The marker at the centre faces the viewer: as tall as it is wide, its symbol's size,
        // where on the ground it would be squashed to a third as tall.
        let (w, h) = bbox(135..166);
        assert!((18..=22).contains(&w) && w.abs_diff(h) <= 1, "{w} × {h}");
        // The one ahead is farther: smaller, still round, and selected (the selection colour
        // rings it).
        let (_, y) = camera.to_screen(lonlat_to_merc(Coord::new(0.0, ahead)), 400.0, 300.0);
        let y = y.round() as u32;
        let (fw, fh) = bbox(y.saturating_sub(12)..y + 12);
        assert!(fw < w && fw > 4 && fw.abs_diff(fh) <= 1, "{fw} × {fh}");
        let ring = r.pixmap.pixels().iter().any(|p| p.red() < 40 && p.green() > 200 && p.blue() > 230);
        assert!(ring, "the selected marker is ringed");
        // Both stand in the drape's list, far first.
        let up = upright_markers(&mut m, &view, 0.3);
        assert_eq!(up.len(), 2);
        assert!(up.first().is_some_and(|u| u.selected) && up.get(1).is_some_and(|u| !u.selected));
        // A point layer seen flat draws as before, and an aggregation stays on the ground.
        if let Some(l) = m.layer_mut(id) {
            l.aggregation = Some(rustgis_map::effects::Aggregation { hexagons: true, size: 20.0 });
        }
        assert!(upright_markers(&mut m, &view, 0.3).is_empty());
    }

    #[test]
    fn extrusions_stand_up_in_a_tilted_view() {
        let mut m = Map::new("Map");
        m.basemap = rustgis_map::Basemap::Blank;
        let mut d = Dataset::new("blocks", GeometryType::Polygon, vec![Field::new("name", FieldType::Text)]);
        // A square about 200 km a side at the centre.
        let s = 0.9;
        let ring = vec![Coord::new(-s, -s), Coord::new(s, -s), Coord::new(s, s), Coord::new(-s, s), Coord::new(-s, -s)];
        d.push(Some(Geometry::Polygon(vec![vec![ring]])), vec![Value::Text("Block".into())]);
        let id = m.add_dataset(d);
        if let Some(l) = m.layer_mut(id) {
            l.labels.enabled = true;
            l.extrusion = Some(rustgis_map::effects::Extrusion {
                field: None,
                height: 100_000.0,
                factor: 1.0,
                face_culling: true,
                lighting: true,
                shadows: true,
            });
        }
        let camera = Camera { center: Coord::new(0.0, 0.0), res: 2000.0, bearing: 30.0, pitch: 60.0 };
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera, sky: false };
        // The square on the ground spans these rows; its roof stands well above them.
        let (_, ground_top) = camera.to_screen(lonlat_to_merc(Coord::new(0.0, s)), 400.0, 300.0);
        let r = render(&mut m, &view).unwrap();
        let bg = r.pixmap.pixel(5, 295).unwrap();
        let top_ink = (0..300u32).find(|&y| (150..250).any(|x| r.pixmap.pixel(x, y).is_some_and(|p| p != bg))).unwrap();
        assert!(f64::from(top_ink) < ground_top - 15.0, "roof from row {top_ink}, ground from {ground_top}");
        // Its label is lifted onto the roof, above the ground under it.
        let l = r.labels.iter().find(|l| l.text == "Block").expect("placed");
        assert!(f64::from(l.y) < 150.0 - 15.0, "label at {}", l.y);
        // The moving view's tessellation holds the square, roof and walls, at its height.
        let shapes = extruded_shapes(&mut m, &view);
        assert_eq!(shapes.len(), 1);
        assert!(shapes[0].roof_indices.len() >= 6 && shapes[0].rings.len() == 1 && shapes[0].metres == 100_000.0);
        // Heights follow each feature's own scale, not the view centre's: at the equator a metre
        // is a Web Mercator unit, at 60°N two (#433 review; a view centred near a pole stood
        // features elsewhere up several times too tall).
        assert!((shapes[0].height / 100_000.0 - 1.0).abs() < 0.01, "{}", shapes[0].height);
        // A long block reaching from behind the eye into view stands up where it is in front (it
        // was dropped whole when its middle or a corner was out of reach).
        let long =
            vec![Coord::new(-0.5, -40.0), Coord::new(0.5, -40.0), Coord::new(0.5, 0.5), Coord::new(-0.5, 0.5), Coord::new(-0.5, -40.0)];
        assert!(camera.try_to_screen(lonlat_to_merc(Coord::new(0.0, -40.0)), 400.0, 300.0).is_none(), "behind the eye");
        let mut flat_map = m.clone();
        if let Some(l) = m.layers.first_mut() {
            l.dataset.features.clear();
            l.dataset.push(Some(Geometry::Polygon(vec![vec![long.clone()]])), vec![Value::Text("Long".into())]);
            l.touch_geometry();
        }
        if let Some(l) = flat_map.layers.first_mut() {
            l.dataset.features.clear();
            l.dataset.push(Some(Geometry::Polygon(vec![vec![long]])), vec![Value::Text("Long".into())]);
            l.touch_geometry();
            l.extrusion = None;
        }
        let up = render(&mut m, &view).unwrap();
        let down = render(&mut flat_map, &view).unwrap();
        let changed = up.pixmap.pixels().iter().zip(down.pixmap.pixels()).filter(|(a, b)| a != b).count();
        assert!(changed > 2000, "the long block stands up: {changed} px");
        if let Some(l) = m.layers.first_mut() {
            let north =
                vec![Coord::new(-1.0, 59.0), Coord::new(1.0, 59.0), Coord::new(1.0, 61.0), Coord::new(-1.0, 61.0), Coord::new(-1.0, 59.0)];
            l.dataset.features.clear();
            l.dataset.push(Some(Geometry::Polygon(vec![vec![north]])), vec![Value::Text("North".into())]);
            l.touch_geometry();
        }
        // Seen from a camera at another latitude, the block's height is still its own latitude's.
        // (A camera so far that the block is off its ground leaves it out: see
        // an_extruded_ring_with_a_hole_is_open_over_it.)
        let south = View { camera: Camera { center: lonlat_to_merc(Coord::new(0.0, 57.0)), ..camera }, ..view };
        let shapes = extruded_shapes(&mut m, &south);
        assert_eq!(shapes.len(), 1);
        assert!((shapes[0].height / 200_000.0 - 1.0).abs() < 0.05, "{}", shapes[0].height);
    }

    /// Extruded features with junk heights or roof indices (hostile data) give finite geometry
    /// or are left out, never garbage.
    #[test]
    fn gpu_extrusions_survive_junk_heights() {
        let ring = vec![Coord::new(0.0, 0.0), Coord::new(10.0, 0.0), Coord::new(10.0, 10.0), Coord::new(0.0, 10.0)];
        let shape = |height: f64, roof_indices: Vec<u32>| ExtrudedShape {
            origin: Coord::new(0.0, 0.0),
            center: Coord::new(5.0, 5.0),
            roof: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            roof_indices,
            rings: vec![ring.clone()],
            holes: vec![],
            metres: height,
            height,
            fill: Rgba::rgb(200, 100, 50),
            stroke: Rgba::rgb(0, 0, 0),
            face_culling: true,
            lighting: true,
            shadows: true,
        };
        let shapes = [shape(f64::NAN, vec![0, 1, 2, 0, 2, 3]), shape(f64::INFINITY, vec![0, 1, 2]), shape(5.0, vec![0, 1, 99])];
        for cmd in extrusion_cmds(&shapes, [0.0, 0.0]).expect("within the budget") {
            let gpu_scene::DrawCmd::Extrusion { vertices, indices, .. } = cmd else { panic!("an extrusion") };
            assert!(vertices.iter().all(|v| v.position.iter().chain(&v.info).all(|x| x.is_finite())));
            assert!(indices.iter().all(|i| (*i as usize) < vertices.len()));
        }
    }

    #[test]
    fn a_globe_turns_and_tilts() {
        let mut m = Map::new("Map");
        m.projection = rustgis_map::Projection::Globe;
        let camera = Camera { center: lonlat_to_merc(Coord::new(10.0, 40.0)), res: 20_000.0, bearing: 0.0, pitch: 0.0 };
        let view = View { width: 300.0, height: 200.0, pixels_per_point: 1.0, camera, sky: false };
        let plain = render(&mut m, &view).unwrap();
        let turned = render(&mut m, &View { camera: Camera { bearing: 40.0, ..camera }, ..view }).unwrap();
        let tilted = render(&mut m, &View { camera: Camera { pitch: 60.0, ..camera }, ..view }).unwrap();
        assert_ne!(plain.pixmap.data(), turned.pixmap.data());
        assert_ne!(plain.pixmap.data(), tilted.pixmap.data());
    }

    #[test]
    fn an_extruded_ring_with_a_hole_is_open_over_it() {
        let mut m = Map::new("Map");
        m.basemap = rustgis_map::Basemap::Blank;
        let mut d = Dataset::new("yard", GeometryType::Polygon, vec![]);
        let outer =
            vec![Coord::new(-2.0, -2.0), Coord::new(2.0, -2.0), Coord::new(2.0, 2.0), Coord::new(-2.0, 2.0), Coord::new(-2.0, -2.0)];
        let hole = vec![Coord::new(-1.2, -1.2), Coord::new(-1.2, 1.2), Coord::new(1.2, 1.2), Coord::new(1.2, -1.2), Coord::new(-1.2, -1.2)];
        d.push(Some(Geometry::Polygon(vec![vec![outer, hole]])), vec![]);
        let id = m.add_dataset(d);
        if let Some(l) = m.layer_mut(id) {
            l.extrusion = Some(rustgis_map::effects::Extrusion {
                field: None,
                height: 20_000.0,
                factor: 1.0,
                face_culling: true,
                lighting: true,
                shadows: false,
            });
        }
        let camera = Camera { center: Coord::new(0.0, 0.0), res: 2_000.0, bearing: 0.0, pitch: 30.0 };
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera, sky: false };
        let r = render(&mut m, &view).unwrap();
        // The middle of the courtyard shows the ground, not a roof over it.
        assert_eq!(r.pixmap.pixel(200, 150), r.pixmap.pixel(5, 295));
        // Its walls stand inside the roof's outline.
        let shapes = extruded_shapes(&mut m, &view);
        assert_eq!((shapes[0].rings.len(), shapes[0].holes.len()), (1, 1));
        // Off the ground the view shows, a feature isn't tessellated at all.
        let far = View { camera: Camera { center: Coord::new(5e6, 5e6), ..camera }, ..view };
        assert!(extruded_shapes(&mut m, &far).is_empty());
        // In a hidden group, the layer stands up nowhere.
        let mut g = rustgis_map::LayerGroup::new("hidden");
        g.visible = false;
        g.children = vec![id];
        m.groups.push(g);
        assert!(extruded_shapes(&mut m, &view).is_empty());
        let hidden = render(&mut m, &view).unwrap();
        assert_eq!(hidden.pixmap.pixel(200, 150), hidden.pixmap.pixel(5, 295));
    }

    #[test]
    fn only_features_still_under_a_pixel_with_their_outline_are_splatted() {
        let view = View {
            width: 100.0,
            height: 100.0,
            pixels_per_point: 2.0,
            camera: Camera { center: Coord::new(0.0, 0.0), res: 1.0, bearing: 0.0, pitch: 0.0 },
            sky: false,
        };
        let ctx = Ctx { view: &view, ppp: 2.0, wireframe: false, cull: Extent::EMPTY, globe: None, sprites: Default::default() };
        let ring = vec![Coord::new(0.0, 0.0), Coord::new(0.25, 0.0), Coord::new(0.25, 0.25), Coord::new(0.0, 0.25), Coord::new(0.0, 0.0)];
        let g = Geometry::Polygon(vec![vec![ring]]);
        let mut pm = Pixmap::new(200, 200).unwrap();
        let thin = Symbol { fill: Rgba::rgb(200, 0, 0), stroke: Rgba::rgb(0, 0, 0), stroke_width: 0.5, size: 0.0, marker: Marker::Circle };
        assert!(ctx.splat(&mut pm, &g.extent(), &g, &thin), "0.5 px across with a 1 px outline");
        // A 3 pt outline at 2× is 6 px wide: drawn as a ring, not a dot.
        let thick = Symbol { stroke_width: 3.0, ..thin };
        assert!(!ctx.splat(&mut pm, &g.extent(), &g, &thick));
    }

    #[test]
    fn offset_polylines_shift_right_of_their_direction() {
        // Screen y points down: right of an eastward line is below it.
        assert_eq!(offset_polyline(&[(0.0, 0.0), (10.0, 0.0)], 3.0), vec![(0.0, 3.0), (10.0, 3.0)]);
        assert_eq!(offset_polyline(&[(10.0, 0.0), (0.0, 0.0)], 3.0), vec![(10.0, -3.0), (0.0, -3.0)]);
        assert_eq!(offset_polyline(&[(0.0, 0.0), (10.0, 0.0)], -3.0), vec![(0.0, -3.0), (10.0, -3.0)]);
        // A right-angle corner is mitred: east then south, shifted right (inside the turn).
        let p = offset_polyline(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], 2.0);
        assert_eq!(p.len(), 3);
        assert!((p[1].0 - 8.0).abs() < 1e-4 && (p[1].1 - 2.0).abs() < 1e-4, "{p:?}");
        // Doubling back is bevelled rather than mitred far away.
        let p = offset_polyline(&[(0.0, 0.0), (10.0, 0.0), (0.0, 0.1)], 2.0);
        assert!(p.iter().all(|q| q.0.abs() < 20.0 && q.1.abs() < 20.0), "{p:?}");
        // Repeated, non-finite and lone points give nothing to draw, and no NaN.
        assert!(offset_polyline(&[(1.0, 1.0), (1.0, 1.0)], 2.0).is_empty());
        assert!(offset_polyline(&[(f32::NAN, 0.0), (1.0, 1.0)], 2.0).is_empty());
        let p = offset_polyline(&[(0.0, 0.0), (0.0, 0.0), (5.0, 0.0), (5.0, 0.0)], 1.0);
        assert_eq!(p, vec![(0.0, 1.0), (5.0, 1.0)]);
    }

    #[test]
    fn stacked_line_strokes_draw_offset_by_direction() {
        // Two copies of one line in opposite directions (#478): with a stroke offset to the right
        // they land on opposite sides, and nothing draws on the shared centreline.
        let mut m = Map::new("Map");
        m.basemap = rustgis_map::Basemap::Blank;
        let mut d = Dataset::new("edges", GeometryType::Polyline, vec![]);
        d.push(Some(Geometry::Line(vec![vec![Coord::new(-1.0, 0.0), Coord::new(1.0, 0.0)]])), vec![]);
        d.push(Some(Geometry::Line(vec![vec![Coord::new(1.0, 0.0), Coord::new(-1.0, 0.0)]])), vec![]);
        let id = m.add_dataset(d);
        let camera = Camera { center: lonlat_to_merc(Coord::new(0.0, 0.0)), res: 1500.0, bearing: 0.0, pitch: 0.0 };
        let view = View { width: 200.0, height: 200.0, pixels_per_point: 1.0, camera, sky: false };
        let red = Rgba::rgb(255, 0, 0);
        let is_red = |r: &Rendered, y: u32| {
            let p = r.pixmap.pixel(100, y).unwrap();
            (p.red(), p.green(), p.blue()) == (255, 0, 0)
        };
        let r = render(&mut m, &view).unwrap();
        let centre = r.pixmap.pixel(100, 100).unwrap();
        let blank = r.pixmap.pixel(100, 60).unwrap();
        assert_ne!(centre, blank, "the single stroke draws on the centreline");
        m.layer_mut(id).unwrap().line_strokes = vec![
            rustgis_map::LineStroke { color: Some(Rgba::rgb(0, 0, 0)), width: Some(5.0), offset: 10.0, ..Default::default() },
            rustgis_map::LineStroke { color: Some(red), width: Some(2.0), offset: 10.0, ..Default::default() },
        ];
        let r = render(&mut m, &view).unwrap();
        assert!(is_red(&r, 110) && is_red(&r, 90), "both copies, one on each side");
        assert_eq!(r.pixmap.pixel(100, 100).unwrap(), blank, "nothing on the centreline");
        // The casing shows around the core.
        let casing = r.pixmap.pixel(100, 111).unwrap();
        assert_eq!((casing.red(), casing.green(), casing.blue()), (0, 0, 0));
        // Selected, it still draws offset, with its halo; a hidden stroke gets no halo.
        m.layer_mut(id).unwrap().line_strokes.push(rustgis_map::LineStroke { width: Some(0.0), offset: -20.0, ..Default::default() });
        m.layer_mut(id).unwrap().selection = vec![0];
        let r = render(&mut m, &view).unwrap();
        assert!(is_red(&r, 110));
        assert_eq!(r.pixmap.pixel(100, 100).unwrap(), blank);
        assert_eq!(r.pixmap.pixel(100, 80).unwrap(), blank, "no halo for the hidden stroke");
        // A line under a pixel across still draws shifted, not splatted on its centreline.
        let mut d = Dataset::new("short", GeometryType::Polyline, vec![]);
        d.push(Some(Geometry::Line(vec![vec![Coord::new(0.0, 0.0), Coord::new(0.0005, 0.0)]])), vec![]);
        let short = m.add_dataset(d);
        m.layer_mut(id).unwrap().visible = false;
        m.layer_mut(short).unwrap().line_strokes =
            vec![rustgis_map::LineStroke { color: Some(red), width: Some(3.0), offset: 10.0, ..Default::default() }];
        let r = render(&mut m, &view).unwrap();
        assert_ne!(r.pixmap.pixel(100, 110).unwrap(), blank, "drawn 10 pt to the right");
        assert_eq!(r.pixmap.pixel(100, 100).unwrap(), blank, "nothing on the centreline");
    }

    #[test]
    fn features_under_a_pixel_still_show() {
        // A dense grid of tiny squares seen whole: each is splatted rather than filled as a path,
        // and the area still comes out the colour of their outlines.
        let mut m = Map::new("Map");
        m.basemap = rustgis_map::Basemap::Blank;
        let mut d = Dataset::new("cells", GeometryType::Polygon, vec![]);
        for i in 0..100 {
            for j in 0..100 {
                let (x, y) = (i as f64 * 0.01, j as f64 * 0.01);
                let ring = vec![
                    Coord::new(x, y),
                    Coord::new(x + 0.005, y),
                    Coord::new(x + 0.005, y + 0.005),
                    Coord::new(x, y + 0.005),
                    Coord::new(x, y),
                ];
                d.push(Some(Geometry::Polygon(vec![vec![ring]])), vec![]);
            }
        }
        let id = m.add_dataset(d);
        let sym = m.layer(id).map(|l| l.symbol(0)).unwrap();
        let camera = Camera { center: lonlat_to_merc(Coord::new(0.5, 0.5)), res: 2000.0, bearing: 0.0, pitch: 0.0 };
        let view = View { width: 200.0, height: 200.0, pixels_per_point: 1.0, camera, sky: false };
        let r = render(&mut m, &view).unwrap();
        let px = |x: u32, y: u32| r.pixmap.pixel(x, y).unwrap();
        let (centre, corner) = (px(100, 100), px(2, 2));
        assert_ne!((centre.red(), centre.green(), centre.blue()), (corner.red(), corner.green(), corner.blue()));
        // Towards the outline's colour (darker than the blank basemap).
        let lum = |p: tiny_skia::PremultipliedColorU8| p.red() as u32 + p.green() as u32 + p.blue() as u32;
        assert!(lum(centre) < lum(corner), "{centre:?} vs {corner:?} ({sym:?})");
    }

    /// The globe and the tilted view on another body: no atmosphere glows past the limb, and the
    /// tilted sky is the black of space (#223).
    #[test]
    fn other_bodies_have_no_atmosphere() {
        let glow_at = |body: &str| {
            let mut m = Map::new("Map");
            m.body = body.into();
            m.graticule = false;
            m.projection = rustgis_map::Projection::Globe;
            let camera = Camera { center: Coord::new(0.0, 0.0), res: 60_000.0, bearing: 0.0, pitch: 0.0 };
            let view = View { width: 400.0, height: 400.0, pixels_per_point: 1.0, camera, sky: true };
            let g = Globe::new(&camera, 400.0, 400.0);
            let (cx, cy) = g.center();
            let pm = render(&mut m, &view).unwrap().pixmap;
            let p = pm.pixel((cx + g.disc_radius() * 1.03) as u32, cy as u32).unwrap();
            (p.red(), p.green(), p.blue())
        };
        let space = globe_space(true);
        assert_ne!(glow_at("earth"), (space.r(), space.g(), space.b()));
        assert_eq!(glow_at("moon"), (space.r(), space.g(), space.b()));
        let mut m = Map::new("Map");
        m.body = "mars".into();
        assert_eq!(tilt_sky(&m), (space, space));
    }
}

#[cfg(test)]
mod stream_tests;

#[cfg(test)]
mod budget_tests;
