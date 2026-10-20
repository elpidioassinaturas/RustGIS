//! Font discovery: a good installed sans-serif when there is one (Segoe UI, SF, Inter, Noto Sans,
//! Open Sans, DejaVu Sans), else the Ubuntu Light face that ships with egui. Fonts are read at
//! run time and never bundled.

use std::sync::OnceLock;

/// Candidate files for (regular, semibold), in order of preference.
const CANDIDATES: &[(&str, &str)] = &[
    ("C:\\Windows\\Fonts\\segoeui.ttf", "C:\\Windows\\Fonts\\seguisb.ttf"),
    ("/System/Library/Fonts/SFNS.ttf", "/System/Library/Fonts/SFNS.ttf"),
    ("/System/Library/Fonts/Supplemental/Arial.ttf", "/System/Library/Fonts/Supplemental/Arial Bold.ttf"),
    ("/usr/share/fonts/TTF/Inter-Regular.ttf", "/usr/share/fonts/TTF/Inter-SemiBold.ttf"),
    ("/usr/share/fonts/inter/Inter-Regular.ttf", "/usr/share/fonts/inter/Inter-SemiBold.ttf"),
    ("/usr/share/fonts/noto/NotoSans-Regular.ttf", "/usr/share/fonts/noto/NotoSans-SemiBold.ttf"),
    ("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf"),
    ("/usr/share/fonts/google-noto/NotoSans-Regular.ttf", "/usr/share/fonts/google-noto/NotoSans-SemiBold.ttf"),
    ("/usr/share/fonts/TTF/OpenSans-Regular.ttf", "/usr/share/fonts/TTF/OpenSans-SemiBold.ttf"),
    ("/usr/share/fonts/truetype/open-sans/OpenSans-Regular.ttf", "/usr/share/fonts/truetype/open-sans/OpenSans-SemiBold.ttf"),
    ("/usr/share/fonts/TTF/DejaVuSans.ttf", "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf"),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"),
];

/// Largest font file accepted (32 MiB).
const MAX_FONT: u64 = 32 << 20;

pub struct UiFonts {
    pub regular: Vec<u8>,
    pub semibold: Vec<u8>,
    /// File the regular face came from, or "built-in".
    pub source: String,
}

fn read(path: &str) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FONT {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    ab_glyph::FontRef::try_from_slice(&bytes).ok()?;
    Some(bytes)
}

/// The weight the semibold face is drawn at where it is a variable font (#591): macOS's system
/// font is one file for every weight, drawn at its default 400 unless told otherwise. 600 is
/// semibold, as the static faces used elsewhere (Segoe UI Semibold, Inter SemiBold) are; a face
/// with no weight axis ignores it. Both renderers set it, so text is as heavy on screen as in an
/// export.
pub const SEMIBOLD_WEIGHT: f32 = 600.0;

/// The interface fonts, discovered once.
pub fn ui_fonts() -> &'static UiFonts {
    static F: OnceLock<UiFonts> = OnceLock::new();
    F.get_or_init(|| {
        if std::env::var_os("RUSTGIS_BUILTIN_FONTS").is_none() {
            for (r, b) in CANDIDATES {
                if let Some(regular) = read(r) {
                    let semibold = read(b).unwrap_or_else(|| regular.clone());
                    return UiFonts { regular, semibold, source: (*r).to_string() };
                }
            }
        }
        let b = epaint_default_fonts::UBUNTU_LIGHT.to_vec();
        UiFonts { regular: b.clone(), semibold: b, source: "built-in".into() }
    })
}

/// Regional CJK glyph styles (the same code point is drawn differently by region).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CjkRegion {
    SimplifiedChinese,
    Japanese,
    Korean,
}

/// Installed CJK font candidates: (path, face index in a collection).
fn cjk_candidates(region: CjkRegion) -> &'static [(&'static str, u32)] {
    // Noto Sans CJK collections order their faces JP, KR, SC, TC, HK.
    match region {
        CjkRegion::SimplifiedChinese => &[
            ("C:\\Windows\\Fonts\\msyh.ttc", 0),
            ("/System/Library/Fonts/PingFang.ttc", 0),
            ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
            ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 2),
            ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 2),
            ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 2),
            ("/usr/share/fonts/noto-cjk/NotoSansSC-Regular.otf", 0),
            ("/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc", 0),
        ],
        CjkRegion::Japanese => &[
            ("C:\\Windows\\Fonts\\YuGothR.ttc", 0),
            ("C:\\Windows\\Fonts\\meiryo.ttc", 0),
            ("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", 0),
            ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),
            ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
            ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 0),
            ("/usr/share/fonts/noto-cjk/NotoSansJP-Regular.otf", 0),
        ],
        CjkRegion::Korean => &[
            ("C:\\Windows\\Fonts\\malgun.ttf", 0),
            ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
            ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 1),
            ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 1),
            ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 1),
            ("/usr/share/fonts/noto-cjk/NotoSansKR-Regular.otf", 0),
        ],
    }
}

