//! Water on a map's terrain (#1915, `Terrain::water`): the polygons of a layer drawn flat at their
//! level in a tilted view, the lowest ground along each one's shore or a field's value, instead of
//! rising and falling with a DEM that is noisy or stepped over water (Copernicus GLO-30 is a
//! surface model).

use std::sync::{Arc, Mutex, Weak};

use rustgis_data::raster::Raster;
use rustgis_data::{Feature, Geometry};
use rustgis_geom::{Coord, Extent, MERCATOR_R, lonlat_to_merc};
use rustgis_map::Map;
use rustgis_map::view::TerrainEncoding;

use super::{Ground, Heights};

/// The most heights read along all the shores of a water layer for their levels: a longer shore
/// is read at wider steps (a national coastline at 30 m would take millions).
const MAX_SHORE_SAMPLES: usize = 1 << 22;

/// The most cells along each side of the grid that finds the lakes near a point.
const MAX_GRID: usize = 256;

/// The most references to its edges a polygon's strips hold, on average per edge.
const EDGES_PER_STRIP_REF: usize = 16;

/// How long a water read over elevation tiles with some along its shores missing (on their way,
/// or failed and waiting to be asked again) is used before it is read again.
const RETRY_MISSING: web_time::Duration = web_time::Duration::from_secs(1);

/// The strip of `n`, each `step` high from `y0`, that holds `y`.
fn strip(y: f64, y0: f64, step: f64, n: usize) -> usize {
    (((y - y0) / step).floor().max(0.0) as usize).min(n - 1)
}

/// The water bodies of a terrain's water layer, flat at their levels.
pub(super) struct Water {
    lakes: Vec<Lake>,
    /// The box round every lake, split into `cols` × `rows` cells, each listing the lakes whose
    /// boxes reach into it.
    extent: Extent,
    cols: usize,
    rows: usize,
    cells: Vec<Vec<u32>>,
    /// Polygons left on the ground: no height read along their shore and no level given.
    pub(super) unlevelled: usize,
    /// Polygons whose shore was read at wider steps than half a cell of the ground, to keep
    /// within [`MAX_SHORE_SAMPLES`]: their lowest shore may lie between two steps.
    pub(super) coarse: usize,
    /// Every height along the shores was there (over elevation tiles, some may not have come).
    complete: bool,
}

/// One polygon of the water layer (a part of a multipolygon is a lake of its own).
struct Lake {
    extent: Extent,
    /// Metres, before exaggeration.
    level: f64,
    edges: Slabs,
}

/// A polygon's edges, by horizontal strip of its box: whether a point is inside (even-odd, holes
/// out) is counted on the edges of the point's strip only.
struct Slabs {
    y0: f64,
    step: f64,
    strips: Vec<Vec<(Coord, Coord)>>,
}

impl Slabs {
    fn new(rings: &[Vec<Coord>], extent: &Extent) -> Slabs {
        let edges: Vec<(Coord, Coord)> = rings
            .iter()
            .flat_map(|r| r.iter().zip(r.iter().cycle().skip(1)).map(|(a, b)| (*a, *b)))
            .filter(|(a, b)| a.is_finite() && b.is_finite() && a.y != b.y)
            .collect();
        // Fewer strips when tall edges would be held in many of them: at most
        // [`EDGES_PER_STRIP_REF`] references an edge on average (one strip holds every edge once).
        let cap = edges.len().saturating_mul(EDGES_PER_STRIP_REF).max(4096);
        let mut n = (edges.len() / 4).clamp(1, 4096);
        let mut step = (extent.height() / n as f64).max(f64::MIN_POSITIVE);
        while n > 1
            && edges
                .iter()
                .map(|(a, b)| strip(a.y.max(b.y), extent.min_y, step, n) - strip(a.y.min(b.y), extent.min_y, step, n) + 1)
                .sum::<usize>()
                > cap
        {
            n /= 2;
            step = (extent.height() / n as f64).max(f64::MIN_POSITIVE);
        }
        let mut strips = vec![Vec::new(); n];
        for (a, b) in edges {
            for s in &mut strips[strip(a.y.min(b.y), extent.min_y, step, n)..=strip(a.y.max(b.y), extent.min_y, step, n)] {
                s.push((a, b));
            }
        }
        Slabs { y0: extent.min_y, step, strips }
    }

    #[cfg(test)]
    fn references(&self) -> usize {
        self.strips.iter().map(Vec::len).sum()
    }

