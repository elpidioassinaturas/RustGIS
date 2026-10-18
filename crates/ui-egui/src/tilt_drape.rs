//! The moving tilted map. While a tilted map pans, zooms, turns or tilts, its ground drawn flat
//! once in bands (`render::render_tilt_drape`) is laid over the tilted ground as textured meshes,
//! so a frame only moves vertices, as the globe's drape does; the sky above the horizon is a
//! gradient. Extruded layers stand up over it in perspective as MapLibre's fill extrusions do:
//! tessellated once, then projected, lit and ordered far to near every frame. The exact frame is
//! drawn again once the map settles.

use egui::epaint::Vertex;
use egui::{Color32, Mesh, Painter, Rect, Shape, TextureFilter, TextureHandle, TextureOptions, pos2};
use rustgis_engine::geom::{Coord, Extent};
use rustgis_engine::map::camera::{Band, FAR};
use rustgis_engine::map::{Camera, Map};
use rustgis_engine::render::{self, ExtrudedShape, View};

use crate::globe_drape::DrapeKey;
use crate::theme::c32;

/// How far past its band each picture reaches on every side, as a fraction of its size: the map
/// can move this far before the drape is drawn again.
const MARGIN: f64 = 0.3;
/// Zooming in past this factor of the drape's scale draws it again, sharper.
const MAX_ZOOM_IN: f64 = 2.0;
/// Seconds between drawing the drape again for tiles that have arrived meanwhile.
const TILE_REFRESH: f64 = 0.5;
/// Mesh cells along each side of a band: fine enough that the texture's straight-line
/// interpolation across a cell doesn't show the perspective.
const CELLS: usize = 16;

struct Drape {
    key: DrapeKey,
    ppp: f32,
    /// The camera's resolution when it was drawn.
    res: f64,
    tiles: u64,
    built: f64,
    /// Each band and its picture.
    bands: Vec<(Band, TextureHandle)>,
    /// The extruded features, tessellated.
    shapes: Vec<ExtrudedShape>,
}

/// The drape of the moving tilted map, kept between frames while the map doesn't change.
#[derive(Default)]
pub(crate) struct TiltDrape {
    drape: Option<Drape>,
}

impl TiltDrape {
    /// Lets go of the textures (the map no longer tilts).
    pub fn clear(&mut self) {
        self.drape = None;
    }

