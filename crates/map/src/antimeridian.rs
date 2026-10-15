//! Cutting geometry along the edge of a world projection: the meridian opposite its central one
//! (`lon_0 ± 180°`). A feature that crosses it is split, and the part past it moves a whole turn
//! to the other side of the map, so it isn't drawn as a band across the whole map (#209).
//! Edges stay straight in longitude/latitude, as everywhere else in the map.

use rustgis_data::Geometry;
use rustgis_geom::Coord;

/// How far inside the edge cut points are kept, in degrees (about a centimetre on the ground), so
/// each piece projects onto its own side of the map.
const EDGE: f64 = 1e-7;

/// Clipped rings smaller than this (square degrees) are slivers along the cut and are dropped.
const SLIVER: f64 = 1e-12;

/// `g` (longitude/latitude) cut along the edge of a world projection centred on `lon0`, with every
/// piece moved by whole turns into `lon0 - 180 ..= lon0 + 180`.
pub fn cut(g: &Geometry, lon0: f64) -> Geometry {
    let lo = lon0 - 180.0;
    match g {
        Geometry::Point(c) => Geometry::Point(fold(*c, lo)),
        Geometry::MultiPoint(v) => Geometry::MultiPoint(v.iter().map(|c| fold(*c, lo)).collect()),
        Geometry::Line(parts) => Geometry::Line(parts.iter().flat_map(|p| cut_line(p, lo)).collect()),
        Geometry::Polygon(polys) => Geometry::Polygon(polys.iter().flat_map(|p| cut_polygon(p, lo)).collect()),
    }
}

/// The turns (360° windows from `lo`) that a run of points covers, or `None` when it can't be cut
/// (a missing coordinate, or longitudes no data has).
fn span(pts: &[Coord], lo: f64) -> Option<(i64, i64)> {
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for c in pts {
        if !c.x.is_finite() {
            return None;
        }
        min = min.min(c.x);
        max = max.max(c.x);
    }
    if !(min.is_finite() && max - min <= 1080.0) {
        return None;
    }
    let k0 = ((min - lo) / 360.0).floor() as i64;
    let k1 = (((max - lo) / 360.0).ceil() as i64 - 1).max(k0);
    Some((k0, k1))
}

/// A point of window `k`, moved into the projection's own turn and kept off its edge.
fn shift(c: Coord, lo: f64, k: i64) -> Coord {
    Coord::new((c.x - 360.0 * k as f64).clamp(lo + EDGE, lo + 360.0 - EDGE), c.y)
}

fn fold(c: Coord, lo: f64) -> Coord {
    if !c.x.is_finite() {
        return c;
    }
    shift(c, lo, ((c.x - lo) / 360.0).floor() as i64)
}

fn cut_line(part: &[Coord], lo: f64) -> Vec<Vec<Coord>> {
    let Some((k0, k1)) = span(part, lo) else { return vec![part.to_vec()] };
    if k0 == k1 {
        return vec![part.iter().map(|c| shift(*c, lo, k0)).collect()];
    }
    let mut out = Vec::new();
    for k in k0..=k1 {
        let (a, b) = (lo + 360.0 * k as f64, lo + 360.0 * (k + 1) as f64);
        let mut piece: Vec<Coord> = Vec::new();
        for w in part.windows(2) {
            match clip_segment(w[0], w[1], a, b) {
                Some((p, q)) => {
                    if piece.last() != Some(&p) {
                        keep_piece(&mut out, std::mem::take(&mut piece), lo, k);
                        piece.push(p);
                    }
                    piece.push(q);
                }
                None => keep_piece(&mut out, std::mem::take(&mut piece), lo, k),
            }
        }
        keep_piece(&mut out, piece, lo, k);
    }
    out
}

fn keep_piece(out: &mut Vec<Vec<Coord>>, piece: Vec<Coord>, lo: f64, k: i64) {
    if piece.windows(2).any(|w| w[0] != w[1]) {
        out.push(piece.into_iter().map(|c| shift(c, lo, k)).collect());
    }
}

