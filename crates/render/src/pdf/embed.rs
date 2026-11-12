//! A face embedded in a PDF as itself (#1815): a subset of its file holding the glyphs the
//! document sets, written as a composite font (Type 0 over a CID font, Identity-H), so that a
//! vector editor reads its text as text. A face is embedded only when its file draws exactly what
//! the export draws (the face at its file's default instance) and its licence allows it; anything
//! else is set in Type 3 outlines, with the reason.

use ab_glyph::{Font, FontRef, GlyphId, OutlineCurve};

/// The largest subset of one face embedded, and of all of a document's faces together: past
/// them a face's text is set in outlines.
const MAX_SUBSET: usize = 8 << 20;
pub(super) const MAX_EMBEDDED: usize = 32 << 20;
/// The most faces of a collection (`.ttc`) looked through for the one drawn.
const MAX_COLLECTION: u32 = 64;

/// A face's subset, ready to be written.
pub(super) struct Embedded {
    /// The subset font file, and whether its outlines are CFF (else TrueType).
    pub file: Vec<u8>,
    pub cff: bool,
    /// Its name, with the subset's tag (`ABCDEF+Name`).
    pub name: String,
    /// The subset's glyph (the CID the content sets) of each glyph of the face.
    pub remapper: subsetter::GlyphRemapper,
    /// Its descriptor's metrics, in thousandths of the em: box, ascent, descent, cap height,
    /// italic angle in degrees.
    pub bbox: [f32; 4],
    pub ascent: f32,
    pub descent: f32,
    pub cap_height: f32,
    pub italic_angle: f32,
}

/// Why a face's text is set in outlines rather than in the face itself.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Refused {
    /// Its licence forbids embedding (`fsType` 2), subsetting (0x100), or allows bitmaps only (0x200).
    Licence(&'static str),
    /// It is drawn at a variable font's instance other than its file's default.
    Instance,
    /// Its file doesn't hold the face as it is drawn.
    NotInFile,
    /// Its file can't be read or subset.
    Unreadable(String),
    /// Its subset would pass [`MAX_SUBSET`].
    SubsetTooLarge,
    /// With it, the document's embedded faces would pass [`MAX_EMBEDDED`].
    DocumentTooLarge,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Licence(what) => write!(f, "its licence {what}"),
            Refused::Instance => f.write_str("it is drawn at another weight or width than its file's default"),
            Refused::NotInFile => f.write_str("its file doesn't hold it as drawn"),
            Refused::Unreadable(e) => write!(f, "its file could not be subset: {e}"),
            Refused::SubsetTooLarge => write!(f, "its subset passes {} MB", MAX_SUBSET >> 20),
            Refused::DocumentTooLarge => write!(f, "the document's embedded faces would pass {} MB", MAX_EMBEDDED >> 20),
        }
    }
}

/// A name for the face in a notice: its full name, else its family, else its PostScript name.
pub(super) fn face_name(font: &FontRef<'_>) -> String {
    let data = font.font_data();
    let face = (0..collection_len(data)).find_map(|i| ttf_parser::Face::parse(data, i).ok().filter(|f| same_face(font, f)));
    face.and_then(|f| {
        [ttf_parser::name_id::FULL_NAME, ttf_parser::name_id::FAMILY, ttf_parser::name_id::POST_SCRIPT_NAME]
            .into_iter()
            .find_map(|id| f.names().into_iter().filter(|n| n.name_id == id).find_map(|n| n.to_string()))
    })
    .map(|n| n.chars().filter(|c| !c.is_control()).take(80).collect())
    .unwrap_or_else(|| "an unnamed face".into())
}

/// How many faces `data` holds: one for a single font file.
fn collection_len(data: &[u8]) -> u32 {
    ttf_parser::fonts_in_collection(data).unwrap_or(1).clamp(1, MAX_COLLECTION)
}

/// Whether `face` parsed from the file is the one `font` was read as: the same number of glyphs
/// and units per em (a quick check before comparing outlines).
fn same_face(font: &FontRef<'_>, face: &ttf_parser::Face) -> bool {
    usize::from(face.number_of_glyphs()) == font.glyph_count() && font.units_per_em() == Some(f32::from(face.units_per_em()))
}

