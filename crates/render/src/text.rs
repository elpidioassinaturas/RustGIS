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

/// The family and style a line is drawn in: a layout text's serif (#517), with its bold and
/// italic, or the label font's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Face {
    pub bold: bool,
    pub italic: bool,
    /// The installed serif; the sans where there is none.
    pub serif: bool,
}

/// The installed serif's faces, parsed once: (regular, bold, italic, bold italic).
type SerifFaces = (FontRef<'static>, Option<FontRef<'static>>, Option<FontRef<'static>>, Option<FontRef<'static>>);

/// The weight a serif's bold faces are drawn at where the file is a variable font (a variable
/// Noto Serif is one file for every weight); static faces ignore it.
const SERIF_BOLD_WEIGHT: f32 = 700.0;

fn serif() -> Option<&'static SerifFaces> {
    static S: OnceLock<Option<SerifFaces>> = OnceLock::new();
    S.get_or_init(|| {
        let f = crate::fonts::serif_fonts()?;
        let parse = |b: Option<&'static [u8]>| b.and_then(|b| FontRef::try_from_slice(b).ok());
        let heavy = |b: Option<&'static [u8]>| {
            let mut face = parse(b)?;
            face.set_variation(b"wght", SERIF_BOLD_WEIGHT);
            Some(face)
        };
        // A variable family's bold faces are its regular and italic bytes parsed again (a
        // `FontRef` only borrows them), drawn at the bold weight.
        let (bold, bold_italic) =
            if f.variable { (Some(f.regular.as_slice()), f.italic.as_deref()) } else { (f.bold.as_deref(), f.bold_italic.as_deref()) };
        Some((FontRef::try_from_slice(&f.regular).ok()?, heavy(bold), parse(f.italic.as_deref()), heavy(bold_italic)))
    })
    .as_ref()
}

/// The face `face` is drawn in from `serif`'s faces, and whether it is slanted by hand: a real
/// italic face where the family has one (bold italic, else italic), else the bold or regular
/// slanted, as the sans always is. Without a serif, the label font.
fn face_in(face: Face, serif: Option<&'static SerifFaces>) -> Option<(&'static FontRef<'static>, bool)> {
    let Some((regular, bold, italic, bold_italic)) = serif.filter(|_| face.serif) else {
        return font(face.bold).map(|f| (f, face.italic));
    };
    let upright = if face.bold { bold.as_ref().unwrap_or(regular) } else { regular };
    let real_italic = if face.bold { bold_italic.as_ref() } else { italic.as_ref() };
    Some(match real_italic.filter(|_| face.italic) {
        Some(f) => (f, false),
        None => (upright, face.italic),
    })
}

fn face_font(face: Face) -> Option<(&'static FontRef<'static>, bool)> {
    face_in(face, serif())
}

/// The label font of `face`'s weight, for the characters `f` lacks: `None` when `f` is it.
fn sans_beside(f: &'static FontRef<'static>, face: Face) -> Option<&'static FontRef<'static>> {
    font(face.bold).filter(|s| !std::ptr::eq(*s, f))
}

/// Whether a glyph drawn from `g` leans: the whole line does when its face is slanted by hand
/// (`slanted`), and in a real italic face `f` a glyph taken from another face (the sans or a
/// CJK face, for a character the italic lacks) is slanted to match it.
fn leans(face: Face, f: &FontRef<'static>, slanted: bool, g: &FontRef<'static>) -> bool {
    slanted || (face.italic && !std::ptr::eq(f, g))
}