/// The part of segment `p`–`q` with `a <= x <= b` (Liang–Barsky on x), `None` when it has no length.
fn clip_segment(p: Coord, q: Coord, a: f64, b: f64) -> Option<(Coord, Coord)> {
    let dx = q.x - p.x;
    if dx == 0.0 {
        return (a <= p.x && p.x <= b).then_some((p, q));
    }
    let (ta, tb) = ((a - p.x) / dx, (b - p.x) / dx);
    let (t0, t1) = (ta.min(tb).max(0.0), ta.max(tb).min(1.0));
    if t0 >= t1 {
        return None;
    }
    // Ends inside the window are kept exactly, so the next segment continues the same piece.
    let at = |t: f64| {
        if t <= 0.0 {
            p
        } else if t >= 1.0 {
            q
        } else {
            Coord::new((p.x + dx * t).clamp(a, b), p.y + (q.y - p.y) * t)
        }
    };
    Some((at(t0), at(t1)))
}

fn cut_polygon(rings: &[Vec<Coord>], lo: f64) -> Vec<Vec<Vec<Coord>>> {
    let Some(outer) = rings.first() else { return Vec::new() };
    let Some((k0, k1)) = span(outer, lo) else { return vec![rings.to_vec()] };
    if k0 == k1 {
        return vec![rings.iter().map(|r| r.iter().map(|c| shift(*c, lo, k0)).collect()).collect()];
    }
    (k0..=k1)
        .filter_map(|k| {
            let (a, b) = (lo + 360.0 * k as f64, lo + 360.0 * (k + 1) as f64);
            let mut kept = Vec::with_capacity(rings.len());
            for (i, r) in rings.iter().enumerate() {
                let c = clip_ring(r, a, b);
                if area(&c).abs() > SLIVER {
                    kept.push(c.into_iter().map(|c| shift(c, lo, k)).collect());
                } else if i == 0 {
                    return None;
                }
            }
            Some(kept)
        })
        .collect()
}

/// The part of a ring with `a <= x <= b` (Sutherland–Hodgman against both meridians). A ring
/// that leaves the window and comes back stays one ring, joined along the cut by edges that
/// enclose nothing.
fn clip_ring(r: &[Coord], a: f64, b: f64) -> Vec<Coord> {
    let closed = r.len() > 1 && r.first() == r.last();
    let open = if closed { &r[..r.len() - 1] } else { r };
    let east = clip_half(open, |c| c.x >= a, a);
    let mut out = clip_half(&east, |c| c.x <= b, b);
    if closed && let Some(&first) = out.first() {
        out.push(first);
    }
    out
}

fn clip_half(pts: &[Coord], inside: impl Fn(Coord) -> bool, x: f64) -> Vec<Coord> {
    let mut out = Vec::with_capacity(pts.len() + 4);
    let Some(&last) = pts.last() else { return out };
    let cross = |p: Coord, q: Coord| Coord::new(x, p.y + (q.y - p.y) * (x - p.x) / (q.x - p.x));
    let mut prev = last;
    for &cur in pts {
        match (inside(prev), inside(cur)) {
            (true, true) => out.push(cur),
            (true, false) => out.push(cross(prev, cur)),
            (false, true) => {
                out.push(cross(prev, cur));
                out.push(cur);
            }
            (false, false) => {}
        }
        prev = cur;
    }
    out
}

