//! Snapping, which reads only the shapes whose kept box is near, finds what a search of every
//! shape finds.
use rustgis_engine::{
    Project, Session,
    cmd::design::snap_except,
    data::{Dataset, Geometry, GeometryType},
    geom::{Coord, merc_to_lonlat},
    map::design::Snapping,
};

const TOLERANCE: f64 = 10.0;

/// Metre-sized fixtures in the longitude/latitude coordinates of feature layers.
fn c(x: f64, y: f64) -> Coord {
    merc_to_lonlat(Coord::new(x, y))
}

/// A repeatable pseudo-random sequence in 0..1.
fn sequence(mut seed: u64) -> impl FnMut() -> f64 {
    move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn layer(next: &mut impl FnMut() -> f64, name: &str, n: usize) -> Dataset {
    let mut d = Dataset::new(name, GeometryType::Polygon, vec![]);
    for i in 0..n {
        let (x, y) = (next() * 400.0 - 5e6, next() * 400.0 - 2e6);
        let mut path = |n: usize| -> Vec<Coord> { (0..n).map(|_| c(x + next() * 60.0, y + next() * 60.0)).collect() };
        let square = |r: f64| vec![c(x - r, y - r), c(x + r, y - r), c(x + r, y + r), c(x - r, y + r), c(x - r, y - r)];
        let g = match i % 6 {
            0 => Some(Geometry::Point(c(x, y))),
            1 => Some(Geometry::MultiPoint(path(3))),
            2 => Some(Geometry::Line(vec![path(4), path(2)])),
            3 => Some(Geometry::Polygon(vec![vec![square(40.0), square(20.0)]])),
            4 => Some(Geometry::Polygon(vec![vec![path(4)], vec![path(3)]])),
            _ => None,
        };
        d.push(g, vec![]);
    }
    d
}

/// The kind and the map place a search of every shape but `skip` finds nearest to `m`: a vertex or
/// a line's end before an edge, the first of those as near.
fn nearest_of_every_shape(s: &mut Session, m: Coord, skip: Option<(u32, usize)>) -> Option<(Coord, &'static str)> {
    let (mut point, mut edge): (Option<(f64, Coord, &str)>, Option<(f64, Coord, &str)>) = (None, None);
    let mut consider = |at: Coord, kind: &'static str| {
        let d = (at.x - m.x).powi(2) + (at.y - m.y).powi(2);
        let slot = if kind == "edge" { &mut edge } else { &mut point };
        if d <= TOLERANCE * TOLERANCE && slot.is_none_or(|b| d < b.0) {
            *slot = Some((d, at, kind));
        }
    };
    for l in &mut s.map_mut().layers {
        let id = l.id;
        for (_, g) in l.projected().iter().enumerate().filter(|(row, _)| skip != Some((id, *row))) {
            let (points, paths): (&[Coord], Vec<&Vec<Coord>>) = match g {
                Some(Geometry::Point(p)) => (std::slice::from_ref(p), vec![]),
                Some(Geometry::MultiPoint(p)) => (p, vec![]),
                Some(Geometry::Line(parts)) => (&[], parts.iter().collect()),
                Some(Geometry::Polygon(polys)) => (&[], polys.iter().flatten().collect()),
                None => continue,
            };
            points.iter().for_each(|p| consider(*p, "vertex"));
            for path in paths {
                if matches!(g, Some(Geometry::Line(_))) {
                    [path.first(), path.last()].into_iter().flatten().for_each(|p| consider(*p, "end"));
                }
                path.iter().for_each(|p| consider(*p, "vertex"));
                for w in path.windows(2) {
                    let (dx, dy) = (w[1].x - w[0].x, w[1].y - w[0].y);
                    let len2 = dx * dx + dy * dy;
                    let t = if len2 > 0.0 { (((m.x - w[0].x) * dx + (m.y - w[0].y) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                    consider(Coord::new(w[0].x + t * dx, w[0].y + t * dy), "edge");
                }
            }
        }
    }
    point.or(edge).map(|(_, at, kind)| (at, kind))
}

fn check(s: &mut Session, next: &mut impl FnMut() -> f64, skip: Option<(u32, usize)>) {
    let mut kinds = std::collections::BTreeMap::new();
    for _ in 0..600 {
        let at = c(next() * 500.0 - 5e6 - 20.0, next() * 500.0 - 2e6 - 20.0);
        let m = s.map().to_map(at).unwrap();
        let expected = nearest_of_every_shape(s, m, skip);
        let got = snap_except(s, at, skip);
        assert_eq!(got.map(|g| g.1), expected.map(|e| e.1), "{at:?}");
        *kinds.entry(got.map(|g| g.1)).or_insert(0) += 1;
        if let (Some((got, _)), Some((expected, _))) = (got, expected) {
            let got = s.map().to_map(got).unwrap();
            assert!((got.x - expected.x).abs() < 1e-6 && (got.y - expected.y).abs() < 1e-6, "{got:?} != {expected:?}");
        }
    }
    // Every kind, and nothing near too.
    assert_eq!(kinds.len(), 4, "{kinds:?}");
}

#[test]
fn snapping_finds_what_a_search_of_every_shape_finds() {
    let mut next = sequence(0x9E37_79B9_7F4A_7C15);
    let mut s = Session::new(Project::new("Snap boxes"));
    s.map_mut().layers.clear();
    let id = s.map_mut().add_dataset(layer(&mut next, "A", 48));
    s.map_mut().add_dataset(layer(&mut next, "B", 48));
    s.map_mut().camera.res = 1.0;
    s.map_mut().editing.snapping = Snapping { enabled: true, tolerance_px: TOLERANCE, vertex: true, edge: true, end: true };
    check(&mut s, &mut next, None);
    // All but one shape.
    check(&mut s, &mut next, Some((id, 3)));
    // Shapes moved, added and removed: their boxes are those of the new ones.
    let l = s.map_mut().layer_mut(id).unwrap();
    let far = c(-5e6 + 900.0, -2e6);
    l.dataset.features.set_geometry(0, Some(Geometry::Point(far)));
    l.dataset.features.set_geometry(3, None);
    l.dataset.features.set_geometry(5, Some(Geometry::Line(vec![vec![c(-5e6, -2e6), c(-5e6 + 400.0, -2e6 + 400.0)]])));
    l.touch_geometry();
    assert_eq!(snap_except(&mut s, c(-5e6 + 905.0, -2e6), None), Some((far, "vertex")));
    check(&mut s, &mut next, None);
}
