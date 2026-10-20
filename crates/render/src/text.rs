//! Text for exported images: label rasterisation (with halos) and the scale bar.

use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, GlyphId, PxScale, ScaleFont, VariableFont};
use tiny_skia::{Paint, Pixmap, Rect, Transform};

use crate::fonts::CjkRegion;
use crate::{PlacedLabel, View};
use rustgis_map::Rgba;
use rustgis_map::layout::TextAlign;

fn font(bold: bool) -> Option<&'static FontRef<'static>> {
    static R: OnceLock<Option<FontRef<'static>>> = OnceLock::new();
    static B: OnceLock<Option<FontRef<'static>>> = OnceLock::new();
    let f = crate::fonts::ui_fonts();
    if bold {
        B.get_or_init(|| {
            let mut face = FontRef::try_from_slice(&f.semibold).ok()?;
            // Heavier where the face is variable (macOS's system font, #591); static ones ignore it.
            face.set_variation(b"wght", crate::fonts::SEMIBOLD_WEIGHT);
            Some(face)
        })
        .as_ref()
    } else {
        R.get_or_init(|| FontRef::try_from_slice(&f.regular).ok()).as_ref()
    }
}

/// Characters that never need a glyph: spaces, controls, zero-width joiners, variation selectors,
/// soft hyphens.
fn invisible(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || matches!(c, '\u{AD}' | '\u{200B}'..='\u{200F}' | '\u{2060}'..='\u{2064}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}' | '\u{E0100}'..='\u{E01EF}')
}

/// An installed CJK face for `region`, parsed once; `None` when there is none (or on the web).
fn cjk(region: CjkRegion) -> Option<&'static FontRef<'static>> {
    static FACES: [OnceLock<Option<FontRef<'static>>>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let slot = match region {
        CjkRegion::SimplifiedChinese => &FACES[0],
        CjkRegion::Japanese => &FACES[1],
        CjkRegion::Korean => &FACES[2],
    };
    slot.get_or_init(|| crate::fonts::cjk_font(region).and_then(|(b, i)| FontRef::try_from_slice_and_index(b, *i).ok())).as_ref()
}

/// The CJK faces to try for `c`, the region whose glyph style suits it first: Hangul looks for a
/// Korean face, kana for a Japanese one, everything else (Han ideographs) for Simplified Chinese.
fn cjk_order(c: char) -> [CjkRegion; 3] {
    use CjkRegion::{Japanese as Jp, Korean as Kr, SimplifiedChinese as Sc};
    match c {
        '\u{1100}'..='\u{11FF}' | '\u{3130}'..='\u{318F}' | '\u{A960}'..='\u{A97F}' | '\u{AC00}'..='\u{D7FF}' => [Kr, Sc, Jp],
        '\u{3040}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}' | '\u{FF66}'..='\u{FF9F}' => [Jp, Sc, Kr],
        _ => [Sc, Jp, Kr],
    }
}

/// The face and glyph that draw `c`: the label font when it has the glyph, else the first
/// installed CJK font that does. `None` when nothing covers it, so no missing-glyph box is drawn.
fn glyph(c: char, bold: bool) -> Option<(&'static FontRef<'static>, GlyphId)> {
    let f = font(bold)?;
    let id = f.glyph_id(c);
    if id.0 != 0 || invisible(c) {
        return (id.0 != 0).then_some((f, id));
    }
    cjk_order(c).into_iter().filter_map(cjk).find_map(|f| {
        let id = f.glyph_id(c);
        (id.0 != 0).then_some((f, id))
    })
}

/// Whether the label fonts (`bold` or regular, with the installed CJK fallbacks) have a glyph for
/// every character of `text`, invisible characters aside.
pub fn covers(text: &str, bold: bool) -> bool {
    if font(bold).is_none() {
        return true;
    }
    text.chars().all(|c| invisible(c) || glyph(c, bold).is_some())
}

/// Lays out `text` at `px` pixels: each drawable glyph with its face and pen position (from 0),
/// and the total advance. Characters no font covers are left out.
fn shape(text: &str, px: f32, bold: bool) -> (Vec<(&'static FontRef<'static>, GlyphId, f32)>, f32) {
    let scale = PxScale::from(px);
    let mut out = Vec::new();
    let mut x = 0.0;
    let mut prev: Option<(&'static FontRef<'static>, GlyphId)> = None;
    for ch in text.chars().take(500) {
        let Some((f, id)) = glyph(ch, bold) else { continue };
        let sf = f.as_scaled(scale);
        if let Some((pf, pid)) = prev
            && std::ptr::eq(pf, f)
        {
            x += sf.kern(pid, id);
        }
        out.push((f, id, x));
        x += sf.h_advance(id);
        prev = Some((f, id));
    }
    (out, x)
}

