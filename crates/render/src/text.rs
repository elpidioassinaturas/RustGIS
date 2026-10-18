//! Text for exported images: label rasterisation (with halos) and the scale bar.

use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, GlyphId, PxScale, ScaleFont};
use tiny_skia::{Paint, Pixmap, Rect, Transform};

use crate::fonts::CjkRegion;
use crate::{PlacedLabel, View};
use rustgis_map::Rgba;

fn font(bold: bool) -> Option<&'static FontRef<'static>> {
    static R: OnceLock<Option<FontRef<'static>>> = OnceLock::new();
    static B: OnceLock<Option<FontRef<'static>>> = OnceLock::new();
    let f = crate::fonts::ui_fonts();
    if bold {
        B.get_or_init(|| FontRef::try_from_slice(&f.semibold).ok()).as_ref()
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

/// Text width in points at `size` (font px ≈ size × 1.33 for point sizes).
pub fn measure(text: &str, size: f32, bold: bool) -> f32 {
    if font(bold).is_none() {
        return text.chars().count() as f32 * size * 0.6;
    }
    shape(text, size * 1.33, bold).1
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

/// Draws a placed label (logical coordinates) with its halo.
pub fn draw(pm: &mut Pixmap, l: &PlacedLabel, ppp: f32) {
    let px = l.size * 1.33 * ppp;
    let (cx, cy) = (l.center_x() * ppp, l.y * ppp);
    if l.framed {
        let (w, h, pad) = (measure(&l.text, l.size, l.bold) * ppp, px * 1.2, 3.0 * ppp);
        if let Some(r) = Rect::from_xywh(cx - w / 2.0 - pad, cy - h / 2.0 - pad, w + 2.0 * pad, h + 2.0 * pad) {
            frame(pm, r, ppp);
        }
    }
    if let Some(h) = l.halo {
        let r = 1.2 * ppp;
        for (dx, dy) in
            [(-r, 0.0), (r, 0.0), (0.0, -r), (0.0, r), (-r * 0.7, -r * 0.7), (r * 0.7, r * 0.7), (-r * 0.7, r * 0.7), (r * 0.7, -r * 0.7)]
        {
            draw_text(pm, &l.text, cx, cy, px, h, l.bold, (dx, dy));
        }
    }
    draw_text(pm, &l.text, cx, cy, px, l.color, l.bold, (0.0, 0.0));
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