    fn contains(&self, p: Coord) -> bool {
        let s = strip(p.y, self.y0, self.step, self.strips.len());
        let mut inside = false;
        for (a, b) in &self.strips[s] {
            if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
                inside = !inside;
            }
        }
        inside
    }
}

impl Water {
    /// The level (metres, before exaggeration) of the water at a Web Mercator point, when one of
    /// the lakes holds it.
    pub(super) fn level(&self, m: Coord) -> Option<f64> {
        if self.lakes.is_empty() || !self.extent.contains(m) {
            return None;
        }
        let cell = |v: f64, lo: f64, size: f64, n: usize| (((v - lo) / size * n as f64).floor().max(0.0) as usize).min(n - 1);
        let (c, r) = (
            cell(m.x, self.extent.min_x, self.extent.width().max(f64::MIN_POSITIVE), self.cols),
            cell(m.y, self.extent.min_y, self.extent.height().max(f64::MIN_POSITIVE), self.rows),
        );
        self.cells[r * self.cols + c]
            .iter()
            .map(|&i| &self.lakes[i as usize])
            .find(|l| l.extent.contains(m) && l.edges.contains(m))
            .map(|l| l.level)
    }

    /// The water of `polygons` (Web Mercator rings, and each one's level when given) over
    /// `ground`, reading at most `budget` heights along the shores of those without a level.
    fn build(polygons: Vec<(Vec<Vec<Coord>>, Option<f64>)>, ground: &Ground, budget: usize) -> Water {
        let perimeter: f64 =
            polygons.iter().filter(|(_, given)| given.is_none()).flat_map(|(rings, _)| rings).map(|r| ring_length(r)).sum();
        let mut lakes = Vec::new();
        let (mut unlevelled, mut coarse, mut complete) = (0, 0, true);
        let mut extent = Extent::EMPTY;
        // Half the ground's cell, or the shortest step that keeps every shore within the budget.
        let least_step = perimeter / budget.max(1) as f64;
        let half_cell = cell_size(ground).map(|c| c / 2.0);
        for (rings, given) in polygons {
            let mut e = Extent::EMPTY;
            rings.iter().flatten().for_each(|c| e.expand(*c));
            if e.is_empty() {
                continue;
            }
            let level = match given {
                Some(level) => Some(level),
                None => {
                    let shore = shore_level(&rings, ground, half_cell, least_step);
                    coarse += usize::from(shore.coarse);
                    complete &= !shore.missing;
                    shore.low
                }
            };
            let Some(level) = level else {
                unlevelled += 1;
                continue;
            };
            extent = extent.union(&e);
            lakes.push(Lake { edges: Slabs::new(&rings, &e), extent: e, level });
        }
        let side = ((lakes.len() as f64).sqrt().ceil() as usize * 2).clamp(1, MAX_GRID);
        let (cols, rows) = (side, side);
        let mut cells = vec![Vec::new(); cols * rows];
        if !extent.is_empty() {
            let (w, h) = (extent.width().max(f64::MIN_POSITIVE), extent.height().max(f64::MIN_POSITIVE));
            let cell = |v: f64, lo: f64, size: f64, n: usize| (((v - lo) / size * n as f64).floor().max(0.0) as usize).min(n - 1);
            for (i, l) in lakes.iter().enumerate() {
                for r in cell(l.extent.min_y, extent.min_y, h, rows)..=cell(l.extent.max_y, extent.min_y, h, rows) {
                    for c in cell(l.extent.min_x, extent.min_x, w, cols)..=cell(l.extent.max_x, extent.min_x, w, cols) {
                        cells[r * cols + c].push(i as u32);
                    }
                }
            }
        }
        Water { lakes, extent, cols, rows, cells, unlevelled, coarse, complete }
    }
}

/// The ground's cell in Web Mercator units: a DEM's at its middle (a cell's diagonal over √2),
/// elevation tiles' pixel at the zoom read. `None` when the DEM's middle can't be placed.
fn cell_size(ground: &Ground) -> Option<f64> {
    match &ground.heights {
        Heights::Raster { raster, .. } => {
            let (gt, t) = (&raster.transform, raster.transformer());
            let at = |px: f64, py: f64| {
                t.to_lonlat(Coord::new(gt.origin_x + px * gt.pixel_width, gt.origin_y + py * gt.pixel_height)).map(lonlat_to_merc)
            };
            let (cx, cy) = (f64::from(raster.width) / 2.0, f64::from(raster.height) / 2.0);
            let (a, b) = (at(cx, cy)?, at(cx + 1.0, cy + 1.0)?);
            let d = (b.x - a.x).hypot(b.y - a.y) / std::f64::consts::SQRT_2;
            (d.is_finite() && d > 0.0).then_some(d)
        }
        Heights::Tiles { zoom, .. } => Some(2.0 * std::f64::consts::PI * MERCATOR_R / (256.0 * f64::from(1u32 << (*zoom).min(30)))),
    }
}