    /// Makes sure the drape covers the tilted `w × h` view of `map`, drawing it again when the
    /// view has moved or zoomed past it or the map changed. False when there is nothing to
    /// drape.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(&mut self, ctx: &egui::Context, map: &mut Map, key: DrapeKey, tiles: u64, w: f64, h: f64, ppp: f32, time: f64) -> bool {
        let cam = map.camera;
        // A screen's worth of detail at most: the settled frame is sharp.
        let drape_ppp = ppp.min(1.0);
        let fresh = self.drape.as_ref().is_some_and(|d| {
            d.key == key
                && d.ppp == drape_ppp
                && cam.res * MAX_ZOOM_IN >= d.res
                && (d.tiles == tiles || time - d.built < TILE_REFRESH)
                && covers(&cam, w, h, &d.bands)
        });
        if fresh {
            return true;
        }
        let max_side = ctx.input(|i| i.max_texture_side).clamp(256, 4096) as u32;
        let view = View { width: w, height: h, pixels_per_point: f64::from(drape_ppp), camera: cam, sky: true };
        let patches = render::render_tilt_drape(map, &view, MARGIN, max_side);
        if patches.is_empty() {
            self.drape = None;
            return false;
        }
        let options = TextureOptions { mipmap_mode: Some(TextureFilter::Linear), ..TextureOptions::LINEAR };
        let mut old = self.drape.take().map(|d| d.bands).unwrap_or_default().into_iter().map(|b| b.1);
        let bands = patches
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let img =
                    egui::ColorImage::from_rgba_premultiplied([p.pixmap.width() as usize, p.pixmap.height() as usize], p.pixmap.data());
                let tex = match old.next() {
                    Some(mut t) => {
                        t.set(img, options);
                        t
                    }
                    None => ctx.load_texture(format!("tilt_drape_{i}"), img, options),
                };
                (p.band, tex)
            })
            .collect();
        let shapes = render::extruded_shapes(map, &view);
        self.drape = Some(Drape { key, ppp: drape_ppp, res: cam.res, tiles, built: time, bands, shapes });
        true
    }

    /// Paints the tilted map in `rect`: the sky, and the drape's bands, farthest first.
    pub fn paint(&self, painter: &Painter, rect: Rect, map: &Map) {
        let (w, h) = (rect.width() as f64, rect.height() as f64);
        let cam = map.camera;
        let (sky, fog) = render::tilt_sky(map);
        painter.rect_filled(rect, 0.0, c32(fog));
        if let Some(y) = cam.sky_y(h) {
            let y = rect.min.y + (y as f32).min(rect.height());
            let mut m = Mesh::default();
            let (top, bottom) = (c32(sky), c32(fog));
            for (p, c) in [(rect.left_top(), top), (rect.right_top(), top), (pos2(rect.max.x, y), bottom), (pos2(rect.min.x, y), bottom)] {
                m.colored_vertex(p, c);
            }
            m.add_triangle(0, 1, 2);
            m.add_triangle(0, 2, 3);
            painter.add(Shape::mesh(m));
        }
        for mesh in self.meshes(rect, &cam, w, h) {
            painter.add(Shape::mesh(mesh));
        }
        if let Some(mesh) = self.extrusions(rect, &cam, fog) {
            painter.add(Shape::mesh(mesh));
        }
    }

    /// The extruded features standing on the drape in perspective, as one mesh: features far to
    /// near, each one's walls (facing the eye, lit by their facing and darker toward their foot,
    /// far first) then its roof; fading into `fog` with distance. Heights are each
    /// feature's own, in the camera's units at its place.
    fn extrusions(&self, rect: Rect, cam: &Camera, fog: rustgis_engine::map::Rgba) -> Option<Mesh> {
        let d = self.drape.as_ref()?;
        let (w, h) = (rect.width() as f64, rect.height() as f64);
        let pos = |p: (f64, f64)| pos2(rect.min.x + p.0 as f32, rect.min.y + p.1 as f32);
        let ground =
            |s: &ExtrudedShape, p: &[f32; 2]| cam.ground_px(Coord::new(s.origin.x + f64::from(p[0]), s.origin.y + f64::from(p[1])));
        // Far (up the screen) first. A feature is drawn while any of its outline is in front of
        // the eye and within the view's reach (past it is sky); its middle places it and fogs it.
        let mut order: Vec<(f64, f64, usize)> = d
            .shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| s.rings.iter().flatten().any(|c| cam.depth_scale(*c, h).is_some_and(|k| k * FAR >= 1.0)))
            .map(|(i, s)| {
                let depth = cam.depth_scale(s.center, h).map_or(1.0, |k| 1.0 / k.max(1e-9));
                (cam.ground_px(s.center).1, depth, i)
            })
            .collect();
        order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        type Rings = Vec<Vec<(f64, f64)>>;
        let to_ground = |rings: &[Vec<Coord>]| -> Rings { rings.iter().map(|r| r.iter().map(|c| cam.ground_px(*c)).collect()).collect() };
        let shapes: Vec<(&ExtrudedShape, f64, f64, Rings, Rings)> = order
            .iter()
            .filter_map(|&(_, depth, i)| {
                let s = d.shapes.get(i)?;
                Some((s, depth, (s.height / cam.res.max(1e-9)).clamp(0.0, 1e7), to_ground(&s.rings), to_ground(&s.holes)))
            })
            .collect();
        let mut mesh = Mesh::default();
        // Shadows on the ground first, under every feature.
        let shade = Color32::from_black_alpha(60);
        for (s, _, up, rings, _) in shapes.iter().filter(|x| x.0.shadows) {
            for q in render::tilted_shadow_quads(cam, w, h, rings, *up) {
                fan(&mut mesh, q.into_iter().map(|p| Some(pos(p))), shade);
            }
            // The footprint moved away from the light: the roof's triangles on the ground.
            let (du, dv) = render::shadow_shift(*up);
            for [a, b, c] in s.roof_indices.as_chunks::<3>().0 {
                let tri: Option<Vec<(f64, f64)>> =
                    [a, b, c].iter().map(|&&k| s.roof.get(k as usize).map(|p| ground(s, p)).map(|(u, v)| (u + du, v + dv))).collect();
                let Some(tri) = tri else { continue };
                let cut = render::clip_near(&tri, cam.near_v(0.0, h));
                fan(&mut mesh, cut.into_iter().map(|(u, v)| cam.project_raised(u, v, 0.0, w, h).map(pos)), shade);
            }
        }
        for (s, depth, up, rings, holes) in &shapes {
            let t = ((depth - FAR * 0.5) / (FAR * 0.5)).clamp(0.0, 1.0);
            let base = s.fill.lerp(fog, t * t * (3.0 - 2.0 * t));
            for wall in render::tilted_walls(cam, w, h, rings, holes, *up, s.face_culling, s.lighting) {
                let (top, foot) = (c32(base.shade(wall.top)), c32(base.shade(wall.foot)));
                let [fa, fb, tb, ta] = wall.quad;
                let k = mesh.vertices.len() as u32;
                for (p, c) in [(fa, foot), (fb, foot), (tb, top), (ta, top)] {
                    mesh.colored_vertex(pos(p), c);
                }
                mesh.add_triangle(k, k + 1, k + 2);
                mesh.add_triangle(k, k + 2, k + 3);
            }
            // The roof's triangles, each cut where it reaches behind the eye.
            let color = c32(base);
            let v_max = cam.near_v(*up, h);
            for [a, b, c] in s.roof_indices.as_chunks::<3>().0 {
                let tri: Option<Vec<(f64, f64)>> = [a, b, c].iter().map(|&&k| s.roof.get(k as usize).map(|p| ground(s, p))).collect();
                let Some(tri) = tri else { continue };
                let cut = render::clip_near(&tri, v_max);
                fan(&mut mesh, cut.into_iter().map(|(u, v)| cam.project_raised(u, v, *up, w, h).map(pos)), color);
            }
        }
        (!mesh.indices.is_empty()).then_some(mesh)
    }

    /// The drape's meshes for `cam` in `rect`, farthest band first. A band with nothing in front
    /// of the camera has no mesh: egui-wgpu can't draw an empty one.
    fn meshes(&self, rect: Rect, cam: &Camera, w: f64, h: f64) -> Vec<Mesh> {
        let Some(d) = &self.drape else { return Vec::new() };
        d.bands
            .iter()
            .rev()
            .filter_map(|(b, tex)| {
                let n = CELLS + 1;
                let mut mesh = Mesh::with_texture(tex.id());
                let mut ok = Vec::with_capacity(n * n);
                for j in 0..n {
                    for i in 0..n {
                        let (u, v) = (i as f64 / CELLS as f64, j as f64 / CELLS as f64);
                        let m = b.camera.to_map(u * b.width, v * b.height, b.width, b.height);
                        // Past the view's reach is sky, as in the settled frame.
                        let in_reach = cam.depth_scale(m, h).is_some_and(|k| k >= 1.0 / FAR);
                        let s = cam.try_to_screen(m, w, h).filter(|s| in_reach && s.0.is_finite() && s.1.is_finite());
                        ok.push(s.is_some());
                        let (x, y) = s.unwrap_or((0.0, 0.0));
                        mesh.vertices.push(Vertex {
                            pos: pos2(rect.min.x + x as f32, rect.min.y + y as f32),
                            uv: pos2(u as f32, v as f32),
                            color: Color32::WHITE,
                        });
                    }
                }
                for j in 0..CELLS {
                    for i in 0..CELLS {
                        let k = j * n + i;
                        let quad = [k, k + 1, k + n, k + n + 1];
                        if quad.iter().all(|&q| ok.get(q).copied().unwrap_or(false)) {
                            let [a, b, c, e] = quad.map(|q| q as u32);
                            mesh.add_triangle(a, b, e);
                            mesh.add_triangle(a, e, c);
                        }
                    }
                }
                (!mesh.indices.is_empty()).then_some(mesh)
            })
            .collect()
    }
}