/// Most lines a label draws: its text comes from files and attributes.
const MAX_LINES: usize = 200;

/// The lines of a label, split as the map view splits them (#577): at each `\n`, a trailing one
/// making an empty last line. Lines past [`MAX_LINES`] are left out (measured and drawn alike).
pub fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n').take(MAX_LINES)
}

/// How many lines a label draws.
pub fn line_count(text: &str) -> usize {
    lines(text).count()
}

/// Text width in points at `size` (font px ≈ size × 1.33 for point sizes), drawn on one line.
pub fn measure(text: &str, size: f32, bold: bool) -> f32 {
    if font(bold).is_none() {
        return text.chars().count() as f32 * size * 0.6;
    }
    shape(text, size * 1.33, bold).1
}

/// A label's width in points: its widest line, as [`draw`] lays its lines out (#577). Text drawn
/// on one line (table cells, chart names…) is measured with [`measure`].
pub fn block_width(text: &str, size: f32, bold: bool) -> f32 {
    lines(text).map(|line| measure(line, size, bold)).fold(0.0, f32::max)
}

/// The distance between a label's lines in points, as [`draw`] sets them: 1.25 × the font's
/// pixel size, as a layout's text.
pub fn line_pitch(size: f32) -> f32 {
    size * 1.33 * 1.25
}

/// A label's height in points, for its collision box: one line as labels have always taken, and
/// a line pitch more for each further line.
pub fn block_height(text: &str, size: f32) -> f32 {
    size * 1.25 + line_count(text).saturating_sub(1) as f32 * line_pitch(size)
}

/// Whether a label is slanted or letter-spaced (a text graphic's, #535), drawn with
/// [`styled_at`].
fn styled(l: &PlacedLabel) -> bool {
    l.italic || l.spacing != 0.0
}

/// How wide a line of `l` is laid out, in pixels at `ppp`: its advance, and its letter spacing
/// between letters (not past the last), as the map view lays it out (#535), so a text is placed
/// alike on screen and in an export. A slant's overhang isn't counted, as it isn't there either.
fn line_width(l: &PlacedLabel, line: &str, ppp: f32) -> f32 {
    let advance = measure(line, l.size, l.bold) * ppp;
    if l.spacing == 0.0 {
        return advance;
    }
    let letters = match font(l.bold) {
        Some(_) => shape(line, l.size * 1.33 * ppp, l.bold).0.len(),
        None => line.chars().count(),
    };
    // Letters drawn tighter than their own width (the most negative spacing) don't make it less
    // than nothing.
    (advance + l.spacing * 1.33 * ppp * letters.saturating_sub(1) as f32).max(0.0)
}

/// A label's width in points: its widest line as [`draw`] lays it out, its style included.
pub fn label_width(l: &PlacedLabel) -> f32 {
    lines(&l.text).map(|line| line_width(l, line, 1.0)).fold(0.0, f32::max)
}

/// The selection frame [`draw`] puts round a label, in points: [left, top, right, bottom].
fn frame_box(l: &PlacedLabel) -> [f32; 4] {
    let (w, pad) = (label_width(l), 3.0);
    let h = l.size * 1.33 * 1.2 + line_count(&l.text).saturating_sub(1) as f32 * line_pitch(l.size);
    let cx = l.center_x();
    [cx - w / 2.0 - pad, l.y - h / 2.0 - pad, cx + w / 2.0 + pad, l.y + h / 2.0 + pad]
}

fn blend(pm: &mut Pixmap, x: i32, y: i32, c: Rgba, cov: f32) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    if x < 0 || y < 0 || x >= w || y >= h || cov <= 0.0 {
        return;
    }
    let idx = (y * w + x) as usize;
    let data = pm.pixels_mut();
    let Some(px) = data.get_mut(idx) else { return };
    let a = (c.a() as f32 / 255.0) * cov.min(1.0);
    let inv = 1.0 - a;
    let r = (c.r() as f32 * a + px.red() as f32 * inv).round() as u8;
    let g = (c.g() as f32 * a + px.green() as f32 * inv).round() as u8;
    let b = (c.b() as f32 * a + px.blue() as f32 * inv).round() as u8;
    let al = ((a * 255.0) + px.alpha() as f32 * inv).round().min(255.0) as u8;
    if let Some(p) = tiny_skia::PremultipliedColorU8::from_rgba(r.min(al), g.min(al), b.min(al), al) {
        *px = p;
    }
}