/// What a polygon's shore read: its lowest height (metres, before exaggeration; `None` when no
/// height along it is known: off the DEM, tiles not read yet), whether it was read at wider
/// steps than half a cell, and whether a height along it was missing over elevation tiles.
struct Shore {
    low: Option<f64>,
    coarse: bool,
    missing: bool,
}

/// The shore of a polygon's rings, read every `half_cell` (two steps an edge when the ground's
/// cell isn't known), or every `least_step` when that is longer.
fn shore_level(rings: &[Vec<Coord>], ground: &Ground, half_cell: Option<f64>, least_step: f64) -> Shore {
    let tiles = matches!(ground.heights, Heights::Tiles { .. });
    let (mut low, mut coarse, mut missing) = (f64::INFINITY, false, false);
    for r in rings {
        for (a, b) in r.iter().zip(r.iter().cycle().skip(1)).take(r.len()) {
            let len = (b.x - a.x).hypot(b.y - a.y);
            if !len.is_finite() {
                continue;
            }
            let wanted = half_cell.map_or(2.0, |h| (len / h).ceil()).max(1.0);
            let allowed = if least_step > 0.0 { (len / least_step).floor().max(1.0) } else { wanted };
            let n = wanted.min(allowed).min(MAX_EDGE_SAMPLES);
            coarse |= n < wanted;
            for k in 0..n as usize {
                let t = k as f64 / n;
                match ground.raw_height(Coord::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)) {
                    Some(h) => low = low.min(h),
                    None => missing |= tiles,
                }
            }
        }
    }
    Shore { low: low.is_finite().then_some(low), coarse, missing }
}

/// The most heights read along one edge of a shore.
const MAX_EDGE_SAMPLES: f64 = 1e6;

fn ring_length(r: &[Coord]) -> f64 {
    r.iter().zip(r.iter().cycle().skip(1)).take(r.len()).map(|(a, b)| (b.x - a.x).hypot(b.y - a.y)).filter(|d| d.is_finite()).sum()
}

/// What a water layer's lakes and levels were read from: rebuilt when any of it changes.
#[derive(Clone)]
struct Key {
    rows: Weak<Vec<Feature>>,
    layer: String,
    level: Option<String>,
    heights: HeightsKey,
}

#[derive(Clone)]
enum HeightsKey {
    Raster(Weak<Raster>),
    Tiles { url: String, encoding: TerrainEncoding, zoom: u8 },
}

impl Key {
    fn same(&self, o: &Key) -> bool {
        self.rows.ptr_eq(&o.rows)
            && self.layer == o.layer
            && self.level == o.level
            && match (&self.heights, &o.heights) {
                (HeightsKey::Raster(a), HeightsKey::Raster(b)) => a.ptr_eq(b),
                (HeightsKey::Tiles { url: a, encoding: e, zoom: x }, HeightsKey::Tiles { url: b, encoding: f, zoom: y }) => {
                    a == b && e == f && x == y
                }
                _ => false,
            }
    }
}

/// The waters read last, kept while their layer, field and heights stay the same: a view drawn as
/// the map moves reads no shore again. One read over elevation tiles with some missing is read
/// again after [`RETRY_MISSING`], once tiles have come or a failed one may be asked again.
static LAST: Mutex<Vec<(Key, Arc<Water>, web_time::Instant)>> = Mutex::new(Vec::new());
/// How many waters [`LAST`] keeps (maps drawn side by side, a layout's frames).
const KEPT: usize = 4;