/// The face `font` as a subset holding `glyphs`, or why it can't be. A subsetter that fails
/// leaves the face to outlines; natively, one that panics on a hostile file does too (its panic
/// caught). In the browser a panic can't be caught (wasm32 aborts), but no file of the user's
/// reaches it there: fonts are read from disk (the interface's, installed families, CJK and script
/// faces), so the web has only the label face bundled with the app. `room` is what the document
/// can still embed.
pub(super) fn embed(font: &FontRef<'_>, glyphs: &[GlyphId], room: usize) -> Result<Embedded, Refused> {
    let data = font.font_data();
    // The face of the file that draws these glyphs as `font` does (a collection's faces can share
    // glyphs; any that draws them alike will do), at the file's own default instance.
    let index = (0..collection_len(data)).find(|&i| {
        FontRef::try_from_slice_and_index(data, i).is_ok_and(|fresh| {
            fresh.glyph_count() == font.glyph_count()
                && fresh.units_per_em() == font.units_per_em()
                && glyphs.iter().all(|&g| draws_alike(font, &fresh, g))
        })
    });
    let Some(index) = index else {
        return Err(if ab_glyph::VariableFont::variations(font).is_empty() { Refused::NotInFile } else { Refused::Instance });
    };
    let face = ttf_parser::Face::parse(data, index).map_err(|e| Refused::Unreadable(e.to_string()))?;
    if let Some(os2) = face.tables().os2 {
        if os2.permissions() == Some(ttf_parser::Permissions::Restricted) {
            return Err(Refused::Licence("forbids embedding"));
        }
        if !os2.is_subsetting_allowed() {
            return Err(Refused::Licence("forbids subsetting"));
        }
        if !os2.is_outline_embedding_allowed() {
            return Err(Refused::Licence("allows embedding bitmaps only"));
        }
    }
    let mut remapper = subsetter::GlyphRemapper::new();
    for g in glyphs {
        remapper.remap(g.0);
    }
    let subset = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| subsetter::subset(data, index, &remapper)));
    let file = match subset {
        Ok(Ok(file)) => file,
        Ok(Err(e)) => return Err(Refused::Unreadable(e.to_string())),
        Err(_) => return Err(Refused::Unreadable("the subsetter failed".into())),
    };
    if file.len() > MAX_SUBSET {
        return Err(Refused::SubsetTooLarge);
    }
    if file.len() > room {
        return Err(Refused::DocumentTooLarge);
    }
    let k = 1000.0 / f32::from(face.units_per_em().max(1));
    let b = face.global_bounding_box();
    let name = face
        .names()
        .into_iter()
        .filter(|n| n.name_id == ttf_parser::name_id::POST_SCRIPT_NAME)
        .find_map(|n| n.to_string())
        .unwrap_or_default();
    let name: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).take(56).collect();
    let name = if name.is_empty() { "Face".to_string() } else { name };
    Ok(Embedded {
        cff: face.tables().cff.is_some(),
        name: format!("{}+{name}", tag(&name, glyphs)),
        remapper,
        bbox: [f32::from(b.x_min) * k, f32::from(b.y_min) * k, f32::from(b.x_max) * k, f32::from(b.y_max) * k],
        ascent: f32::from(face.ascender()) * k,
        descent: f32::from(face.descender()) * k,
        cap_height: f32::from(face.capital_height().unwrap_or(face.ascender())) * k,
        italic_angle: face.italic_angle(),
        file,
    })
}

/// Whether glyph `g` has the same advance and outline in both faces.
fn draws_alike(a: &impl Font, b: &impl Font, g: GlyphId) -> bool {
    if a.h_advance_unscaled(g).to_bits() != b.h_advance_unscaled(g).to_bits() {
        return false;
    }
    match (a.outline(g), b.outline(g)) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            x.curves.len() == y.curves.len() && x.curves.iter().zip(&y.curves).all(|(p, q)| curve_points(p) == curve_points(q))
        }
        _ => false,
    }
}

/// A curve's kind and points, to compare.
fn curve_points(c: &OutlineCurve) -> (u8, [(f32, f32); 4]) {
    let p = |q: &ab_glyph::Point| (q.x, q.y);
    match c {
        OutlineCurve::Line(a, b) => (0, [p(a), p(b), (0.0, 0.0), (0.0, 0.0)]),
        OutlineCurve::Quad(a, b, c) => (1, [p(a), p(b), p(c), (0.0, 0.0)]),
        OutlineCurve::Cubic(a, b, c, d) => (2, [p(a), p(b), p(c), p(d)]),
    }
}