/// An installed CJK font for `region` (bytes and face index), found once. `None` on the web or
/// when nothing suitable is installed.
pub fn cjk_font(region: CjkRegion) -> Option<&'static (Vec<u8>, u32)> {
    static FOUND: [OnceLock<Option<(Vec<u8>, u32)>>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let slot = match region {
        CjkRegion::SimplifiedChinese => &FOUND[0],
        CjkRegion::Japanese => &FOUND[1],
        CjkRegion::Korean => &FOUND[2],
    };
    slot.get_or_init(|| {
        cjk_candidates(region).iter().find_map(|(path, index)| {
            let meta = std::fs::metadata(path).ok()?;
            // CJK collections are large; accept up to 128 MiB.
            if meta.len() > 128 << 20 {
                return None;
            }
            std::fs::read(path).ok().filter(|b| valid_font(b, *index)).map(|b| (b, *index))
        })
    })
    .as_ref()
}

/// Whether `bytes` hold a font with face `index` that the font parser accepts. Fonts handed to the
/// UI are checked first: a corrupt file or an out-of-range face index must never reach it.
pub fn valid_font(bytes: &[u8], index: u32) -> bool {
    ab_glyph::FontRef::try_from_slice_and_index(bytes, index).is_ok()
}

/// Where the web app downloads a CJK face from (Noto Sans, SIL Open Font License).
pub fn cjk_font_url(region: CjkRegion) -> &'static str {
    match region {
        CjkRegion::SimplifiedChinese => "https://assets.rustgis.app/fonts/NotoSansSC-Regular.otf",
        CjkRegion::Japanese => "https://assets.rustgis.app/fonts/NotoSansJP-Regular.otf",
        CjkRegion::Korean => "https://assets.rustgis.app/fonts/NotoSansKR-Regular.otf",
    }
}

/// A script the interface fonts may lack, and that an interface language needs a face for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptFont {
    Cjk(CjkRegion),
    /// Arabic script (Persian). The face has Latin letters too: egui shapes each run of one face
    /// on its own, so a line of Persian with a Latin word in it reads in order only when one face
    /// draws the whole line.
    Arabic,
}

/// Installed faces with both Arabic and Latin letters (checked by [`covers_arabic`]).
const ARABIC_CANDIDATES: &[&str] = &[
    "C:\\Windows\\Fonts\\segoeui.ttf",
    "C:\\Windows\\Fonts\\tahoma.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/System/Library/Fonts/Supplemental/Tahoma.ttf",
    "/usr/share/fonts/TTF/Vazirmatn-Regular.ttf",
    "/usr/share/fonts/truetype/vazirmatn/Vazirmatn-Regular.ttf",
    "/usr/share/fonts/vazirmatn-fonts/Vazirmatn-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
];

/// Whether a face draws Persian letters (farsi yeh, gaf) and Latin ones.
fn covers_arabic(bytes: &[u8], index: u32) -> bool {
    use ab_glyph::Font;
    ab_glyph::FontRef::try_from_slice_and_index(bytes, index)
        .is_ok_and(|f| ['\u{6CC}', '\u{6AF}', 'A', 'z'].iter().all(|c| f.glyph_id(*c).0 != 0))
}

/// An installed face for `script` (bytes and face index), found once. `None` on the web or when
/// nothing suitable is installed.
pub fn script_font(script: ScriptFont) -> Option<&'static (Vec<u8>, u32)> {
    match script {
        ScriptFont::Cjk(region) => cjk_font(region),
        ScriptFont::Arabic => {
            static FOUND: OnceLock<Option<(Vec<u8>, u32)>> = OnceLock::new();
            FOUND.get_or_init(|| ARABIC_CANDIDATES.iter().find_map(|p| read(p).filter(|b| covers_arabic(b, 0)).map(|b| (b, 0)))).as_ref()
        }
    }
}

/// Where the web app downloads a face for `script` from (SIL Open Font License): Noto Sans for
/// CJK, Vazirmatn for Arabic script.
pub fn script_font_url(script: ScriptFont) -> &'static str {
    match script {
        ScriptFont::Cjk(region) => cjk_font_url(region),
        ScriptFont::Arabic => "https://assets.rustgis.app/fonts/Vazirmatn-Regular.ttf",
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_arabic_face_needs_persian_and_latin_letters() {
        // Ubuntu Light has Latin but no Arabic; junk is no font at all.
        assert!(!super::covers_arabic(epaint_default_fonts::UBUNTU_LIGHT, 0));
        assert!(!super::covers_arabic(b"not a font", 0));
        // DejaVu Sans has both, where it is installed.
        for p in super::ARABIC_CANDIDATES.iter().filter(|p| p.contains("DejaVu")) {
            if let Ok(b) = std::fs::read(p) {
                assert!(super::covers_arabic(&b, 0), "{p}");
            }
        }
    }
}