/// The water of `map`'s terrain over `ground` (its heights, before the water is set), when it
/// names a polygon layer there.
pub(super) fn of(map: &Map, ground: &Ground) -> Option<Arc<Water>> {
    let w = map.view.terrain.as_ref()?.water.as_ref()?;
    let layer = map.layer_by_name(&w.layer).filter(|l| l.geometry_type() == rustgis_data::GeometryType::Polygon)?;
    let key = Key {
        rows: layer.dataset.features.allocation_key(),
        layer: layer.name.clone(),
        level: w.level.clone(),
        heights: match &ground.heights {
            Heights::Raster { raster, .. } => HeightsKey::Raster(Arc::downgrade(raster)),
            Heights::Tiles { url, encoding, zoom, .. } => HeightsKey::Tiles { url: url.clone(), encoding: *encoding, zoom: *zoom },
        },
    };
    if let Ok(last) = LAST.lock()
        && let Some((_, water, _)) = last.iter().find(|(k, w, at)| k.same(&key) && (w.complete || at.elapsed() < RETRY_MISSING))
    {
        return Some(water.clone());
    }
    let field = w.level.as_deref().and_then(|f| layer.dataset.field_index(f));
    let mut polygons = Vec::new();
    for ft in layer.dataset.features.iter() {
        let Some(Geometry::Polygon(parts)) = &ft.geometry else { continue };
        let given = field.and_then(|i| ft.attrs.get(i)).and_then(rustgis_data::Value::as_f64).filter(|v| v.is_finite());
        for rings in parts {
            let rings: Vec<Vec<Coord>> = rings.iter().map(|r| r.iter().map(|c| lonlat_to_merc(*c)).collect()).collect();
            polygons.push((rings, given));
        }
    }
    let water = Arc::new(Water::build(polygons, ground, MAX_SHORE_SAMPLES));
    if let Ok(mut last) = LAST.lock() {
        last.retain(|(k, _, _)| k.rows.strong_count() > 0 && !k.same(&key));
        if last.len() >= KEPT {
            last.remove(0);
        }
        last.push((key, water.clone(), web_time::Instant::now()));
    }
    Some(water)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, y0: f64, side: f64) -> Vec<Coord> {
        vec![Coord::new(x0, y0), Coord::new(x0 + side, y0), Coord::new(x0 + side, y0 + side), Coord::new(x0, y0 + side)]
    }

    /// The test hill (a 3000 m peak at 0°, 0°, its DEM from −1° to 1°) with a water layer: a
    /// lake on its eastern slope with an island, a reservoir of a known level, and a pond off the
    /// DEM.
    fn hill_with_water(level: Option<&str>) -> (Map, rustgis_map::Camera) {
        use rustgis_data::{Dataset, Field, FieldType, GeometryType, Value};
        let mut m = crate::tests::hill_map();
        let mut d = Dataset::new("lagos", GeometryType::Polygon, vec![Field::new("nivel", FieldType::Double)]);
        let lake = vec![square(0.2, -0.2, 0.3), square(0.33, -0.02, 0.04)];
        d.push(Some(Geometry::Polygon(vec![lake])), vec![Value::Null]);
        d.push(Some(Geometry::Polygon(vec![vec![square(-0.5, -0.5, 0.2)]])), vec![Value::Float(1234.0)]);
        d.push(Some(Geometry::Polygon(vec![vec![square(5.0, 5.0, 0.2)]])), vec![Value::Null]);
        m.add_dataset(d);
        m.view.terrain = Some(rustgis_map::view::Terrain {
            source: rustgis_map::view::TerrainSource::Layer("dem".into()),
            exaggeration: 2.0,
            water: Some(rustgis_map::view::TerrainWater { layer: "lagos".into(), level: level.map(str::to_string) }),
        });
        let camera = rustgis_map::Camera { center: lonlat_to_merc(Coord::new(0.0, -0.6)), res: 600.0, bearing: 0.0, pitch: 65.0 };
        (m, camera)
    }

    #[test]
    fn water_lies_flat_at_its_shores_lowest_height() {
        let at = |lon, lat| lonlat_to_merc(Coord::new(lon, lat));
        let (mut m, camera) = hill_with_water(None);
        let g = Ground::of(&m, &camera).unwrap();
        // The shore's lowest ground is at the lake's corners farthest from the peak.
        let shore = g.raw_height(at(0.5, -0.2)).unwrap() * 2.0;
        for (lon, lat) in [(0.25, 0.0), (0.45, 0.05), (0.3, -0.15)] {
            let h = g.height(at(lon, lat)).unwrap();
            assert!((h - shore).abs() < 0.05 * shore, "({lon}, {lat}): {h} vs the shore's {shore}");
            assert!(g.raw_height(at(lon, lat)).unwrap() * 2.0 > h, "the DEM stands above the water there");
        }
        // The island and the land around stay the DEM's.
        for (lon, lat) in [(0.35, 0.0), (0.1, 0.0), (0.6, 0.0)] {
            assert_eq!(g.height(at(lon, lat)), g.raw_height(at(lon, lat)).map(|h| h * 2.0), "({lon}, {lat})");
        }
        // Without a level field the reservoir lies at its shore too; the pond off the DEM stays
        // on the ground, and is counted.
        assert_ne!(g.height(at(-0.4, -0.4)), Some(2468.0));
        assert_eq!(g.water_unlevelled(), 1);
        // With the field, the reservoir's level is its value; a row without one keeps its shore.
        let (with_field, _) = hill_with_water(Some("nivel"));
        let f = Ground::of(&with_field, &camera).unwrap();
        assert_eq!(f.height(at(-0.4, -0.4)), Some(2468.0));
        assert!((f.height(at(0.25, 0.0)).unwrap() - shore).abs() < 0.05 * shore);
        // An edited layer is read again: the lake moved off the slope leaves it on the ground.
        let l = m.layers.iter_mut().find(|l| l.name == "lagos").unwrap();
        l.dataset.features.clear();
        let g = Ground::of(&m, &camera).unwrap();
        assert_eq!(g.height(at(0.25, 0.0)), g.raw_height(at(0.25, 0.0)).map(|h| h * 2.0));
        // No water layer of that name: the ground as before.
        m.view.terrain.as_mut().unwrap().water.as_mut().unwrap().layer = "none".into();
        assert_eq!(Ground::of(&m, &camera).unwrap().water_unlevelled(), 0);
    }

    #[test]
    fn a_long_shore_is_read_at_the_grounds_cell_and_its_budget_said() {
        let at = |lon, lat| lonlat_to_merc(Coord::new(lon, lat));
        let (m, camera) = hill_with_water(None);
        let g = Ground::of(&m, &camera).unwrap();
        // A band of water far wider than the DEM (−1° to 1°): its shore crosses the DEM in a
        // short stretch, read every half cell there, so the band has a level.
        let band = vec![vec![at(-150.0, 0.1), at(150.0, 0.1), at(150.0, 0.2), at(-150.0, 0.2)]];
        let w = Water::build(vec![(band.clone(), None)], &g, MAX_SHORE_SAMPLES);
        assert_eq!((w.unlevelled, w.coarse), (0, 0));
        let low = w.level(at(0.0, 0.15)).unwrap();
        assert!(low < 10.0, "the band's shore is lowest at the DEM's edges: {low}");
        // A budget too small for half a cell along it: read at wider steps, and counted.
        let w = Water::build(vec![(band, None)], &g, 1000);
        assert_eq!(w.coarse, 1);
        // A level given reads no shore at all.
        let w = Water::build(vec![(vec![square(0.0, 0.0, 1.0)], Some(5.0))], &g, 1);
        assert_eq!((w.coarse, w.unlevelled), (0, 0));
    }

    #[test]
    fn tiles_of_another_encoding_are_other_heights() {
        let key = |encoding| Key {
            rows: Weak::new(),
            layer: "lagos".into(),
            level: None,
            heights: HeightsKey::Tiles { url: "https://e.com/{z}/{x}/{y}.png".into(), encoding, zoom: 9 },
        };
        assert!(key(TerrainEncoding::Terrarium).same(&key(TerrainEncoding::Terrarium)));
        assert!(!key(TerrainEncoding::Terrarium).same(&key(TerrainEncoding::Mapbox)));
    }

    #[test]
    fn tall_edges_are_held_in_fewer_strips() {
        // A comb of 4000 teeth, each edge as tall as the polygon: in 1000 strips every edge would
        // be held 1000 times.
        let mut ring: Vec<Coord> = (0..4000).map(|i| Coord::new(i as f64, if i % 2 == 0 { 0.0 } else { 100.0 })).collect();
        ring.push(Coord::new(3999.0, -10.0));
        ring.push(Coord::new(0.0, -10.0));
        let mut e = Extent::EMPTY;
        ring.iter().for_each(|c| e.expand(*c));
        let s = Slabs::new(std::slice::from_ref(&ring), &e);
        assert!(s.references() <= ring.len() * EDGES_PER_STRIP_REF, "{}", s.references());
        // Still right: under the teeth is inside, between two teeth's tips above the base is not.
        assert!(s.contains(Coord::new(10.5, -5.0)));
        assert!(!s.contains(Coord::new(10.5, 200.0)));
    }

    #[test]
    fn slabs_count_holes_out() {
        let rings = vec![square(0.0, 0.0, 10.0), square(4.0, 4.0, 2.0)];
        let mut e = Extent::EMPTY;
        rings.iter().flatten().for_each(|c| e.expand(*c));
        let s = Slabs::new(&rings, &e);
        assert!(s.contains(Coord::new(1.0, 1.0)));
        assert!(s.contains(Coord::new(9.0, 5.0)));
        assert!(!s.contains(Coord::new(5.0, 5.0)), "the island is land");
        assert!(!s.contains(Coord::new(11.0, 5.0)));
    }
}