/// A subset's six-letter tag, from its face's name and glyphs: the same subset, the same tag.
fn tag(name: &str, glyphs: &[GlyphId]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes().chain(glyphs.iter().flat_map(|g| g.0.to_le_bytes())) {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    (0..6).map(|i| char::from(b'A' + ((h >> (i * 5)) % 26) as u8)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::tests::objects;
    use crate::pdf::{Page, Text, write, write_with};
    use crate::vector::{Drawing, Glyph, GlyphRun};
    use tiny_skia::{Color, Transform};

    /// A face of the test's own (egui's bundled Ubuntu Light, which allows embedding), its bytes
    /// changed by `patch` first, parsed and kept.
    fn face(patch: impl FnOnce(&mut Vec<u8>)) -> &'static FontRef<'static> {
        let mut bytes = epaint_default_fonts::UBUNTU_LIGHT.to_vec();
        patch(&mut bytes);
        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        Box::leak(Box::new(FontRef::try_from_slice(bytes).unwrap()))
    }

    /// Where the table `tag` starts in a single font file.
    fn table(bytes: &[u8], tag: &[u8; 4]) -> usize {
        let n = usize::from(u16::from_be_bytes([bytes[4], bytes[5]]));
        let rec = (0..n).map(|i| 12 + 16 * i).find(|&r| &bytes[r..r + 4] == tag).unwrap();
        u32::from_be_bytes(bytes[rec + 8..rec + 12].try_into().unwrap()) as usize
    }

    /// A page with `text` set in `font` twice: on the page (a layout text) and in a group (a
    /// map's label), the second slanted as a synthetic italic is.
    fn page(font: &'static FontRef<'static>, text: &str) -> Page {
        let run = |kx: f32| GlyphRun {
            font,
            size: 20.0,
            color: Color::BLACK,
            transform: Transform::from_row(1.0, 0.0, kx, 1.0, 10.0, 40.0),
            glyphs: text.chars().zip(0..).map(|(ch, i)| Glyph { id: font.glyph_id(ch), x: 11.0 * i as f32, ch }).collect(),
        };
        let mut d = Drawing::new(200.0, 100.0);
        d.glyphs(run(0.0));
        let mut label = Drawing::new(200.0, 100.0);
        label.glyphs(run(-0.2));
        d.place(label, Transform::from_translate(0.0, 40.0), 1.0, rustgis_map::effects::Blend::Normal, None);
        Page::drawing(d, 100.0, 50.0)
    }

    /// Text in fonts is its face embedded as a subset (#1815), a Type 0 font over a TrueType CID
    /// font, the label and the slanted copy set in it too, and its text still read back.
    #[test]
    fn text_in_fonts_is_its_face_embedded() {
        let font = face(|_| ());
        let (pdf, outlined) = write_with(&[page(font, "Ação")], "Fonts", Text::Fonts);
        assert!(outlined.is_empty(), "{outlined:?}");
        let objs = objects(&pdf);
        let dicts: Vec<&str> = objs.iter().map(|o| o.0.as_str()).collect();
        assert!(dicts.iter().any(|d| d.contains("/Subtype /Type0") && d.contains("/Encoding /Identity-H")), "{dicts:?}");
        assert!(dicts.iter().any(|d| d.contains("/Subtype /CIDFontType2") && d.contains("/CIDToGIDMap /Identity")));
        assert!(!dicts.iter().any(|d| d.contains("/Type3")), "no glyph is set in outlines");
        let file = objs.iter().find(|o| o.0.contains("/Length1")).unwrap();
        assert!(file.1.len() < epaint_default_fonts::UBUNTU_LIGHT.len() / 4, "a subset: {} bytes", file.1.len());
        assert!(FontRef::try_from_slice(&file.1).is_ok(), "the subset is a font");
        // Each character back from its CID, and the content setting two-byte codes.
        let cmap = objs.iter().map(|o| String::from_utf8_lossy(&o.1).into_owned()).find(|s| s.contains("<0000> <FFFF>")).unwrap();
        for ch in ['A', 'ç', 'ã', 'o'] {
            assert!(cmap.contains(&format!("> <{:04X}>", ch as u32)), "{ch} in {cmap}");
        }
        let content = objs.iter().map(|o| String::from_utf8_lossy(&o.1).into_owned()).find(|s| s.contains(" TJ")).unwrap();
        assert!(content.contains("[<0001>") && content.contains("<0004>] TJ"), "codes are CIDs: {content}");
        // Each CID is its character's glyph: in the subset (CIDToGIDMap /Identity) the same
        // advance and outline as the face's own glyph, and the same width in /W.
        let subset = FontRef::try_from_slice(&file.1).unwrap();
        let cid_font = objs.iter().find(|o| o.0.contains("/Subtype /CIDFontType2")).map(|o| o.0.as_str()).unwrap();
        let w: Vec<f32> = cid_font
            .split("/W [")
            .nth(1)
            .unwrap()
            .split(" ]")
            .next()
            .unwrap()
            .replace(['[', ']'], " ")
            .split_whitespace()
            .map(|v| v.parse().unwrap())
            .collect();
        let upm = font.units_per_em().unwrap();
        let mut checked = 0;
        for line in cmap.lines().filter(|l| l.starts_with('<') && l.len() == 13 && *l != "<0000> <FFFF>") {
            let cid = u16::from_str_radix(&line[1..5], 16).unwrap();
            let ch = char::from_u32(u32::from_str_radix(&line[8..12], 16).unwrap()).unwrap();
            let (own, sub) = (font.glyph_id(ch), GlyphId(cid));
            assert_eq!(font.h_advance_unscaled(own), subset.h_advance_unscaled(sub), "{ch}: CID {cid}");
            let points = |f: &FontRef, g| f.outline(g).map(|o| o.curves.iter().map(curve_points).collect::<Vec<_>>());
            assert_eq!(points(font, own), points(&subset, sub), "{ch}: CID {cid}'s outline");
            let width =
                w.chunks(2).find(|p| p[0] == f32::from(cid)).map(|p| p[1]).unwrap_or_else(|| panic!("CID {cid} in {w:?}: {cid_font}"));
            assert!((width - font.h_advance_unscaled(own) / upm * 1000.0).abs() < 0.01, "{ch}: /W {width}");
            checked += 1;
        }
        assert_eq!(checked, 4, "A, ç, ã, o");
        // Outlines, as before, whatever the face allows.
        let (outlines, none) = write_with(&[page(font, "Ação")], "Fonts", Text::Outlines);
        assert!(none.is_empty() && outlines == write(&[page(font, "Ação")], "Fonts"));
        let text = String::from_utf8_lossy(&outlines);
        assert!(text.contains("/Subtype /Type3") && !text.contains("/Type0"));
    }

    /// A face whose licence forbids embedding (fsType 2) is set in outlines, and said.
    #[test]
    fn a_face_that_forbids_embedding_is_set_in_outlines() {
        let font = face(|b| {
            let os2 = table(b, b"OS/2");
            b[os2 + 8..os2 + 10].copy_from_slice(&2u16.to_be_bytes());
        });
        let (pdf, outlined) = write_with(&[page(font, "Rio")], "Restricted", Text::Fonts);
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains("/Subtype /Type3") && !text.contains("/Type0"));
        assert_eq!(outlined.len(), 1, "{outlined:?}");
        assert_eq!(outlined[0].face, "Ubuntu Light");
        assert_eq!(outlined[0].why, "its licence forbids embedding");
    }

    /// A face drawn at another instance than its file's default isn't the file's face: embedding
    /// the file would draw it otherwise. A static face (Ubuntu Light) has none, so it is embedded;
    /// a face its file doesn't hold as drawn is refused.
    #[test]
    fn only_the_face_as_drawn_is_embedded() {
        let font = face(|_| ());
        let glyphs: Vec<GlyphId> = "Rio".chars().map(|c| font.glyph_id(c)).collect();
        assert!(embed(font, &glyphs, MAX_EMBEDDED).is_ok());
        // Another file's face, read from this one's bytes, draws its glyphs otherwise.
        let hack = FontRef::try_from_slice(epaint_default_fonts::HACK_REGULAR).unwrap();
        assert!(!draws_alike(font, &hack, glyphs[0]));
        let full = embed(font, &glyphs, 10).err();
        assert_eq!(full, Some(Refused::DocumentTooLarge), "past the room left");
        assert_eq!(full.unwrap().to_string(), "the document's embedded faces would pass 32 MB");
    }

    /// A font file cut short or scrambled anywhere never panics: its face is embedded, or set in
    /// outlines with the reason.
    #[test]
    fn a_hostile_font_file_never_panics() {
        let clean = epaint_default_fonts::UBUNTU_LIGHT;
        let glyf = table(clean, b"glyf");
        let mut tried = 0;
        for step in 0..64usize {
            let mut bytes = clean.to_vec();
            // Scramble a stretch of the glyph outlines, the table directory or the metrics.
            let at = [glyf + step * 97, 12 + step * 3, table(clean, b"hmtx") + step * 13][step % 3];
            for b in bytes.iter_mut().skip(at).take(16) {
                *b = b.wrapping_mul(31).wrapping_add(step as u8);
            }
            if step % 4 == 0 {
                bytes.truncate(clean.len() * (step + 1) / 70);
            }
            let Ok(font) = FontRef::try_from_slice(&bytes) else { continue };
            tried += 1;
            let glyphs: Vec<GlyphId> = (0..40).map(GlyphId).collect();
            let _ = embed(&font, &glyphs, MAX_EMBEDDED);
            let _ = face_name(&font);
        }
        assert!(tried > 10, "{tried} scrambled files parsed");
    }
}
