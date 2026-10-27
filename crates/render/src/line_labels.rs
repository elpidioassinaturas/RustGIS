//! Labels along lines, in screen points: straight and turned to the line ([`parallel`]), or each
//! character turned with it where it bends ([`curved`]), at the line's middle and kept upright,
//! as QGIS's Parallel and Curved line placements put text on the line.

/// The most a curved label's text turns from one character to the next (radians): QGIS's
/// default limit, 25°. Past it, where a line bends sharply, the label is left out.
pub(super) const MAX_TURN: f32 = 0.436;

/// The most characters a label curves along a line: a longer one is placed straight, so a long
/// attribute value on many lines doesn't lay out, and draw, character by character.
pub(super) const MAX_CURVED_CHARS: usize = 200;

/// A line in screen points, with how far along it each vertex is.
struct Path {
    pts: Vec<(f32, f32)>,
    along: Vec<f32>,
}

impl Path {
    /// The finite vertices of `line` (repeats dropped), reversed when `reversed`; `None` for
    /// one with fewer than two.
    fn new(line: &[(f32, f32)], reversed: bool) -> Option<Path> {
        let mut pts: Vec<(f32, f32)> = Vec::with_capacity(line.len());
        for p in line.iter().copied().filter(|p| p.0.is_finite() && p.1.is_finite()) {
            if pts.last() != Some(&p) {
                pts.push(p);
            }
        }
        if reversed {
            pts.reverse();
        }
        let mut along = Vec::with_capacity(pts.len());
        let mut d = 0.0f32;
        for (i, p) in pts.iter().enumerate() {
            if let Some(q) = i.checked_sub(1).and_then(|j| pts.get(j)) {
                d += (p.0 - q.0).hypot(p.1 - q.1);
            }
            along.push(d);
        }
        (pts.len() >= 2 && d.is_finite() && d > 0.0).then_some(Path { pts, along })
    }

    fn length(&self) -> f32 {
        self.along.last().copied().unwrap_or(0.0)
    }

    /// The point `d` along the line; before its start or past its end, on the first or last
    /// segment carried on.
    fn at(&self, d: f32) -> Option<(f32, f32)> {
        let n = self.pts.len();
        // The segment holding `d`: the first or last one for a place off the line's ends.
        let i = self.along.partition_point(|a| *a < d).clamp(1, n - 1);
        let (a, b) = (self.pts.get(i - 1)?, self.pts.get(i)?);
        let (da, db) = (self.along.get(i - 1)?, self.along.get(i)?);
        let t = if db > da { (d - da) / (db - da) } else { 0.0 };
        Some((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t))
    }
}

/// The stretches of a line in screen points that are in a view `w` × `h` points: cut where it
/// crosses the view's edges and at points out of sight (`None`, or of no number), each with two
/// points or more.
pub(super) fn in_view(line: impl Iterator<Item = Option<(f32, f32)>>, w: f32, h: f32) -> Vec<Vec<(f32, f32)>> {
    let mut runs: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut run: Vec<(f32, f32)> = Vec::new();
    let mut prev: Option<(f32, f32)> = None;
    let end = |run: &mut Vec<(f32, f32)>, runs: &mut Vec<Vec<(f32, f32)>>| {
        let run = std::mem::take(run);
        if run.len() >= 2 {
            runs.push(run);
        }
    };
    for p in line {
        let p = p.filter(|p| p.0.is_finite() && p.1.is_finite());
        match (prev, p) {
            (Some(a), Some(b)) => match clip(a, b, w, h) {
                Some((a, b, cut_end)) => {
                    if run.last() != Some(&a) {
                        end(&mut run, &mut runs);
                        run.push(a);
                    }
                    run.push(b);
                    // Left the view: what comes back in is another stretch.
                    if cut_end {
                        end(&mut run, &mut runs);
                    }
                }
                None => end(&mut run, &mut runs),
            },
            _ => end(&mut run, &mut runs),
        }
        prev = p;
    }
    end(&mut run, &mut runs);
    runs
}

/// The part of the segment `a`–`b` inside a view `w` × `h` points (Liang–Barsky), and whether it
/// stops short of `b`; `None` when none of it is.
fn clip(a: (f32, f32), b: (f32, f32), w: f32, h: f32) -> Option<((f32, f32), (f32, f32), bool)> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in [(-dx, a.0), (dx, w - a.0), (-dy, a.1), (dy, h - a.1)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
        }
    }
    if t0.is_nan() || t1.is_nan() || t0 > t1 {
        return None;
    }
    let at = |t: f32| (a.0 + dx * t, a.1 + dy * t);
    Some((if t0 > 0.0 { at(t0) } else { a }, if t1 < 1.0 { at(t1) } else { b }, t1 < 1.0))
}