/// The scale that draws `f` with an em `px` pixels tall, as CSS sizes a font and the map view
/// (egui) draws it. (ab_glyph's own pixel scale is the height from descender to ascender,
/// a good third more than the em in most faces.)
fn em(f: &FontRef, px: f32) -> PxScale {
    let (h, upm) = (f.height_unscaled(), f.units_per_em().unwrap_or(0.0));
    let k = if h.is_finite() && upm.is_finite() && h > 0.0 && upm > 0.0 { h / upm } else { 1.0 };
    PxScale::from(px * k)
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

/// The face and glyph that draw `c`: `f` when it has the glyph, else `sans` (the label font a
/// serif text falls back to: an arrow or a script the serif lacks), else the first installed CJK
/// font that does. `None` when nothing covers it, so no missing-glyph box is drawn.
fn glyph(c: char, f: &'static FontRef<'static>, sans: Option<&'static FontRef<'static>>) -> Option<(&'static FontRef<'static>, GlyphId)> {
    let id = f.glyph_id(c);
    if id.0 != 0 || invisible(c) {
        return (id.0 != 0).then_some((f, id));
    }
    if let Some(s) = sans {
        let id = s.glyph_id(c);
        if id.0 != 0 {
            return Some((s, id));
        }
    }
    cjk_order(c).into_iter().filter_map(cjk).find_map(|f| {
        let id = f.glyph_id(c);
        (id.0 != 0).then_some((f, id))
    })
}

/// Whether the label fonts (`bold` or regular, with the installed CJK fallbacks) have a glyph for
/// every character of `text`, invisible characters aside.
pub fn covers(text: &str, bold: bool) -> bool {
    let Some(f) = font(bold) else { return true };
    text.chars().all(|c| invisible(c) || glyph(c, f, None).is_some())
}

/// Lays out `text` at `px` pixels in `face` (with `sans` for the characters it lacks, as
/// [`glyph`] takes them): each drawable glyph with its face and pen position (from 0), and the
/// total advance. Characters no font covers are left out.
fn shape(
    text: &str,
    px: f32,
    face: &'static FontRef<'static>,
    sans: Option<&'static FontRef<'static>>,
) -> (Vec<(&'static FontRef<'static>, GlyphId, f32)>, f32) {
    let mut out = Vec::new();
    let mut x = 0.0;
    let mut prev: Option<(&'static FontRef<'static>, GlyphId)> = None;
    for ch in text.chars().take(500) {
        let Some((f, id)) = glyph(ch, face, sans) else { continue };
        let sf = f.as_scaled(em(f, px));
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
    let Some(f) = font(bold) else {
        return text.chars().count() as f32 * size * 0.6;
    };
    shape(text, size * 1.33, f, None).1
}

/// How wide `text` is on one line at `size` with `spacing` points between letters, in points.
fn spaced_width(text: &str, size: f32, bold: bool, spacing: f32) -> f32 {
    let w = measure(text, size, bold);
    if spacing == 0.0 {
        return w;
    }
    (w + spacing * 1.33 * text.chars().count().saturating_sub(1) as f32).max(0.0)
}

/// A line of a label broken into lines about `max_w` points wide, as MapLibre wraps a label to
/// `text-max-width`: at spaces, and after a hyphen, dash, slash or the like; the lines balanced
/// (each near the same width) rather than filled, and a word longer than that a line of its
/// own. A line that fits, or can't be broken, is kept as it is.
pub fn wrap(line: &str, size: f32, bold: bool, spacing: f32, max_w: f32) -> Vec<String> {
    // The pieces a line may break after: each with whether a space followed it.
    let mut pieces: Vec<(String, bool)> = Vec::new();
    let mut cur = String::new();
    for c in line.chars().take(2000) {
        if c == ' ' {
            if !cur.is_empty() {
                pieces.push((std::mem::take(&mut cur), true));
            } else if let Some(last) = pieces.last_mut() {
                // A space after a piece that ended at a dash or "&".
                last.1 = true;
            }
            continue;
        }
        cur.push(c);
        if matches!(c, '-' | '/' | '&' | '+' | ')' | '\u{AD}' | '\u{B7}' | '\u{200B}' | '\u{2010}' | '\u{2013}') {
            pieces.push((std::mem::take(&mut cur), false));
        }
    }
    if !cur.is_empty() {
        pieces.push((cur, true));
    }
    let width = |s: &str| spaced_width(s, size, bold, spacing);
    let total = width(line);
    if pieces.len() < 2 || !(max_w.is_finite() && max_w > 0.0) || total <= max_w {
        return vec![line.to_string()];
    }
    // As many lines as the width takes, each aiming at an equal share of it.
    let n = (total / max_w).ceil().max(1.0);
    let target = total / n;
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut spaced = false;
    for (piece, space_after) in pieces {
        if cur.is_empty() {
            cur = piece;
            spaced = space_after;
            continue;
        }
        let longer = if spaced { format!("{cur} {piece}") } else { format!("{cur}{piece}") };
        let (now, then) = (width(&cur), width(&longer));
        // The piece goes on this line if that brings it nearer the target (and no wider than
        // the most a line may be), else it starts the next.
        if then <= target || (then <= max_w.max(target) && (then - target).abs() <= (now - target).abs()) {
            cur = longer;
        } else {
            out.push(std::mem::take(&mut cur));
            cur = piece;
        }
        spaced = space_after;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
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
        Some(f) => shape(line, l.size * 1.33 * ppp, f, None).0.len(),
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
    let sf = f.as_scaled(em(f, px_size));
    let (glyphs, width) = shape(text, px_size, f, None);
    let x0 = cx - width / 2.0 + offset.0;
    let base = cy + (sf.ascent() + sf.descent()) / 2.0 + offset.1;
    for (f, id, x) in glyphs {
        let g = id.with_scale_and_position(em(f, px_size), ab_glyph::point(x0 + x, base));
        if let Some(og) = f.outline_glyph(g) {
            let bb = og.px_bounds();
            og.draw(|gx, gy, cov| blend(pm, bb.min.x as i32 + gx as i32, bb.min.y as i32 + gy as i32, color, cov));
        }
    }
}

/// The slant of synthetic italics (the fonts have no italic face): tan 12°, as type designers
/// slant an oblique.
const SLANT: f32 = 0.2126;

/// The ink `text` draws at `px` in `face` with `spacing`, from the pen's start: its leftmost
/// and rightmost pixel offsets. With a negative spacing the letters can run backwards past the
/// pen, and a slant moves their tops right and their descenders left; (0, 0) when nothing draws.
pub fn styled_extent(text: &str, px: f32, face: Face, spacing: f32) -> (f32, f32) {
    let Some((f, slanted)) = face_font(face) else { return (0.0, 0.0) };
    let sf = f.as_scaled(em(f, px));
    let base = (sf.ascent() + sf.descent()) / 2.0;
    let (glyphs, _) = shape(text, px, f, sans_beside(f, face));
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for (i, (gf, id, x)) in glyphs.into_iter().enumerate() {
        // Measured as `styled_at` draws it.
        let italic = leans(face, f, slanted, gf);
        let g = id.with_scale_and_position(em(gf, px), ab_glyph::point(x + spacing * i as f32, base));
        let Some(og) = gf.outline_glyph(g) else { continue };
        let bb = og.px_bounds();
        let (lean_top, lean_bottom) = if italic { (SLANT * (base - bb.min.y), SLANT * (base - bb.max.y)) } else { (0.0, 0.0) };
        lo = lo.min(bb.min.x + lean_top.min(lean_bottom));
        hi = hi.max(bb.max.x + lean_top.max(lean_bottom) + if italic { 1.0 } else { 0.0 });
    }
    if lo.is_finite() && hi.is_finite() { (lo, hi) } else { (0.0, 0.0) }
}

/// Draws `text` from `left` (pixels), its line centred on `cy`, at `px` in `face` with `spacing`
/// pixels between letters: italic in the family's italic face where it has one, else slanted.
/// Plain (sans, no spacing, upright) it draws as [`text_at`] does from the left.
#[allow(clippy::too_many_arguments)]
pub fn styled_at(pm: &mut Pixmap, text: &str, left: f32, cy: f32, px: f32, color: Rgba, face: Face, spacing: f32) {
    if !(left.is_finite() && cy.is_finite() && px.is_finite() && spacing.is_finite()) || px <= 0.0 {
        return;
    }
    let Some((f, slanted)) = face_font(face) else { return };
    let sf = f.as_scaled(em(f, px));
    let base = cy + (sf.ascent() + sf.descent()) / 2.0;
    let (glyphs, _) = shape(text, px, f, sans_beside(f, face));
    for (i, (gf, id, x)) in glyphs.into_iter().enumerate() {
        let italic = leans(face, f, slanted, gf);
        let g = id.with_scale_and_position(em(gf, px), ab_glyph::point(left + x + spacing * i as f32, base));
        let Some(og) = gf.outline_glyph(g) else { continue };
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

/// Draws a placed label (logical coordinates) with its halo. A label turned with the map (one
/// along a street) is drawn upright into a picture of its own, which is turned into place.
pub fn draw(pm: &mut Pixmap, l: &PlacedLabel, ppp: f32) {
    match l.angle {
        Some(a) if a.is_finite() && a.abs() > 1e-3 => draw_turned(pm, l, a, ppp),
        _ => draw_upright(pm, l, ppp),
    }
}

/// Draws `l` turned by `angle` (radians, clockwise) about its centre.
fn draw_turned(pm: &mut Pixmap, l: &PlacedLabel, angle: f32, ppp: f32) {
    let (w, h) = (label_width(l), block_height(&l.text, l.size).max(line_pitch(l.size)));
    // Room for the halo, and for the glyphs reaching past the line.
    let pad = 4.0 + l.size * 0.5;
    let (pw, ph) = (((w + 2.0 * pad) * ppp).ceil(), ((h + 2.0 * pad) * ppp).ceil());
    if !(pw.is_finite() && ph.is_finite()) || pw > 4096.0 || ph > 4096.0 {
        return;
    }
    let Some(mut pic) = Pixmap::new(pw.max(1.0) as u32, ph.max(1.0) as u32) else { return };
    let (cx, cy) = (pw / ppp / 2.0, ph / ppp / 2.0);
    let upright = PlacedLabel { x: cx + l.align * w, y: cy, angle: None, framed: false, icon: None, ..l.clone() };
    draw_upright(&mut pic, &upright, ppp);
    let (tx, ty) = (l.center_x() * ppp, l.y * ppp);
    let t = Transform::from_translate(-pw / 2.0, -ph / 2.0).post_rotate(angle.to_degrees()).post_translate(tx, ty);
    let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..tiny_skia::PixmapPaint::default() };
    pm.draw_pixmap(0, 0, pic.as_ref(), &paint, t, None);
}

fn draw_upright(pm: &mut Pixmap, l: &PlacedLabel, ppp: f32) {
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
            styled_at(pm, line, left + dx, y + dy, px, color, Face { bold: l.bold, italic: l.italic, serif: false }, spacing);
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
        angle: None,
        collide: Default::default(),
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

    /// A text asking for a serif (#517) where none is installed is drawn in the label font, slanted
    /// as before for italic: nothing goes blank, and the result is the same on every machine.
    #[test]
    fn a_missing_serif_draws_in_the_sans() {
        use super::{Face, face_in, font};
        for bold in [false, true] {
            for italic in [false, true] {
                let (f, slant) = face_in(Face { bold, italic, serif: true }, None).unwrap();
                assert!(std::ptr::eq(f, font(bold).unwrap()) && slant == italic);
            }
        }
    }

    /// The serif's faces picked for each style, a character it lacks taken from the sans, and that
    /// glyph slanted in a real italic, with faces of the test's own (egui's bundled Hack as the
    /// serif's regular, Ubuntu Light as its italic): run on every machine, serif installed or not.
    #[test]
    fn serif_faces_are_picked_and_fall_back() {
        use super::{Face, SerifFaces, face_in, leans, shape};
        use ab_glyph::{Font, FontRef};
        let parse = |b: &'static [u8]| FontRef::try_from_slice(b).unwrap();
        let faces: &'static SerifFaces =
            Box::leak(Box::new((parse(epaint_default_fonts::HACK_REGULAR), None, Some(parse(epaint_default_fonts::UBUNTU_LIGHT)), None)));
        let (regular, italic) = (&faces.0, faces.2.as_ref().unwrap());
        let pick = |bold, it| face_in(Face { bold, italic: it, serif: true }, Some(faces)).unwrap();
        let is = |(f, slant): (&FontRef, bool), g: &FontRef, s: bool| std::ptr::eq(f, g) && slant == s;
        assert!(is(pick(false, false), regular, false));
        assert!(is(pick(false, true), italic, false), "the real italic, not slanted");
        // No bold file: the regular; no bold italic: the regular, slanted.
        assert!(is(pick(true, false), regular, false));
        assert!(is(pick(true, true), regular, true));
        // A sans text never takes them.
        assert!(!std::ptr::eq(face_in(Face::default(), Some(faces)).unwrap().0, regular));
        // A character the serif lacks and the sans has: drawn from the sans.
        let sans = italic;
        let c = ('\u{100}'..='\u{2FFF}').find(|&c| regular.glyph_id(c).0 == 0 && sans.glyph_id(c).0 != 0).expect("a character Hack lacks");
        let (glyphs, _) = shape(&format!("a{c}"), 24.0, regular, Some(sans));
        assert!(glyphs.len() == 2 && std::ptr::eq(glyphs[1].0, sans), "{c:?} left out");
        // In a real italic, a glyph from another face leans; the italic's own don't.
        let it = Face { italic: true, serif: true, ..Face::default() };
        assert!(!leans(it, italic, false, italic) && leans(it, italic, false, regular));
        assert!(!leans(Face { serif: true, ..Face::default() }, regular, false, sans), "upright text stays upright");
    }

    /// With a serif installed (Georgia on Windows, Times on macOS, Noto or DejaVu Serif on Linux),
    /// a text asking for it is drawn in it, and italic in the family's italic face, not slanted
    /// by hand; the sans is untouched.
    #[test]
    fn an_installed_serif_is_drawn() {
        use super::{Face, face_font, font};
        let Some(serif) = super::serif() else { return };
        let (f, slant) = face_font(Face { serif: true, ..Face::default() }).unwrap();
        assert!(std::ptr::eq(f, &serif.0) && !slant);
        let (sans, _) = face_font(Face::default()).unwrap();
        assert!(std::ptr::eq(sans, font(false).unwrap()));
        if let Some(italic) = &serif.2 {
            let (it, slant) = face_font(Face { italic: true, serif: true, ..Face::default() }).unwrap();
            assert!(std::ptr::eq(it, italic) && !slant);
        }
        // The same words in the serif and the sans draw differently.
        let draw = |face: Face| {
            let mut pm = tiny_skia::Pixmap::new(300, 40).unwrap();
            super::styled_at(&mut pm, "Rio Amazonas", 2.0, 20.0, 24.0, rustgis_map::Rgba::rgb(0, 0, 0), face, 0.0);
            pm.data().to_vec()
        };
        assert_ne!(draw(Face { serif: true, ..Face::default() }), draw(Face::default()));
        // A character the serif lacks and the label font has is drawn from the label font, not
        // left out.
        use ab_glyph::Font;
        let sans = font(false).unwrap();
        let lacking = ('\u{2190}'..='\u{2BFF}').find(|&c| serif.0.glyph_id(c).0 == 0 && sans.glyph_id(c).0 != 0);
        if let Some(c) = lacking {
            let (glyphs, _) = super::shape(&c.to_string(), 24.0, &serif.0, super::sans_beside(&serif.0, Face::default()));
            assert!(glyphs.len() == 1 && std::ptr::eq(glyphs[0].0, sans), "{c:?} left out");
        }
    }
}