fn draw_text(pm: &mut Pixmap, text: &str, cx: f32, cy: f32, px_size: f32, color: Rgba, bold: bool, offset: (f32, f32)) {
    let Some(f) = font(bold) else { return };
    let sf = f.as_scaled(PxScale::from(px_size));
    let (glyphs, width) = shape(text, px_size, bold);
    let x0 = cx - width / 2.0 + offset.0;
    let base = cy + (sf.ascent() + sf.descent()) / 2.0 + offset.1;
    for (f, id, x) in glyphs {
        let g = id.with_scale_and_position(sf.scale(), ab_glyph::point(x0 + x, base));
        if let Some(og) = f.outline_glyph(g) {
            let bb = og.px_bounds();
            og.draw(|gx, gy, cov| blend(pm, bb.min.x as i32 + gx as i32, bb.min.y as i32 + gy as i32, color, cov));
        }
    }
}

/// The slant of synthetic italics (the fonts have no italic face): tan 12°, as type designers
/// slant an oblique.
const SLANT: f32 = 0.2126;

/// The ink `text` draws at `px` with `spacing` and `italic`, from the pen's start: its leftmost
/// and rightmost pixel offsets. With a negative spacing the letters can run backwards past the
/// pen, and a slant moves their tops right and their descenders left; (0, 0) when nothing draws.
pub fn styled_extent(text: &str, px: f32, bold: bool, italic: bool, spacing: f32) -> (f32, f32) {
    let Some(f) = font(bold) else { return (0.0, 0.0) };
    let sf = f.as_scaled(PxScale::from(px));
    let base = (sf.ascent() + sf.descent()) / 2.0;
    let (glyphs, _) = shape(text, px, bold);
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for (i, (f, id, x)) in glyphs.into_iter().enumerate() {
        let g = id.with_scale_and_position(sf.scale(), ab_glyph::point(x + spacing * i as f32, base));
        let Some(og) = f.outline_glyph(g) else { continue };
        let bb = og.px_bounds();
        let (lean_top, lean_bottom) = if italic { (SLANT * (base - bb.min.y), SLANT * (base - bb.max.y)) } else { (0.0, 0.0) };
        lo = lo.min(bb.min.x + lean_top.min(lean_bottom));
        hi = hi.max(bb.max.x + lean_top.max(lean_bottom) + if italic { 1.0 } else { 0.0 });
    }
    if lo.is_finite() && hi.is_finite() { (lo, hi) } else { (0.0, 0.0) }
}

/// Draws `text` from `left` (pixels), its line centred on `cy`, at `px` with `spacing` pixels
/// between letters, slanted when `italic`. Plain (no spacing, upright) it draws as
/// [`text_at`] does from the left.
#[allow(clippy::too_many_arguments)]
pub fn styled_at(pm: &mut Pixmap, text: &str, left: f32, cy: f32, px: f32, color: Rgba, bold: bool, italic: bool, spacing: f32) {
    if !(left.is_finite() && cy.is_finite() && px.is_finite() && spacing.is_finite()) || px <= 0.0 {
        return;
    }
    let Some(f) = font(bold) else { return };
    let sf = f.as_scaled(PxScale::from(px));
    let base = cy + (sf.ascent() + sf.descent()) / 2.0;
    let (glyphs, _) = shape(text, px, bold);
    for (i, (f, id, x)) in glyphs.into_iter().enumerate() {
        let g = id.with_scale_and_position(sf.scale(), ab_glyph::point(left + x + spacing * i as f32, base));
        let Some(og) = f.outline_glyph(g) else { continue };
        let bb = og.px_bounds();
        og.draw(|gx, gy, cov| {
            let (px_x, px_y) = (bb.min.x + gx as f32, bb.min.y + gy as f32);
            // (Floored, so a row just above or left of the pixmap is clipped, not drawn on row 0.)
            let row = px_y.floor() as i32;
            if !italic {
                blend(pm, px_x.floor() as i32, row, color, cov);
                return;
            }
            // Each row leans right by its height above the baseline, split across the two
            // pixels it falls between so the slant stays smooth.
            let shifted = px_x + SLANT * (base - px_y - 0.5);
            let (ix, frac) = (shifted.floor(), shifted - shifted.floor());
            blend(pm, ix as i32, row, color, cov * (1.0 - frac));
            blend(pm, ix as i32 + 1, row, color, cov * frac);
        });
    }
}

