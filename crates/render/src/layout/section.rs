//! A map frame's section lines (#2863): where each runs across its frame, what isn't drawn and
//! why, and the lines with their names.

use super::*;

/// A section line's ends in its frame's points, from its left or top.
pub type SectionEnds = [(f64, f64); 2];

/// Where `line` is drawn on a frame `w` × `h` points showing `map` through `cam`, within its
/// `shape`: its ends, the outermost of where it meets the shape's outline (#2863). It is straight
/// on the page whatever the frame's projection or bearing. On a tilted frame or the globe it runs
/// through the place as the frame's grid puts it there (on the ground, without its relief), only as
/// far as there is ground along it, to the horizon or the globe's limb; what it crosses there is no
/// straight section of the ground. `Err` says why it isn't drawn: its place is outside the frame,
/// or one the frame's map can't show (behind the globe or the eye, or outside its projection).
pub fn section_ends(
    map: &Map,
    cam: &rustgis_map::Camera,
    (w, h): (f64, f64),
    shape: &FrameShape,
    line: SectionLine,
) -> Result<SectionEnds, &'static str> {
    const NOT_SHOWN: &str = "its place isn't one the frame's map shows";
    let globe = map.projection.is_globe().then(|| rustgis_map::globe::Globe::new(cam, w, h));
    let mut at = line.at();
    // On a map whose world repeats, the copy of the place nearest the frame's middle.
    if let Some(mid) = map.to_lonlat(cam.center).filter(|c| c.x.is_finite()) {
        at.x += 360.0 * ((mid.x - at.x) / 360.0).round();
    }
    let (px, py) = match &globe {
        Some(g) => g.forward(at),
        None => map.to_map(at).and_then(|m| cam.try_to_screen(m, w, h)),
    }
    .filter(|(x, y)| x.is_finite() && y.is_finite())
    .ok_or(NOT_SHOWN)?;
    // Along the line and across it: a vertical line is worked out with the axes swapped.
    let vertical = matches!(line, SectionLine::Vertical(_));
    let swap = |(x, y): (f64, f64)| if vertical { (y, x) } else { (x, y) };
    let (along, across) = swap((px, py));
    // Where the line meets the shape's outline, and which way the outline crosses it there: the
    // place is inside as the shape is filled, by its winding number. A circle is as wide as the
    // shorter side ([`shape_path`]).
    let (bw, bh) = if *shape == FrameShape::Circle { (w.min(h), w.min(h)) } else { (w, h) };
    let outline: Vec<(f64, f64)> =
        shape.outline().into_iter().map(|(u, v)| swap(((w - bw) / 2.0 + u * bw, (h - bh) / 2.0 + v * bh))).collect();
    let mut meets: Vec<(f64, i32)> = Vec::new();
    for (a, b) in outline.iter().zip(outline.iter().cycle().skip(1)) {
        if (a.1 <= across) != (b.1 <= across) {
            meets.push((a.0 + (across - a.1) / (b.1 - a.1) * (b.0 - a.0), if a.1 <= across { 1 } else { -1 }));
        }
    }
    let inside = meets.iter().filter(|m| m.0 <= along).map(|m| m.1).sum::<i32>() != 0;
    let (from, to) = meets.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), m| (lo.min(m.0), hi.max(m.0)));
    if !inside || from >= to {
        return Err("its place is outside the frame");
    }
    let ground = |along: f64| {
        let (x, y) = swap((along, across));
        match &globe {
            Some(g) => g.inverse(x, y).is_some(),
            None => cam.pitch.abs() <= 1e-9 || cam.try_to_map(x, y, w, h).is_some(),
        }
    };
    // The ground along the line is in one piece, which holds the place: 24 halvings find its end.
    let reach = |end: f64| {
        let (mut on, mut off) = (along, end);
        if ground(end) {
            return end;
        }
        for _ in 0..24 {
            let mid = (on + off) / 2.0;
            *(if ground(mid) { &mut on } else { &mut off }) = mid;
        }
        on
    };
    Ok([swap((reach(from), across)), swap((reach(to), across))])
}

/// A section line its frame doesn't draw ([`section_ends`]).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SectionGap {
    pub frame: u32,
    pub frame_name: String,
    pub section: String,
    pub why: &'static str,
}

/// The section lines of `layout`'s visible frames they don't draw, as the page is now: those
/// [`section_ends`] gives no ends for, and every one of a frame whose map is missing.
pub fn sections_not_drawn(layout: &Layout, maps: &[Map]) -> Vec<SectionGap> {
    let mut gaps = Vec::new();
    for e in layout.elements.iter().filter(|e| e.visible) {
        let ElementKind::MapFrame { map, center, scale, bearing, pitch, shape, sections, .. } = &e.kind else { continue };
        let shown = map_index(maps, map).and_then(|i| maps.get(i)).map(|m| (m, frame_camera(m, *center, *scale, *bearing, *pitch)));
        let size = (e.w * PT_PER_MM, e.h * PT_PER_MM);
        gaps.extend(sections.iter().filter_map(|s| {
            let why = match &shown {
                Some((m, cam)) => section_ends(m, cam, size, shape, s.line).err()?,
                None => "its frame's map is missing",
            };
            Some(SectionGap { frame: e.id, frame_name: e.name.clone(), section: s.name.clone(), why })
        }));
    }
    gaps
}

