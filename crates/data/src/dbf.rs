//! Shared Shapefile character encoding selection for whole-file and streamed reads.

use encoding_rs as rs;
use shapefile::dbase::{CodePageMark, UnicodeLossy, encoding::DynEncoding};

const MAX_CPG_BYTES: usize = 1024;

/// Keep the selection thread-safe; dbase's dynamic decoder is created per read because it
/// is `Send` but not `Sync`, whereas feature sources can be shared between readers.
pub(crate) enum DbfEncoding {
    Standard(&'static rs::Encoding),
    Oem(CodePageMark),
    Utf8,
}

impl DbfEncoding {
    pub(crate) fn decoder(&self) -> DynEncoding {
        match self {
            Self::Standard(encoding) => shapefile::dbase::encoding::EncodingRs::from(*encoding).into(),
            Self::Oem(mark) => mark.to_encoding().unwrap_or_else(|| UnicodeLossy.into()),
            Self::Utf8 => UnicodeLossy.into(),
        }
    }

    fn from_number(number: u16) -> Option<Self> {
        let encoding = match number {
            437 => return Some(Self::Oem(CodePageMark::CP437)),
            850 => return Some(Self::Oem(CodePageMark::CP850)),
            852 => return Some(Self::Oem(CodePageMark::CP852)),
            861 => return Some(Self::Oem(CodePageMark::CP861)),
            865 => return Some(Self::Oem(CodePageMark::CP865)),
            866 => rs::IBM866,
            874 => rs::WINDOWS_874,
            932 => rs::SHIFT_JIS,
            936 => rs::GBK,
            949 => rs::EUC_KR,
            950 => rs::BIG5,
            1250..=1258 => return Self::from_label(&format!("windows-{number}")),
            65001 => rs::UTF_8,
            _ => return None,
        };
        Some(Self::Standard(encoding))
    }

    fn from_label(label: &str) -> Option<Self> {
        // dbase's name lookup with both decoder features enabled falls back to CP1252
        // for unknown labels. Resolve explicitly so unusable metadata is reported.
        let encoding = rs::Encoding::for_label(label.as_bytes())?;
        // UTF-16 and replacement encodings cannot represent DBF's ASCII padding and
        // NUL-terminated field names. Treat these labels as unsupported metadata.
        encoding.is_ascii_compatible().then_some(Self::Standard(encoding))
    }

    fn from_name(name: &str) -> Option<Self> {
        let upper = name.to_ascii_uppercase();
        if let Some(suffix) = upper.strip_prefix("8859") {
            return Self::from_label(&format!("iso-8859-{}", suffix.strip_prefix('-').unwrap_or(suffix)));
        }
        let prefixed = ["CP", "IBM", "OEM", "WINDOWS", "ANSI"].into_iter().find_map(|prefix| upper.strip_prefix(prefix));
        let number = match prefixed {
            Some(suffix) => suffix.strip_prefix('-').unwrap_or(suffix).trim(),
            None => &upper,
        }
        .parse::<u16>();
        match number {
            Ok(number) => Self::from_number(number),
            Err(_) => Self::from_label(&upper),
        }
    }

    fn from_mark(mark: CodePageMark) -> Option<Self> {
        // Keep encoding_rs for its supported pages: dbase's yore adapter is strict for
        // those encodings, which can discard records containing malformed text.
        let encoding = match mark {
            CodePageMark::CP437 | CodePageMark::CP850 | CodePageMark::CP852 | CodePageMark::CP861 | CodePageMark::CP865 => {
                return Some(Self::Oem(mark));
            }
            CodePageMark::CP1250 => rs::WINDOWS_1250,
            CodePageMark::CP1251 => rs::WINDOWS_1251,
            CodePageMark::CP1252 => rs::WINDOWS_1252,
            CodePageMark::CP1253 => rs::WINDOWS_1253,
            CodePageMark::CP1254 => rs::WINDOWS_1254,
            CodePageMark::CP1255 => rs::WINDOWS_1255,
            CodePageMark::CP1256 => rs::WINDOWS_1256,
            CodePageMark::CP866 => rs::IBM866,
            CodePageMark::CP874 => rs::WINDOWS_874,
            CodePageMark::CP932 => rs::SHIFT_JIS,
            CodePageMark::CP936 => rs::GBK,
            CodePageMark::CP949 => rs::EUC_KR,
            CodePageMark::CP950 => rs::BIG5,
            CodePageMark::Utf8 => rs::UTF_8,
            CodePageMark::Undefined => return Some(Self::Utf8),
            _ => return None,
        };
        Some(Self::Standard(encoding))
    }
}

pub(crate) fn encoding(cpg: Option<&[u8]>, ldid: u8, notes: &mut Vec<String>) -> DbfEncoding {
    if let Some(bytes) = cpg {
        let label = if bytes.len() <= MAX_CPG_BYTES { std::str::from_utf8(bytes).unwrap_or("") } else { "" };
        let label = label.trim().trim_start_matches('\u{feff}').trim();
        if let Some(encoding) = DbfEncoding::from_name(label) {
            return encoding;
        }
        notes.push("Shapefile: unsupported or empty .cpg; using the .dbf code page, or UTF-8 if unavailable".into());
    }
    // Esri's LDID table assigns 0x57 to ANSI 1252 (ArcPad, "Setting the code page
    // for shapefiles"); dbase does not recognize that widely used marker.
    let mark = if ldid == 0x57 { CodePageMark::CP1252 } else { CodePageMark::from(ldid) };
    if let Some(encoding) = DbfEncoding::from_mark(mark) {
        return encoding;
    }
    notes.push(format!("Shapefile: unsupported .dbf code page 0x{ldid:02X}; using UTF-8"));
    DbfEncoding::Utf8
}

/// A code-page label should be a few bytes. Bound reads even for a hostile sidecar.
pub(crate) fn read_cpg(path: &std::path::Path, notes: &mut Vec<String>) -> Option<Vec<u8>> {
    match try_read_cpg(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            notes.push(format!("Shapefile: could not read .cpg ({e}); using the .dbf code page, or UTF-8 if unavailable"));
            None
        }
    }
}

fn try_read_cpg(path: &std::path::Path) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    // Check every ASCII case combination without scanning a potentially large directory.
    // Prefer the conventional spellings if multiple sidecars exist.
    for ext in ["cpg", "CPG", "Cpg", "cPg", "cpG", "CPg", "CpG", "cPG"] {
        match std::fs::File::open(path.with_extension(ext)) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(MAX_CPG_BYTES as u64 + 1).read_to_end(&mut bytes)?;
                return Ok(Some(bytes));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}