/// Draws a placed label (logical coordinates) with its halo.
pub fn draw(pm: &mut Pixmap, l: &PlacedLabel, ppp: f32) {
    let px = l.size * 1.33 * ppp;
    let (cx, cy) = (l.center_x() * ppp, l.y * ppp);
    // Several lines (#577) as the map view draws them: each aligned in a block as wide as the
    // widest (from its left unless a text graphic says otherwise, #535), the block centred on the
    // label's point. Each line is measured once: its width places it, the widest sizes the block.
    let widths: Vec<(&str, f32)> = lines(&l.text).map(|line| (line, line_width(l, line, ppp))).collect();
    let (n, line_h) = (widths.len(), line_pitch(l.size) * ppp);
    let w = widths.iter().map(|(_, lw)| *lw).fold(0.0, f32::max);
    if l.framed {
        let [x0, y0, x1, y1] = frame_box(l).map(|v| v * ppp);
        if let Some(r) = Rect::from_ltrb(x0, y0, x1, y1) {
            frame(pm, r, ppp);
        }
    }
    let top = cy - n.saturating_sub(1) as f32 * line_h / 2.0;
    // Each line: its text, its left edge (where the pen starts), its width, its centre y.
    let placed: Vec<(&str, f32, f32, f32)> = widths
        .iter()
        .enumerate()
        .map(|(i, (line, lw))| {
            let free = match l.line_align {
                TextAlign::Left => 0.0,
                TextAlign::Center => (w - lw) / 2.0,
                TextAlign::Right => w - lw,
            };
            (*line, cx - w / 2.0 + free, *lw, top + i as f32 * line_h)
        })
        .collect();
    let (styled, spacing) = (styled(l), l.spacing * 1.33 * ppp);
    let put = |pm: &mut Pixmap, &(line, left, lw, y): &(&str, f32, f32, f32), color: Rgba, (dx, dy): (f32, f32)| {
        if styled {
            styled_at(pm, line, left + dx, y + dy, px, color, l.bold, l.italic, spacing);
        } else {
            draw_text(pm, line, left + lw / 2.0, y, px, color, l.bold, (dx, dy));
        }
    };
    // Every line's halo first, so none covers the line above it.
    if let Some(h) = l.halo {
        let r = 1.2 * ppp;
        let ring =
            [(-r, 0.0), (r, 0.0), (0.0, -r), (0.0, r), (-r * 0.7, -r * 0.7), (r * 0.7, r * 0.7), (-r * 0.7, r * 0.7), (r * 0.7, -r * 0.7)];
        for line in &placed {
            for offset in ring {
                put(pm, line, h, offset);
            }
        }
    }
    for line in &placed {
        put(pm, line, l.color, (0.0, 0.0));
    }
}

/// The selection's frame: a light fill and an outline in the selection colour.
fn frame(pm: &mut Pixmap, r: Rect, ppp: f32) {
    let path = tiny_skia::PathBuilder::from_rect(r);
    let fill = crate::paint(crate::SELECTION.with_alpha(40), 1.0);
    pm.fill_path(&path, &fill, tiny_skia::FillRule::Winding, Transform::identity(), None);
    let stroke = tiny_skia::Stroke { width: 2.0 * ppp, ..tiny_skia::Stroke::default() };
    pm.stroke_path(&path, &crate::paint(crate::SELECTION, 1.0), &stroke, Transform::identity(), None);
}

/// A two-tone scale bar in the lower-left corner.
/// `ground_per_pt` is ground metres per logical pixel at the centre of view.
pub fn scale_bar(pm: &mut Pixmap, view: &View, ground_per_pt: f64) {
    let ppp = view.pixels_per_point as f32;
    let target = rustgis_geom::nice_distance(ground_per_pt * 120.0);
    if target <= 0.0 {
        return;
    }
    let len = (target / ground_per_pt) as f32;
    let (x0, y0) = (16.0 * ppp, pm.height() as f32 - 22.0 * ppp);
    let mut bg = Paint::default();
    bg.set_color_rgba8(255, 255, 255, 210);
    if let Some(r) = Rect::from_xywh(x0 - 6.0 * ppp, y0 - 18.0 * ppp, len * ppp + 12.0 * ppp, 30.0 * ppp) {
        pm.fill_rect(r, &bg, Transform::identity(), None);
    }
    for i in 0..4 {
        let mut p = Paint::default();
        if i % 2 == 0 {
            p.set_color_rgba8(40, 40, 40, 255);
        } else {
            p.set_color_rgba8(250, 250, 250, 255);
        }
        if let Some(r) = Rect::from_xywh(x0 + len * ppp * i as f32 / 4.0, y0, len * ppp / 4.0, 5.0 * ppp) {
            pm.fill_rect(r, &p, Transform::identity(), None);
        }
    }
    let label = PlacedLabel {
        text: rustgis_geom::format_distance(target),
        x: (x0 / ppp) + len / 2.0,
        y: y0 / ppp - 8.0,
        size: 8.0,
        color: Rgba::rgb(30, 30, 30),
        halo: None,
        bold: false,
        icon: None,
        align: 0.0,
        framed: false,
        italic: false,
        spacing: 0.0,
        line_align: TextAlign::Left,
    };
    draw(pm, &label, ppp);
}