/// What an export of `pages` pages says of the section lines not drawn, each with the pages it is
/// missing from, by reason: one notice, the lines counted, the first ten named; `None` for none.
pub fn sections_notice(gaps: &BTreeMap<SectionGap, usize>, pages: usize) -> Option<String> {
    if gaps.is_empty() {
        return None;
    }
    // A line can be missing for one reason on some pages and another on others: one line still.
    let mut lines: BTreeMap<(u32, &str, &str), Vec<String>> = BTreeMap::new();
    for (g, on) in gaps {
        let on = if pages > 1 { format!(" on {on} of {pages} pages") } else { String::new() };
        lines.entry((g.frame, &g.frame_name, &g.section)).or_default().push(format!("{on}: {}", g.why));
    }
    let named = lines.iter().take(10).map(|((frame, name, section), why)| format!("'{section}' of {name} ({frame}){}", why.join(",")));
    let mut named = named.collect::<Vec<_>>().join("; ");
    if lines.len() > 10 {
        named.push_str(&format!("; and {} more", lines.len() - 10));
    }
    Some(format!("{} section line(s) not drawn: {named}", lines.len()))
}

/// The size of a section's name at its ends, in points.
const SECTION_NAME_PT: f32 = 8.0;

/// A frame's section lines, each from end to end with its name in a box of its colour at both (A
/// and A'), just inside them and wholly in the frame. Gives the names drawn, as [`draw_grid`]
/// gives its labels, each with the edge the credit treats it as: a horizontal line's right name
/// as a right label, which it goes left of, and every other as a bottom one, which it goes above.
pub(super) fn draw_sections(pm: &mut Canvas, sections: &[(&Section, SectionEnds)], ppp: f64) -> Vec<(Edge, [f32; 4])> {
    let k = ppp as f32;
    let (fw, fh) = (pm.width() as f32, pm.height() as f32);
    // A centre `c` of a box `half` across, inside `0..size` when the box fits there.
    let inside = |c: f32, half: f32, size: f32| if size >= 2.0 * half { c.clamp(half, size - half) } else { c };
    let mut drawn = Vec::new();
    for (s, ends) in sections {
        let [a, b] = ends.map(|(x, y)| (x as f32 * k, y as f32 * k));
        let mut pb = PathBuilder::new();
        pb.move_to(a.0, a.1);
        pb.line_to(b.0, b.1);
        if let Some(p) = pb.finish() {
            let stroke = dashed(Stroke { width: s.width as f32 * k, ..Stroke::default() }, s.dash.as_ref(), ppp, &p);
            pm.stroke_path(&p, &paint(s.color, 1.0), &stroke, Transform::identity(), None);
        }
    }
    // The names over every line.
    for (s, ends) in sections {
        let [a, b] = ends.map(|(x, y)| (x as f32 * k, y as f32 * k));
        let px = SECTION_NAME_PT * 1.33 * k;
        let vertical = matches!(s.line, SectionLine::Vertical(_));
        // Dark on a light colour, or one the map shows through, else white.
        let [r, g, b_, alpha] = s.color.0.map(f32::from);
        let ink = if alpha < 128.0 || 0.299 * r + 0.587 * g + 0.114 * b_ > 150.0 { INK } else { Rgba::rgb(255, 255, 255) };
        let boxes = [(a, 1.0, s.name.clone()), (b, -1.0, format!("{}'", s.name))];
        for ((x, y), inward, name) in boxes {
            let hh = px * 0.8;
            let hw = (text::measure(&name, px / 1.33, true) / 2.0 + px * 0.35).max(hh);
            // Inside the end by a point, but never past the line's middle.
            let length = (b.0 - a.0).max(b.1 - a.1);
            let inset = inward * (k + if vertical { hh } else { hw }).min(length / 2.0);
            // Across the line, the whole box inside the frame too.
            let (cx, cy) = if vertical { (inside(x, hw, fw), y + inset) } else { (x + inset, inside(y, hh, fh)) };
            if let Some(path) = Rect::from_ltrb(cx - hw, cy - hh, cx + hw, cy + hh).and_then(|r| rounded_rect(r, hh)) {
                pm.fill_path(&path, &paint(s.color, 1.0), FillRule::Winding, Transform::identity(), None);
            }
            text::text_at(pm, &name, cx, cy, px, ink, true, text::Align::Center, false);
            // Section names lie anywhere in the frame, not only along its edges as a grid's labels
            // do, so the credit keeps clear of every one, whichever end it is.
            let edge = if !vertical && inward < 0.0 { Edge::Right } else { Edge::Bottom };
            drawn.push((edge, [(cx - hw) / k, (cy - hh) / k, (cx + hw) / k, (cy + hh) / k]));
        }
    }
    drawn
}