/// Signed shoelace area.
fn area(r: &[Coord]) -> f64 {
    let Some(&last) = r.last() else { return 0.0 };
    let mut prev = last;
    let mut s = 0.0;
    for &c in r {
        s += prev.x * c.y - c.x * prev.y;
        prev = c;
    }
    s / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(x: f64, y: f64) -> Coord {
        Coord::new(x, y)
    }

    fn xs(g: &Geometry) -> (f64, f64) {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        g.for_each_coord(|c| {
            lo = lo.min(c.x);
            hi = hi.max(c.x);
        });
        (lo, hi)
    }

    #[test]
    fn a_polygon_across_the_edge_is_split_onto_both_sides() {
        // Central meridian 90°W: the edge is 90°E. A box from 60°E to 120°E crosses it.
        let square = Geometry::Polygon(vec![vec![vec![c(60.0, 40.0), c(120.0, 40.0), c(120.0, 60.0), c(60.0, 60.0), c(60.0, 40.0)]]]);
        let Geometry::Polygon(polys) = cut(&square, -90.0) else { panic!("a polygon") };
        assert_eq!(polys.len(), 2, "{polys:?}");
        let mut areas: Vec<f64> = polys.iter().map(|p| area(&p[0]).abs()).collect();
        areas.sort_by(f64::total_cmp);
        assert!((areas[0] - 600.0).abs() < 1e-3 && (areas[1] - 600.0).abs() < 1e-3, "{areas:?}");
        for p in &polys {
            let g = Geometry::Polygon(vec![p.clone()]);
            let (lo, hi) = xs(&g);
            assert!(lo >= -270.0 && hi <= 90.0, "inside the projection's turn: {lo}..{hi}");
        }
        // The eastern half moved a turn west, onto the left of the map.
        assert!(polys.iter().any(|p| p[0].iter().all(|c| c.x <= -239.0)));
    }

    #[test]
    fn holes_follow_their_piece() {
        let outer = vec![c(60.0, 0.0), c(120.0, 0.0), c(120.0, 30.0), c(60.0, 30.0), c(60.0, 0.0)];
        let hole = vec![c(100.0, 10.0), c(100.0, 20.0), c(110.0, 20.0), c(110.0, 10.0), c(100.0, 10.0)];
        let Geometry::Polygon(polys) = cut(&Geometry::Polygon(vec![vec![outer, hole]]), -90.0) else { panic!("a polygon") };
        let with_hole: Vec<_> = polys.iter().filter(|p| p.len() == 2).collect();
        assert_eq!(with_hole.len(), 1, "the hole goes with the eastern piece only: {polys:?}");
        assert!(with_hole[0][1].iter().all(|c| (-260.0..=-250.0).contains(&c.x)));
    }

    #[test]
    fn a_line_across_the_edge_becomes_two_parts() {
        let river = Geometry::Line(vec![vec![c(80.0, 50.0), c(100.0, 52.0)]]);
        let Geometry::Line(parts) = cut(&river, -90.0) else { panic!("a line") };
        assert_eq!(parts.len(), 2, "{parts:?}");
        assert!((parts[0][1].x - 90.0).abs() < 1e-6 && parts[0][1].x < 90.0, "west part ends just inside the edge");
        assert!((parts[1][0].x + 270.0).abs() < 1e-6 && parts[1][0].x > -270.0, "east part starts just inside the other edge");
        assert!((parts[0][1].y - 51.0).abs() < 1e-9 && (parts[1][0].y - 51.0).abs() < 1e-9);
    }

    #[test]
    fn whole_world_shapes_survive() {
        // A world box and an Antarctica-like ring along the bottom: under a 0° projection
        // nothing changes, and under 90°W each is cut in two that together cover the same area.
        let world =
            Geometry::Polygon(vec![vec![vec![c(-180.0, -60.0), c(180.0, -60.0), c(180.0, 60.0), c(-180.0, 60.0), c(-180.0, -60.0)]]]);
        let ant =
            Geometry::Polygon(vec![vec![vec![c(-180.0, -70.0), c(180.0, -70.0), c(180.0, -90.0), c(-180.0, -90.0), c(-180.0, -70.0)]]]);
        for (g, want) in [(&world, 360.0 * 120.0), (&ant, 360.0 * 20.0)] {
            for lon0 in [0.0, -90.0, -54.0, 150.0] {
                let Geometry::Polygon(polys) = cut(g, lon0) else { panic!("a polygon") };
                let total: f64 = polys.iter().map(|p| area(&p[0]).abs()).sum();
                assert!((total - want).abs() < 1e-3, "lon0 {lon0}: {total} vs {want}");
                assert_eq!(polys.len(), if lon0 == 0.0 { 1 } else { 2 }, "lon0 {lon0}");
            }
        }
    }

    #[test]
    fn data_already_inside_is_left_alone() {
        let line = Geometry::Line(vec![vec![c(-170.0, 0.0), c(-100.0, 10.0), c(80.0, -5.0)]]);
        assert_eq!(cut(&line, -90.0), line);
        assert_eq!(cut(&Geometry::Point(c(120.0, 1.0)), -90.0), Geometry::Point(c(-240.0, 1.0)));
        let odd = Geometry::Line(vec![vec![c(f64::NAN, 0.0), c(100.0, 0.0)]]);
        assert_eq!(format!("{:?}", cut(&odd, -90.0)), format!("{odd:?}"), "left for the projection to drop");
    }
}