/// Horizontal alignment for [`text_at`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
}

/// Draws one line of text at pixel position (`x`, `y`) (`y` is the line's middle), `px_size`
/// pixels tall, optionally with a white halo.
pub fn text_at(pm: &mut Pixmap, text: &str, x: f32, y: f32, px_size: f32, color: Rgba, bold: bool, align: Align, halo: bool) {
    if !(x.is_finite() && y.is_finite() && px_size.is_finite()) || px_size <= 0.0 {
        return;
    }
    let cx = match align {
        Align::Center => x,
        Align::Left => x + measure(text, px_size / 1.33, bold) / 2.0,
    };
    if halo {
        let r = (px_size * 0.09).max(1.0);
        for (dx, dy) in [(-r, 0.0), (r, 0.0), (0.0, -r), (0.0, r)] {
            draw_text(pm, text, cx, y, px_size, Rgba::new(255, 255, 255, 230), bold, (dx, dy));
        }
    }
    draw_text(pm, text, cx, y, px_size, color, bold, (0.0, 0.0));
}

#[cfg(test)]
mod coverage_tests {
    #[test]
    fn knows_which_characters_it_can_draw() {
        // Which scripts are covered depends on the fonts this computer has (a system font may
        // cover Arabic); Latin always is, and a private-use character never is.
        for bold in [false, true] {
            assert!(super::covers("Syria", bold));
            assert!(super::covers("Türkiye Ελλάδα Беларусь", bold));
            assert!(!super::covers("Syria \u{10FFFD}", bold));
            // Invisible characters don't need a glyph.
            assert!(super::covers("Syria\u{200B}\u{FE0F}\u{E0100}", bold));
        }
    }

    #[test]
    fn draws_nothing_for_characters_no_font_has() {
        // A private-use character has no glyph anywhere: it used to come out as a .notdef box.
        let mut pm = tiny_skia::Pixmap::new(80, 40).unwrap();
        super::text_at(
            &mut pm,
            "\u{10FFFD}\u{10FFFD}",
            40.0,
            20.0,
            24.0,
            rustgis_map::Rgba::rgb(0, 0, 0),
            false,
            super::Align::Center,
            true,
        );
        assert!(pm.pixels().iter().all(|p| p.alpha() == 0), "a missing-glyph box was drawn");
        assert_eq!(super::measure("\u{10FFFD}", 10.0, false), 0.0);
    }

    #[test]
    fn falls_back_to_an_installed_cjk_font() {
        use crate::fonts::{CjkRegion, cjk_font};
        use ab_glyph::Font;
        // Only meaningful on a computer with a CJK font (Windows has Microsoft YaHei).
        if cjk_font(CjkRegion::SimplifiedChinese).is_none() {
            return;
        }
        for bold in [false, true] {
            assert!(super::covers("碑林区", bold));
            assert!(super::covers("Mixed 中英 label", bold));
        }
        // The fallback glyphs take up room, so CJK labels are measured and centred correctly.
        assert!(super::measure("Mixed 中英 label", 10.0, false) > super::measure("Mixed  label", 10.0, false) + 10.0);
        // And the fallback glyphs are drawn, not just measured.
        let Some(ch) = "碑林区".chars().find(|&c| {
            let (Some(label), Some(cjk)) = (super::font(false), super::cjk(CjkRegion::SimplifiedChinese)) else { return false };
            label.glyph_id(c).0 == 0 && cjk.glyph_id(c).0 != 0
        }) else {
            return;
        };
        let mut pm = tiny_skia::Pixmap::new(80, 40).unwrap();
        super::text_at(&mut pm, &ch.to_string(), 40.0, 20.0, 24.0, rustgis_map::Rgba::rgb(0, 0, 0), false, super::Align::Center, false);
        assert!(pm.pixels().iter().any(|p| p.alpha() > 0), "the fallback glyph was not drawn");
    }
}