/// The angle (radians, clockwise) from `a` to `b`.
fn angle(a: (f32, f32), b: (f32, f32)) -> f32 {
    (b.1 - a.1).atan2(b.0 - a.0)
}

/// The line as text `len` points long reads along it, left to right: reversed when its middle
/// stretch runs leftward, so the text is never upside down. A vertical stretch reads downward,
/// as street labels turned with the map do ([`rustgis_tiles::symbols::upright`]), whichever way
/// the line was drawn. `None` when the line, with `overrun` past each end, is shorter than the
/// text.
fn upright(line: &[(f32, f32)], len: f32, overrun: f32) -> Option<Path> {
    let path = Path::new(line, false)?;
    let total = path.length();
    if !(len.is_finite() && len > 0.0) || len > total + 2.0 * overrun.max(0.0) {
        return None;
    }
    let (a, b) = (path.at((total - len) / 2.0)?, path.at((total + len) / 2.0)?);
    let a_to_b = angle(a, b);
    // Turned half round to stand upright: read from the other end.
    let flipped = (rustgis_tiles::symbols::upright(a_to_b) - a_to_b).abs() > 1.0;
    if flipped { Path::new(line, true) } else { Some(path) }
}

/// Text `len` points long straight along `line` at its middle: its centre and angle (radians,
/// clockwise, upright). `None` when the line (with `overrun` past each end) is shorter.
pub(super) fn parallel(line: &[(f32, f32)], len: f32, overrun: f32) -> Option<(f32, f32, f32)> {
    let path = upright(line, len, overrun)?;
    let total = path.length();
    let (a, b) = (path.at((total - len) / 2.0)?, path.at((total + len) / 2.0)?);
    let a_to_b = angle(a, b);
    a_to_b.is_finite().then_some(((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0, a_to_b))
}

/// Characters `advances` wide, `gap` apart (letter spacing; both in the label's units, points ×
/// 1.33, as its text's size), in turn along `line`
/// at its middle: the label's centre, and each character's centre relative to it and angle
/// (radians, clockwise, upright). `None` when the line (with `overrun` past each end) is shorter than the text, or turns more than
/// [`MAX_TURN`] from one character to the next.
pub(super) fn curved(line: &[(f32, f32)], advances: &[f32], gap: f32, overrun: f32) -> Option<((f32, f32), Vec<(f32, f32, f32)>)> {
    // Each step from a character to the next: its advance and the letter spacing (never back).
    let step = |adv: f32| (adv + gap).max(0.0);
    let len: f32 = advances.iter().map(|a| step(*a)).sum::<f32>() - advances.last().map_or(0.0, |a| step(*a) - a);
    let path = upright(line, len, overrun)?;
    let total = path.length();
    let centre = path.at(total / 2.0)?;
    let mut d = (total - len) / 2.0;
    let mut out = Vec::with_capacity(advances.len());
    let mut last: Option<f32> = None;
    for adv in advances {
        let mid = d + adv / 2.0;
        // A character's angle: across its own width, so it sits on the bend rather than a vertex.
        let (a, b) = (path.at(mid - adv.max(0.5) / 2.0)?, path.at(mid + adv.max(0.5) / 2.0)?);
        let (p, turn) = (path.at(mid)?, angle(a, b));
        if !turn.is_finite() {
            return None;
        }
        if let Some(prev) = last {
            let mut step = turn - prev;
            // Wrapped to within half a turn either way.
            step = (step + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            if step.abs() > MAX_TURN {
                return None;
            }
        }
        last = Some(turn);
        out.push((p.0 - centre.0, p.1 - centre.1, turn));
        d += step(*adv);
    }
    Some((centre, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_text_sits_at_the_middle_turned_to_the_line_and_upright() {
        // Along a line rising to the right at 45° (screen y down: up-right is −45°).
        let (x, y, a) = parallel(&[(0.0, 100.0), (100.0, 0.0)], 40.0, 0.0).unwrap();
        assert!((x - 50.0).abs() < 1e-3 && (y - 50.0).abs() < 1e-3);
        assert!((a + std::f32::consts::FRAC_PI_4).abs() < 1e-4, "{a}");
        // The same line drawn right to left reads the same way.
        let (_, _, b) = parallel(&[(100.0, 0.0), (0.0, 100.0)], 40.0, 0.0).unwrap();
        assert!((a - b).abs() < 1e-4, "{a} {b}");
        // Too short for the text, unless it may run past the ends.
        assert!(parallel(&[(0.0, 0.0), (30.0, 0.0)], 40.0, 0.0).is_none());
        assert!(parallel(&[(0.0, 0.0), (30.0, 0.0)], 40.0, 5.0).is_some());
        assert!(parallel(&[(0.0, 0.0), (0.0, 0.0)], 1.0, 5.0).is_none(), "no length");
    }

    #[test]
    fn a_vertical_line_reads_downward_whichever_way_it_was_drawn() {
        // Screen y down: downward is +90°. Drawn up or down, exactly vertical or a hair off it.
        let down = std::f32::consts::FRAC_PI_2;
        for line in
            [[(50.0, 0.0), (50.0, 100.0)], [(50.0, 100.0), (50.0, 0.0)], [(50.0, 100.0), (50.001, 0.0)], [(50.001, 100.0), (50.0, 0.0)]]
        {
            let (_, _, a) = parallel(&line, 40.0, 0.0).unwrap();
            assert!((a - down).abs() < 1e-3, "{line:?}: {a}");
            let (_, glyphs) = curved(&line, &[10.0; 4], 0.0, 0.0).unwrap();
            assert!(glyphs.iter().all(|g| (g.2 - down).abs() < 1e-3), "{line:?}: {glyphs:?}");
            assert!(glyphs.windows(2).all(|w| w[1].1 > w[0].1), "first character at the top: {glyphs:?}");
        }
    }

    #[test]
    fn letter_spacing_sets_curved_characters_apart_and_needs_more_line() {
        let line = [(0.0, 50.0), (100.0, 50.0)];
        let (_, tight) = curved(&line, &[10.0; 3], 0.0, 0.0).unwrap();
        let (_, spaced) = curved(&line, &[10.0; 3], 5.0, 0.0).unwrap();
        assert!((tight[1].0 - tight[0].0 - 10.0).abs() < 1e-3 && (spaced[1].0 - spaced[0].0 - 15.0).abs() < 1e-3);
        assert!(spaced[1].0.abs() < 1e-3, "still centred: {spaced:?}");
        // 3 × 10 and two gaps of 40: longer than the line.
        assert!(curved(&line, &[10.0; 3], 40.0, 0.0).is_none());
        // Spacing tighter than a letter doesn't step back.
        let (_, back) = curved(&line, &[10.0; 3], -20.0, 0.0).unwrap();
        assert!(back.windows(2).all(|w| w[1].0 >= w[0].0), "{back:?}");
    }

    #[test]
    fn a_line_in_view_is_cut_at_its_edges_and_where_it_is_out_of_sight() {
        // Across the view and out, back in and out again: two stretches, cut at the edges.
        let line = [(-50.0, 50.0), (50.0, 50.0), (50.0, 150.0), (80.0, 150.0), (80.0, 50.0), (150.0, 50.0)];
        let runs = in_view(line.iter().copied().map(Some), 100.0, 100.0);
        assert_eq!(runs, [vec![(0.0, 50.0), (50.0, 50.0), (50.0, 100.0)], vec![(80.0, 100.0), (80.0, 50.0), (100.0, 50.0)]]);
        // A point out of sight splits the line; a stretch of one point is none.
        let runs =
            in_view([Some((10.0, 10.0)), Some((20.0, 10.0)), None, Some((30.0, 10.0)), Some((f32::NAN, 0.0))].into_iter(), 100.0, 100.0);
        assert_eq!(runs, [vec![(10.0, 10.0), (20.0, 10.0)]]);
        assert!(in_view([Some((-10.0, -10.0)), Some((-20.0, 200.0))].into_iter(), 100.0, 100.0).is_empty());
    }

    #[test]
    fn curved_text_follows_a_gentle_bend_and_leaves_a_sharp_one() {
        // A shallow arc: each character a little more turned than the one before.
        let arc: Vec<(f32, f32)> = (0..=40)
            .map(|i| {
                let t = (i as f32 / 40.0 - 0.5) * 1.2;
                (200.0 * t.sin() + 200.0, 200.0 - 200.0 * t.cos())
            })
            .collect();
        let (_, glyphs) = curved(&arc, &[10.0; 8], 0.0, 0.0).unwrap();
        assert_eq!(glyphs.len(), 8);
        assert!(glyphs.windows(2).all(|w| w[1].2 > w[0].2), "turning steadily clockwise");
        assert!(glyphs.windows(2).all(|w| w[1].0 > w[0].0), "left to right");
        // A right angle at the middle: too sharp for the text.
        assert!(curved(&[(0.0, 100.0), (100.0, 100.0), (100.0, 0.0)], &[10.0; 6], 0.0, 0.0).is_none());
    }
}