/// Adds a convex polygon (screen points, `None` where one couldn't be placed) to `mesh` as a fan
/// of triangles in one colour; nothing when a point is missing or there are fewer than three.
fn fan(mesh: &mut Mesh, pts: impl Iterator<Item = Option<egui::Pos2>>, color: Color32) {
    let pts: Option<Vec<egui::Pos2>> = pts.map(|p| p.filter(|p| p.x.is_finite() && p.y.is_finite())).collect();
    let Some(pts) = pts.filter(|p| p.len() >= 3) else { return };
    let k = mesh.vertices.len() as u32;
    for p in &pts {
        mesh.colored_vertex(*p, color);
    }
    for i in 1..pts.len() as u32 - 1 {
        mesh.add_triangle(k, k + i, k + i + 1);
    }
}

/// Whether the bands cover the ground a tilted `w × h` view of `cam` shows (sampled on a grid;
/// the sky needs no cover).
fn covers(cam: &Camera, w: f64, h: f64, bands: &[(Band, TextureHandle)]) -> bool {
    let extents: Vec<Extent> = bands.iter().map(|(b, _)| b.camera.extent(b.width, b.height)).collect();
    let n = 6;
    (0..=n).all(|j| {
        (0..=n).all(|i| {
            let (x, y) = (w * f64::from(i) / f64::from(n), h * f64::from(j) / f64::from(n));
            match cam.try_to_map(x, y, w, h) {
                Some(m) => extents.iter().any(|e| e.contains(Coord::new(m.x, m.y))),
                None => true,
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use rustgis_engine::geom::lonlat_to_merc;

    use super::*;

    fn tilted(pitch: f64) -> Map {
        let mut m = Map::new("m");
        m.camera = Camera { center: lonlat_to_merc(Coord::new(10.0, 48.0)), res: 2_000.0, bearing: 20.0, pitch };
        m
    }

    fn fog() -> rustgis_engine::map::Rgba {
        rustgis_engine::map::Rgba::rgb(200, 210, 220)
    }

    #[test]
    fn extrusions_stand_up_on_the_drape() {
        use rustgis_engine::data::{Dataset, Geometry, GeometryType};
        let ctx = egui::Context::default();
        let key = DrapeKey { rev: 0, sources: 0 };
        let mut m = tilted(55.0);
        let mut ds = Dataset::new("blocks", GeometryType::Polygon, vec![]);
        let ring =
            vec![Coord::new(9.0, 47.0), Coord::new(11.0, 47.0), Coord::new(11.0, 49.0), Coord::new(9.0, 49.0), Coord::new(9.0, 47.0)];
        ds.push(Some(Geometry::Polygon(vec![vec![ring]])), vec![]);
        let id = m.add_dataset(ds);
        if let Some(l) = m.layer_mut(id) {
            l.extrusion = Some(rustgis_engine::map::effects::Extrusion {
                field: None,
                height: 100_000.0,
                factor: 1.0,
                face_culling: true,
                lighting: true,
                shadows: false,
            });
        }
        let mut d = TiltDrape::default();
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.0));
        let rect = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let mesh = d.extrusions(rect, &m.camera, fog()).expect("the block stands up");
        assert!(mesh.is_valid() && mesh.indices.len() > 6);
        // A long block whose first point is behind the eye still stands up where it reaches into
        // view (it was kept or dropped by that point alone, and vanished while moving).
        if let Some(l) = m.layer_mut(id) {
            let long = vec![
                Coord::new(10.0, 20.0),
                Coord::new(11.0, 20.0),
                Coord::new(11.0, 48.5),
                Coord::new(10.0, 48.5),
                Coord::new(10.0, 20.0),
            ];
            l.dataset.features.clear();
            l.dataset.push(Some(Geometry::Polygon(vec![vec![long]])), vec![]);
            l.touch_geometry();
        }
        let behind = rustgis_engine::geom::lonlat_to_merc(Coord::new(10.0, 20.0));
        assert!(m.camera.try_to_screen(behind, 800.0, 600.0).is_none(), "the first point is behind the eye");
        let mut d2 = TiltDrape::default();
        assert!(d2.prepare(&ctx, &mut m, DrapeKey { rev: 1, sources: 0 }, 0, 800.0, 600.0, 1.0, 0.0));
        assert!(d2.extrusions(rect, &m.camera, fog()).is_some_and(|mesh| mesh.is_valid() && !mesh.indices.is_empty()));
        // Its roof stands above its foot on screen.
        let foot = m.camera.to_screen(rustgis_engine::geom::lonlat_to_merc(Coord::new(10.0, 47.0)), 800.0, 600.0).1 as f32;
        let top = mesh.vertices.iter().map(|v| v.pos.y).fold(f32::MAX, f32::min);
        assert!(top < foot - 20.0, "{top} vs {foot}");
    }

    #[test]
    fn the_drape_covers_the_view_and_is_kept_while_it_does() {
        let ctx = egui::Context::default();
        let key = DrapeKey { rev: 0, sources: 0 };
        let mut m = tilted(60.0);
        let mut d = TiltDrape::default();
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.0));
        let built = d.drape.as_ref().map(|d| d.built);
        assert!(d.drape.as_ref().is_some_and(|d| d.bands.len() >= 2), "near and far bands");
        // A small pan stays inside the margin: nothing is drawn.
        m.camera.pan_px(30.0, 20.0);
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.1));
        assert_eq!(d.drape.as_ref().map(|d| d.built), built);
        // No extruded layer, no extrusion mesh.
        assert!(d.extrusions(Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(800.0, 600.0)), &m.camera, fog()).is_none());
        // Every band in front of the camera gives a valid mesh.
        let rect = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let meshes = d.meshes(rect, &m.camera, 800.0, 600.0);
        assert!(!meshes.is_empty() && meshes.iter().all(|mesh| !mesh.indices.is_empty() && mesh.is_valid()));
        // Zooming in past the drape's detail draws it again.
        m.camera.res /= 3.0;
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.2));
        assert_ne!(d.drape.as_ref().map(|d| d.built), built);
        // So does panning far away.
        let built = d.drape.as_ref().map(|d| d.built);
        m.camera.pan_px(5_000.0, 0.0);
        assert!(d.prepare(&ctx, &mut m, key, 0, 800.0, 600.0, 1.0, 0.3));
        assert_ne!(d.drape.as_ref().map(|d| d.built), built);
    }
}
