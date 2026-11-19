//! GRIB files (`.grb`, `.grib`, `.grb2`, `.grib2`): weather and climate model output, read
//! without GDAL. Each field is a band.
//!
//! Read: editions 1 and 2, simple packing (GRIB1's grid point data with simple packing, GRIB2's
//! data representation template 5.0), on a regular latitude/longitude grid (GRIB1 data
//! representation type 0, GRIB2 grid definition template 3.0), in any scanning order, with or
//! without a bitmap. Following WMO's Manual on Codes (WMO-No. 306), Volume I.2, FM 92 GRIB.
//!
//! Not read yet, and said: complex, JPEG 2000, PNG and the other packings; other grids (Gaussian,
//! rotated, projected, quasi-regular). A message the reader can't read is left out with a notice
//! naming why; a file with nothing it can read is an error naming why.
//!
//! The fields on the grid of the first one read are the raster's bands, in file order; fields on
//! another grid are left out with a notice. Values are `(R + X × 2^E) / 10^D`, in the units the
//! file stores (kelvin stays kelvin), unpacked in `f64`; those `f32` changes are counted in
//! [`Raster::rounded`]. Points a bitmap masks are NaN. Rows are laid out north up,
//! and a grid round the whole globe runs from −180° (as GDAL lays it out).

use super::{Crs, GeoTransform, Raster, max_samples};
use crate::{DataError, Result};
use std::io::{Read, Seek, SeekFrom};

/// File extensions read as GRIB.
pub const EXTENSIONS: &[&str] = &["grb", "grib", "grb2", "grib2"];

/// Whether a file name is a GRIB file's.
pub fn is_grib_name(name: &str) -> bool {
    let ext = std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    EXTENSIONS.contains(&ext.as_str())
}

/// The bytes read at a time while looking for the next message ("GRIB"): files may put a
/// bulletin header or padding between messages.
const CHUNK: usize = 4 * 1024;

/// The most bits a packed value may have: GRIB allows more, no producer writes them.
const MAX_BITS: u32 = 32;

/// The smallest a message can be: GRIB1's indicator, its shortest sections and "7777".
const MIN_MESSAGE: u64 = 8 + 28 + 11 + 4;

fn bad(m: impl std::fmt::Display) -> DataError {
    DataError::Format(format!("GRIB: {m}"))
}

/// Reads a GRIB file's bytes (an upload, a download, a byte range of a larger file).
pub fn read_bytes(bytes: &[u8], name: &str) -> Result<Raster> {
    read(std::io::Cursor::new(bytes), name)
}

/// Reads a GRIB file: its fields on the first readable one's grid, as bands.
pub fn read<R: Read + Seek>(mut source: R, name: &str) -> Result<Raster> {
    let file_len = source.seek(SeekFrom::End(0))?;
    source.seek(SeekFrom::Start(0))?;
    let limit = max_samples();
    let mut fields: Vec<Field> = Vec::new();
    let mut skipped = Skipped::default();
    let mut at = 0u64;
    let mut messages = 0usize;
    let mut held = 0usize;
    let mut chunk = vec![0u8; CHUNK];
    // Progress in KiB of the file, counted as the search for each message reads it and as each
    // message is read; one message is decoded without a check (at most `max_samples` values).
    crate::progress::begin(usize::try_from(file_len / 1024).unwrap_or(usize::MAX));
    // Whether the bytes before the next "GRIB" are the rest of a message left out (whose length
    // isn't known), rather than something outside the messages.
    let mut in_skipped = false;
    loop {
        let (found, junk) = find_message(&mut source, at, file_len, &mut chunk)?;
        if !in_skipped {
            skipped.junk += junk;
        }
        let Some(start) = found else { break };
        let (len, edition) = message_header(&mut source, start)?;
        in_skipped = false;
        match (edition, len) {
            // Not a message GRIB 1 or 2 describes: its four letters are passed, and the search
            // goes on.
            (Some(e), _) if e != 1 && e != 2 => {
                skipped.message(format!("not GRIB 1 or 2 (edition {e})"));
                (at, in_skipped) = (start + 4, true);
                continue;
            }
            (Some(_), len) if len < MIN_MESSAGE => {
                skipped.message("damaged".to_string());
                (at, in_skipped) = (start + 4, true);
                continue;
            }
            // A message cut short (a byte range that ends early, a truncated download): what was
            // read before it stands, and the cut is said.
            (None, _) => {
                skipped.cut += 1;
                break;
            }
            (Some(_), len) if start.saturating_add(len) > file_len => {
                skipped.cut += 1;
                break;
            }
            _ => {}
        }
        messages += 1;
        at = start + len;
        crate::progress::step(usize::try_from(len / 1024).unwrap_or(0))?;
        // Past the memory limit, or a message larger than the values left room for can take (at
        // most 32 bits each, a bitmap and a megabyte of sections): only counted, never read.
        let room = limit.saturating_sub(held);
        if held >= limit || len > (room as u64).saturating_mul(5).saturating_add(1 << 20) {
            skipped.over_limit += count_fields(&mut source, start, len, edition == Some(2))?;
            continue;
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len as usize).map_err(|_| bad("not enough memory for a message"))?;
        source.seek(SeekFrom::Start(start))?;
        source.by_ref().take(len).read_to_end(&mut bytes)?;
        let decoded = if edition == Some(1) { vec![grib1(&bytes, room)] } else { grib2(&bytes, room) };
        for d in decoded {
            match d {
                Ok(f) => {
                    let first = fields.first().map(|f| f.grid);
                    if first.is_some_and(|g| !g.same(&f.grid)) {
                        skipped.other_grid += 1;
                    } else if held + f.values.len() > limit {
                        skipped.over_limit += 1;
                    } else {
                        held += f.values.len();
                        fields.push(f);
                    }
                }
                Err(Skip::TooLarge) => skipped.over_limit += 1,
                Err(Skip::Other(why)) => skipped.add(why),
            }
        }
    }
    if messages == 0 && skipped.messages.is_empty() {
        return Err(bad(if skipped.cut > 0 { "the first message is cut short" } else { "no GRIB message in the file" }));
    }
    let Some(first) = fields.first() else {
        return Err(bad(format!("nothing in it can be read here: {}", skipped.notes(limit).join("; "))));
    };
    let grid = first.grid;
    let mut r = to_raster(fields, name)?;
    r.lost.extend(grid.notes());
    r.lost.extend(skipped.notes(limit));
    Ok(r)
}

/// The offset of the next "GRIB" at or after `at` (`None` at the end of the file), and how many
/// of the bytes passed on the way aren't zeros (padding): a bulletin header, other data.
fn find_message<R: Read + Seek>(source: &mut R, at: u64, file_len: u64, chunk: &mut [u8]) -> Result<(Option<u64>, u64)> {
    let mut from = at;
    let mut junk = 0u64;
    while from < file_len {
        source.seek(SeekFrom::Start(from))?;
        let n = usize::try_from(file_len - from).unwrap_or(usize::MAX).min(chunk.len());
        let window = chunk.get_mut(..n).unwrap_or_default();
        source.read_exact(window)?;
        // Each window looks at Cancel, and counts the bytes it passes (a message's own are counted
        // as it is read): a long run of other bytes can be stopped.
        if let Some(p) = window.windows(4).position(|w| w == b"GRIB") {
            crate::progress::step(p / 1024)?;
            junk += window.iter().take(p).filter(|b| **b != 0).count() as u64;
            return Ok((Some(from + p as u64), junk));
        }
        // The last three bytes may start a "GRIB" that the next chunk ends.
        let keep = if from + n as u64 >= file_len { 0 } else { 3.min(n) };
        crate::progress::step((n - keep) / 1024)?;
        junk += window.iter().take(n - keep).filter(|b| **b != 0).count() as u64;
        from += (n - keep).max(1) as u64;
    }
    Ok((None, junk))
}

/// A message's length and edition, from its indicator section: no edition when the file ends
/// inside the section, a length of 0 for an edition other than 1 and 2.
fn message_header<R: Read + Seek>(source: &mut R, start: u64) -> Result<(u64, Option<u8>)> {
    source.seek(SeekFrom::Start(start))?;
    let mut head = [0u8; 16];
    let mut got = 0;
    while got < head.len() {
        match source.read(head.get_mut(got..).unwrap_or_default())? {
            0 => break,
            n => got += n,
        }
    }
    if got < 8 {
        return Ok((0, None));
    }
    let edition = head[7];
    let len = match edition {
        1 => u64::from(be(&head, 4, 3)),
        2 if got == 16 => head[8..16].iter().fold(0u64, |v, &b| (v << 8) | u64::from(b)),
        2 => return Ok((0, None)),
        _ => 0,
    };
    Ok((len, Some(edition)))
}

/// How many fields a message holds: one in GRIB1, one a data section (7) in GRIB2, counted from
/// the sections' headers alone.
fn count_fields<R: Read + Seek>(source: &mut R, start: u64, len: u64, grib2: bool) -> Result<usize> {
    if !grib2 {
        return Ok(1);
    }
    let (mut at, end, mut n) = (start + 16, start + len, 0usize);
    while at + 5 <= end {
        source.seek(SeekFrom::Start(at))?;
        let mut head = [0u8; 5];
        source.read_exact(&mut head)?;
        if head.starts_with(b"7777") {
            break;
        }
        let size = u64::from(be(&head, 0, 4));
        if size < 5 {
            break;
        }
        n += usize::from(head[4] == 7);
        at += size;
    }
    Ok(n.max(1))
}

/// Why a field isn't read: past the memory limit, or another reason said once with a count.
enum Skip {
    TooLarge,
    Other(String),
}

/// What was left out of a file, for its notices.
#[derive(Default)]
struct Skipped {
    /// Each reason, with how many fields it left out, in the order first met.
    reasons: Vec<(String, usize)>,
    /// Each reason a whole message was left out (another edition, damaged), with a count.
    messages: Vec<(String, usize)>,
    other_grid: usize,
    over_limit: usize,
    cut: usize,
    /// Bytes outside every message that aren't zeros.
    junk: u64,
}

impl Skipped {
    fn add(&mut self, why: String) {
        tally(&mut self.reasons, why);
    }

    fn message(&mut self, why: String) {
        tally(&mut self.messages, why);
    }

    fn notes(&self, limit: usize) -> Vec<String> {
        let mut out: Vec<String> = self.reasons.iter().map(|(r, n)| format!("{} left out: {r}, not read yet", count(*n))).collect();
        out.extend(self.messages.iter().map(|(r, n)| {
            let what = if *n == 1 { "1 message".to_string() } else { format!("{n} messages") };
            format!("{what} left out: {r}")
        }));
        if self.other_grid > 0 {
            out.push(format!("{} left out: on another grid than the first field's", count(self.other_grid)));
        }
        if self.over_limit > 0 {
            out.push(format!(
                "{} left out: the bands read hold the most this computer keeps in memory ({limit} values); open a smaller part of the file",
                count(self.over_limit)
            ));
        }
        if self.cut > 0 {
            out.push("the file ends inside a message, which is left out".to_string());
        }
        if self.junk > 0 {
            out.push(format!("{} bytes outside its messages aren't GRIB, and are passed over", self.junk));
        }
        out
    }
}

/// One more of `why` in `list`.
fn tally(list: &mut Vec<(String, usize)>, why: String) {
    match list.iter_mut().find(|(r, _)| *r == why) {
        Some((_, n)) => *n += 1,
        None => list.push((why, 1)),
    }
}

fn count(n: usize) -> String {
    if n == 1 { "1 field".to_string() } else { format!("{n} fields") }
}

/// A regular latitude/longitude grid, as the file gives it: degrees of the first and last points'
/// centres, the increments, and the scanning mode (WMO flag table 3.4).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Grid {
    ni: usize,
    nj: usize,
    lat1: f64,
    lon1: f64,
    lat2: f64,
    lon2: f64,
    dx: f64,
    dy: f64,
    scan: u8,
    /// The earth's shape when it isn't WGS 84's (or near enough): its description.
    earth: Option<&'static str>,
}

const SCAN_I_NEGATIVE: u8 = 0x80;
const SCAN_J_POSITIVE: u8 = 0x40;
const SCAN_J_CONSECUTIVE: u8 = 0x20;
const SCAN_ALTERNATE: u8 = 0x10;

impl Grid {
    fn same(&self, o: &Grid) -> bool {
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-6;
        self.ni == o.ni
            && self.nj == o.nj
            && close(self.lat1, o.lat1)
            && close(self.lon1, o.lon1)
            && close(self.dx, o.dx)
            && close(self.dy, o.dy)
            && self.scan == o.scan
    }

    fn goes_north(&self) -> bool {
        self.scan & SCAN_J_POSITIVE != 0
    }

    /// Whether the first and last latitudes run against the scanning mode (CPTEC's BAM files
    /// give −90° first and +90° last, the grid's edges, and scan north to south): rows then follow
    /// the scanning mode, centred between the two latitudes.
    fn latitudes_disagree(&self) -> bool {
        self.nj > 1 && self.lat1 != self.lat2 && (self.lat2 > self.lat1) != self.goes_north()
    }

    /// The centre latitude of the northernmost row.
    fn north(&self) -> f64 {
        if self.latitudes_disagree() {
            (self.lat1 + self.lat2) / 2.0 + (self.nj.saturating_sub(1)) as f64 * self.dy / 2.0
        } else if self.goes_north() {
            self.lat1 + (self.nj.saturating_sub(1)) as f64 * self.dy
        } else {
            self.lat1
        }
    }

    /// The centre longitude of the westernmost column, in −180..180 (a grid given from 0 to 360°
    /// east of 180° is moved west of the antimeridian, as GDAL moves one).
    fn west(&self) -> f64 {
        let w = if self.scan & SCAN_I_NEGATIVE != 0 { self.lon1 - (self.ni.saturating_sub(1)) as f64 * self.dx } else { self.lon1 };
        let w = w.rem_euclid(360.0);
        if w >= 180.0 { w - 360.0 } else { w }
    }

    /// Whether the columns go round the whole globe.
    fn whole_turn(&self) -> bool {
        (self.ni as f64 * self.dx - 360.0).abs() < self.dx * 0.5
    }

    /// The column and row (north up) of the `k`th value in scanning order.
    fn place(&self, k: usize) -> (usize, usize) {
        let (i, j) = if self.scan & SCAN_J_CONSECUTIVE != 0 { (k / self.nj, k % self.nj) } else { (k % self.ni, k / self.ni) };
        let alternate = self.scan & SCAN_ALTERNATE != 0;
        let (i, j) = if alternate && self.scan & SCAN_J_CONSECUTIVE == 0 && j % 2 == 1 {
            (self.ni - 1 - i, j)
        } else if alternate && self.scan & SCAN_J_CONSECUTIVE != 0 && i % 2 == 1 {
            (i, self.nj - 1 - j)
        } else {
            (i, j)
        };
        let col = if self.scan & SCAN_I_NEGATIVE != 0 { self.ni - 1 - i } else { i };
        let row = if self.goes_north() { self.nj - 1 - j } else { j };
        (col, row)
    }

    fn notes(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.latitudes_disagree() {
            let north = self.north();
            out.push(format!(
                "the grid gives its first row at {}° and its last at {}° but scans {}: its {} rows of {}° are laid out as the scanning mode says, centred between the two, from {}° to {}°",
                self.lat1,
                self.lat2,
                if self.goes_north() { "south to north" } else { "north to south" },
                self.nj,
                self.dy,
                round6(north),
                round6(north - (self.nj.saturating_sub(1)) as f64 * self.dy),
            ));
        }
        if let Some(earth) = self.earth {
            out.push(format!("its earth is {earth}; latitudes and longitudes are read as WGS 84's"));
        }
        out
    }
}

/// `v` to a millionth, for a notice (89.91, not 89.91000000000001).
fn round6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

/// One field: its band name, grid and values in scanning order (NaN where a bitmap masks).
struct Field {
    name: String,
    grid: Grid,
    values: Vec<f32>,
    /// Values `f32` changed: decimals (`D` > 0) and sums with more digits than it holds.
    rounded: u64,
}

/// The raster of fields on one grid: north up, a whole turn of longitude from −180°.
fn to_raster(fields: Vec<Field>, name: &str) -> Result<Raster> {
    let Some(grid) = fields.first().map(|f| f.grid) else { return Err(bad("no field")) };
    let (w, h) = (grid.ni, grid.nj);
    let west = grid.west();
    // A whole turn starting east of −180°: the column at 180° moves to the first, so the columns
    // run from −180°.
    let roll = if grid.whole_turn() && west > -180.0 + grid.dx * 0.5 {
        let at_180 = ((180.0 - west) / grid.dx).round() as usize % w.max(1);
        (w - at_180) % w.max(1)
    } else {
        0
    };
    let origin_x = if roll > 0 { west - (roll as f64) * grid.dx } else { west } - grid.dx / 2.0;
    let origin_y = grid.north() + grid.dy / 2.0;
    let mut names = Vec::with_capacity(fields.len());
    let mut rounded = Vec::with_capacity(fields.len());
    let mut bands = Vec::with_capacity(fields.len());
    for f in fields {
        let mut band = super::filled(w, h, f32::NAN)?;
        for (k, v) in f.values.iter().enumerate() {
            let (col, row) = grid.place(k);
            let col = (col + roll) % w.max(1);
            if let Some(cell) = band.get_mut(row * w + col) {
                *cell = *v;
            }
        }
        names.push(f.name);
        rounded.push(f.rounded);
        bands.push(band);
    }
    let size = |n: usize| u32::try_from(n).map_err(|_| bad("the grid is too large"));
    let transform = GeoTransform { origin_x, origin_y, pixel_width: grid.dx, pixel_height: -grid.dy };
    let mut r = Raster::new(name, size(w)?, size(h)?, bands, transform, Crs::LonLat, None, Default::default(), "Float32")?;
    for (b, n) in r.bands.iter_mut().zip(names) {
        b.name = n;
    }
    if rounded.iter().any(|n| *n > 0) {
        r.rounded = rounded;
    }
    Ok(r)
}

/// Octets `o..o + n` (0-based offset `o`) of `b`, unsigned, big-endian; 0 past the end.
fn be(b: &[u8], o: usize, n: usize) -> u32 {
    b.get(o..o + n).map_or(0, |s| s.iter().fold(0u32, |v, &x| (v << 8) | u32::from(x)))
}

/// Octets `o..o + n` of `b`, a sign bit and magnitude (GRIB's signed integers).
fn signed(b: &[u8], o: usize, n: usize) -> i64 {
    let v = i64::from(be(b, o, n));
    let sign = 1i64 << (8 * n - 1);
    if v & sign != 0 { -(v & (sign - 1)) } else { v }
}

/// An IBM System/360 single-precision number (GRIB1's reference value).
fn ibm(v: u32) -> f64 {
    let sign = if v & 0x8000_0000 != 0 { -1.0 } else { 1.0 };
    let exponent = ((v >> 24) & 0x7f) as i32 - 64;
    let mantissa = f64::from(v & 0x00ff_ffff) / f64::from(1u32 << 24);
    sign * mantissa * 16f64.powi(exponent)
}

/// `count` values of `bits` bits each from `data`, big-endian, unpacked as `(R + X × 2^E) / 10^D`;
/// with fewer bits there than that, an error.
fn unpack(data: &[u8], count: usize, bits: u32, r: f64, e: i64, d: i64) -> std::result::Result<(Vec<f64>, u64), Skip> {
    if bits > MAX_BITS {
        return Err(Skip::Other(format!("values of {bits} bits")));
    }
    let need = (count as u64).saturating_mul(u64::from(bits)).div_ceil(8);
    if (data.len() as u64) < need {
        return Err(Skip::Other("a data section shorter than its values".to_string()));
    }
    let scale = 2f64.powi(e.clamp(-1000, 1000) as i32);
    let ten = 10f64.powi(d.clamp(-1000, 1000) as i32);
    let mut out = Vec::new();
    out.try_reserve_exact(count).map_err(|_| Skip::TooLarge)?;
    let mut rounded = 0u64;
    let mut bit = 0u64;
    for _ in 0..count {
        // The 8 octets from the value's first: at most 7 bits before it, and its 32 at most.
        let byte = (bit / 8) as usize;
        let word = (0..8).fold(0u64, |w, i| (w << 8) | u64::from(data.get(byte + i).copied().unwrap_or(0)));
        let x = (word << (bit % 8)) >> (64 - bits);
        bit += u64::from(bits);
        let v = (r + x as f64 * scale) / ten;
        if f64::from(v as f32) != v {
            rounded += 1;
        }
        out.push(v);
    }
    Ok((out, rounded))
}

/// `count` copies of `v`, and how many of them `f32` changes: a field packed in no bits holds its
/// reference value everywhere.
fn constant(v: f64, count: usize) -> std::result::Result<(Vec<f64>, u64), Skip> {
    let mut out = Vec::new();
    out.try_reserve_exact(count).map_err(|_| Skip::TooLarge)?;
    out.resize(count, v);
    Ok((out, if f64::from(v as f32) != v { count as u64 } else { 0 }))
}

/// Values in scanning order: those unpacked, spread over the points a bitmap keeps (NaN at the
/// others), or all of them without one.
fn spread(values: Vec<f64>, bitmap: Option<&[u8]>, points: usize) -> std::result::Result<Vec<f32>, Skip> {
    let mut out = Vec::new();
    out.try_reserve_exact(points).map_err(|_| Skip::TooLarge)?;
    match bitmap {
        None => {
            if values.len() != points {
                return Err(Skip::Other("a field whose values don't fill its grid".to_string()));
            }
            out.extend(values.into_iter().map(|v| v as f32));
        }
        Some(bits) => {
            if (bits.len() as u64) * 8 < points as u64 {
                return Err(Skip::Other("a bitmap shorter than its grid".to_string()));
            }
            let mut values = values.into_iter();
            for k in 0..points {
                let set = bits.get(k / 8).is_some_and(|b| b & (0x80 >> (k % 8)) != 0);
                out.push(if set { values.next().map_or(f32::NAN, |v| v as f32) } else { f32::NAN });
            }
        }
    }
    Ok(out)
}

/// `n` bitmap bits set among the first `points`.
fn kept(bits: &[u8], points: usize) -> usize {
    (0..points).filter(|k| bits.get(k / 8).is_some_and(|b| b & (0x80 >> (k % 8)) != 0)).count()
}

/// The reference time, `d=YYYYMMDDHH` as wgrib's inventories write it (with the century).
fn date(year: i64, month: u32, day: u32, hour: u32) -> String {
    format!("d={year:04}{month:02}{day:02}{hour:02}")
}

/// A section of `b` at `at`: its bytes, by the 3-octet length GRIB1 sections start with.
fn section1(b: &[u8], at: usize) -> std::result::Result<&[u8], Skip> {
    let len = be(b, at, 3) as usize;
    if len < 3 {
        return Err(Skip::Other("a damaged section".to_string()));
    }
    b.get(at..at + len).ok_or_else(|| Skip::Other("a section past the end of its message".to_string()))
}

/// A GRIB1 message (WMO FM 92 GRIB edition 1): its one field.
fn grib1(b: &[u8], room: usize) -> std::result::Result<Field, Skip> {
    let pds = section1(b, 8)?;
    if pds.len() < 28 {
        return Err(Skip::Other("a damaged product definition section".to_string()));
    }
    let flags = pds[7];
    if flags & 0x80 == 0 {
        return Err(Skip::Other(format!("predefined grids (grid {})", pds[6])));
    }
    let d = signed(pds, 26, 2);
    let century = i64::from(pds[24]);
    let year = (century - 1) * 100 + i64::from(pds[12]);
    let name = format!(
        "{}:kpds5={}:kpds6={}:kpds7={}:TR={}:P1={}:P2={}:TimeU={}",
        date(year, u32::from(pds[13]), u32::from(pds[14]), u32::from(pds[15])),
        pds[8],
        pds[9],
        be(pds, 10, 2),
        pds[20],
        pds[18],
        pds[19],
        pds[17],
    );
    let gds_at = 8 + pds.len();
    let gds = section1(b, gds_at)?;
    let grid = grib1_grid(gds)?;
    let points = grid.ni.checked_mul(grid.nj).filter(|n| *n <= room).ok_or(Skip::TooLarge)?;
    let mut at = gds_at + gds.len();
    let bitmap = if flags & 0x40 != 0 {
        let bms = section1(b, at)?;
        if be(bms, 4, 2) != 0 {
            return Err(Skip::Other("predefined bitmaps".to_string()));
        }
        at += bms.len();
        Some(bms.get(6..).unwrap_or_default())
    } else {
        None
    };
    let bds = section1(b, at)?;
    let bds_flags = bds.get(3).copied().unwrap_or(0);
    if bds_flags & 0x80 != 0 {
        return Err(Skip::Other("spherical harmonics".to_string()));
    }
    if bds_flags & 0x40 != 0 {
        return Err(Skip::Other("GRIB1 second-order (complex) packing".to_string()));
    }
    let e = signed(bds, 4, 2);
    let r = ibm(be(bds, 6, 4));
    let bits = u32::from(bds.get(10).copied().unwrap_or(0));
    let count = bitmap.map_or(points, |m| kept(m, points));
    let data = bds.get(11..).unwrap_or_default();
    let (values, rounded) =
        if bits == 0 { constant(r / 10f64.powi(d.clamp(-1000, 1000) as i32), count)? } else { unpack(data, count, bits, r, e, d)? };
    Ok(Field { name, grid, values: spread(values, bitmap, points)?, rounded })
}

/// A GRIB1 grid description section: a regular latitude/longitude grid only.
fn grib1_grid(g: &[u8]) -> std::result::Result<Grid, Skip> {
    if g.len() < 28 {
        return Err(Skip::Other("a damaged grid description section".to_string()));
    }
    let kind = g[5];
    if kind != 0 {
        return Err(Skip::Other(format!("GRIB1 grids of data representation type {kind} (not latitude/longitude)")));
    }
    let (ni, nj) = (be(g, 6, 2) as usize, be(g, 8, 2) as usize);
    // A missing row or column length: a quasi-regular grid.
    if ni == 0xffff || nj == 0xffff || ni == 0 || nj == 0 {
        return Err(Skip::Other("quasi-regular grids".to_string()));
    }
    let deg = |o: usize| signed(g, o, 3) as f64 / 1000.0;
    let (lat1, lon1, lat2, lon2) = (deg(10), deg(13), deg(17), deg(20));
    let res = g[16];
    let (di, dj) = (be(g, 23, 2), be(g, 25, 2));
    let given = res & 0x80 != 0;
    let dx = if given && di != 0xffff { f64::from(di) / 1000.0 } else { span(lon1, lon2, true) / (ni.saturating_sub(1)).max(1) as f64 };
    let dy = if given && dj != 0xffff { f64::from(dj) / 1000.0 } else { span(lat1, lat2, false) / (nj.saturating_sub(1)).max(1) as f64 };
    let earth =
        Some(if res & 0x40 != 0 { "the IAU 1965 spheroid (GRIB1's oblate earth)" } else { "a sphere of radius 6,367,470 m (GRIB1's)" });
    checked(Grid { ni, nj, lat1, lon1, lat2, lon2, dx, dy, scan: g[27], earth })
}

/// The distance between two longitudes (eastward, wrapping) or latitudes.
fn span(a: f64, b: f64, lon: bool) -> f64 {
    if lon {
        let d = (b - a).rem_euclid(360.0);
        if d == 0.0 { 360.0 } else { d }
    } else {
        (b - a).abs()
    }
}

/// A grid whose increments and corners are usable, or why it isn't.
fn checked(g: Grid) -> std::result::Result<Grid, Skip> {
    let ok = g.dx.is_finite()
        && g.dy.is_finite()
        && g.dx > 0.0
        && g.dy > 0.0
        && g.lat1.abs() <= 90.0
        && g.lat2.abs() <= 90.0
        && g.ni as f64 * g.dx <= 360.0 + g.dx
        && g.nj as f64 * g.dy <= 180.0 + g.dy;
    let north = g.north();
    let south = north - (g.nj.saturating_sub(1)) as f64 * g.dy;
    if ok && north <= 90.0 + 1e-6 && south >= -90.0 - 1e-6 {
        Ok(g)
    } else {
        Err(Skip::Other("a latitude/longitude grid that isn't on the globe".to_string()))
    }
}

/// A GRIB2 section at `at`: its number and bytes.
fn section2(b: &[u8], at: usize) -> Option<(u8, &[u8])> {
    let len = be(b, at, 4) as usize;
    let s = b.get(at..at.checked_add(len)?)?;
    (len >= 5).then(|| (s[4], s))
}

/// A GRIB2 message (WMO FM 92 GRIB edition 2): each of its fields.
fn grib2(b: &[u8], room: usize) -> Vec<std::result::Result<Field, Skip>> {
    let mut out = Vec::new();
    let discipline = b.get(6).copied().unwrap_or(255);
    let mut at = 16usize;
    let mut reference = String::from("d=?");
    let mut grid: Option<std::result::Result<Grid, String>> = None;
    let mut product = String::new();
    let mut packing: Option<std::result::Result<(usize, f64, i64, i64, u32), String>> = None;
    let mut bitmap: Option<std::result::Result<Option<&[u8]>, String>> = None;
    let mut previous_bitmap: Option<&[u8]> = None;
    let mut held = 0usize;
    while b.get(at..at + 4).is_some_and(|m| m != b"7777") {
        let Some((number, s)) = section2(b, at) else {
            out.push(Err(Skip::Other("a damaged GRIB2 section".to_string())));
            break;
        };
        at += s.len();
        match number {
            1 => {
                reference = date(
                    i64::from(be(s, 12, 2)),
                    u32::from(s.get(14).copied().unwrap_or(0)),
                    u32::from(s.get(15).copied().unwrap_or(0)),
                    u32::from(s.get(16).copied().unwrap_or(0)),
                );
            }
            3 => grid = Some(grib2_grid(s)),
            4 => product = grib2_product(s),
            5 => packing = Some(grib2_packing(s)),
            6 => {
                bitmap = Some(match s.get(5).copied().unwrap_or(255) {
                    255 => Ok(None),
                    0 => {
                        let bits = s.get(6..).unwrap_or_default();
                        previous_bitmap = Some(bits);
                        Ok(Some(bits))
                    }
                    254 => previous_bitmap.map(Some).ok_or_else(|| "a bitmap that refers to none before it".to_string()),
                    _ => Err("predefined bitmaps".to_string()),
                });
            }
            7 => {
                let field = (|| {
                    let grid = grid.clone().ok_or_else(|| Skip::Other("a field without a grid".to_string()))?.map_err(Skip::Other)?;
                    let (count, r, e, d, bits) =
                        packing.clone().ok_or_else(|| Skip::Other("a field without its packing".to_string()))?.map_err(Skip::Other)?;
                    let mask = bitmap.clone().unwrap_or(Ok(None)).map_err(Skip::Other)?;
                    let points = grid.ni.checked_mul(grid.nj).filter(|n| held.saturating_add(*n) <= room).ok_or(Skip::TooLarge)?;
                    if mask.is_some_and(|m| kept(m, points) != count) || (mask.is_none() && count != points) {
                        return Err(Skip::Other("a field whose values don't fill its grid".to_string()));
                    }
                    let (values, rounded) = if bits == 0 {
                        constant(r / 10f64.powi(d.clamp(-1000, 1000) as i32), count)?
                    } else {
                        unpack(s.get(5..).unwrap_or_default(), count, bits, r, e, d)?
                    };
                    held += points;
                    Ok(Field {
                        name: format!("{reference}:discipline={discipline}:{product}"),
                        grid,
                        values: spread(values, mask, points)?,
                        rounded,
                    })
                })();
                out.push(field);
            }
            _ => {}
        }
    }
    if out.is_empty() {
        out.push(Err(Skip::Other("a GRIB2 message without a data section".to_string())));
    }
    out
}

/// A GRIB2 grid definition section: template 3.0 (regular latitude/longitude) only.
fn grib2_grid(s: &[u8]) -> std::result::Result<Grid, String> {
    if s.get(5).copied().unwrap_or(1) != 0 {
        return Err("predefined grids".to_string());
    }
    if s.get(10).copied().unwrap_or(0) != 0 {
        return Err("quasi-regular grids".to_string());
    }
    let template = be(s, 12, 2);
    if template != 0 {
        return Err(format!("GRIB2 grid template 3.{template} (not regular latitude/longitude)"));
    }
    if s.len() < 72 {
        return Err("a damaged grid definition section".to_string());
    }
    let earth = match s[14] {
        // GRS 80, WGS 84: near enough.
        4 | 5 => None,
        0 => Some("a sphere of radius 6,367,470 m"),
        6 => Some("a sphere of radius 6,371,229 m"),
        8 => Some("a sphere of radius 6,371,200 m"),
        1 => Some("a sphere of a radius the file gives"),
        2 => Some("the IAU 1965 spheroid"),
        _ => Some("a spheroid the file gives"),
    };
    let (ni, nj) = (be(s, 30, 4) as usize, be(s, 34, 4) as usize);
    if ni == 0 || nj == 0 || ni == u32::MAX as usize || nj == u32::MAX as usize {
        return Err("quasi-regular grids".to_string());
    }
    let (basic, sub) = (be(s, 38, 4), be(s, 42, 4));
    let unit = if basic == 0 || basic == u32::MAX {
        1e-6
    } else if sub == 0 || sub == u32::MAX {
        return Err("a grid with no angle subdivisions".to_string());
    } else {
        f64::from(basic) / f64::from(sub)
    };
    let deg = |o: usize| signed(s, o, 4) as f64 * unit;
    let (lat1, lon1, lat2, lon2) = (deg(46), deg(50), deg(55), deg(59));
    let res = s[54];
    let (di, dj) = (be(s, 63, 4), be(s, 67, 4));
    let dx = if res & 0x20 != 0 && di != u32::MAX {
        f64::from(di) * unit
    } else {
        span(lon1, lon2, true) / (ni.saturating_sub(1)).max(1) as f64
    };
    let dy = if res & 0x10 != 0 && dj != u32::MAX {
        f64::from(dj) * unit
    } else {
        span(lat1, lat2, false) / (nj.saturating_sub(1)).max(1) as f64
    };
    checked(Grid { ni, nj, lat1, lon1, lat2, lon2, dx, dy, scan: s[71], earth }).map_err(|e| match e {
        Skip::Other(m) => m,
        Skip::TooLarge => "a grid too large".to_string(),
    })
}

/// A GRIB2 product definition section, as part of a band name: the parameter's category and
/// number (its discipline is section 0's), the template, and for templates 4.0 to 4.15 the first
/// fixed surface and the forecast time.
fn grib2_product(s: &[u8]) -> String {
    let template = be(s, 7, 2);
    let (category, number) = (s.get(9).copied().unwrap_or(255), s.get(10).copied().unwrap_or(255));
    let mut name = format!("category={category}:number={number}:template=4.{template}");
    // Templates 4.0 to 4.15 share these octets; the surface's value is scaled by 10^-(its scale
    // factor).
    if template <= 15 && s.len() >= 28 {
        let value = signed(s, 24, 4) as f64 / 10f64.powi(signed(s, 23, 1) as i32);
        name.push_str(&format!(":surface={}:value={}:TimeU={}:forecast={}", s[22], round6(value), s[17], be(s, 18, 4)));
    }
    // An ensemble's member (templates 4.1 and 4.11): its type and perturbation number.
    if matches!(template, 1 | 11) && s.len() >= 37 {
        name.push_str(&format!(":ensemble={}:member={}", s[34], s[35]));
    }
    name
}

/// A GRIB2 data representation section: template 5.0 (simple packing) only, as the number of
/// values, R, E, D and the bits a value.
fn grib2_packing(s: &[u8]) -> std::result::Result<(usize, f64, i64, i64, u32), String> {
    let template = be(s, 9, 2);
    if template != 0 {
        let kind = match template {
            2 | 3 => "complex packing",
            4 => "IEEE floating point",
            40 => "JPEG 2000",
            41 => "PNG",
            42 => "CCSDS",
            _ => "a packing",
        };
        return Err(format!("GRIB2 {kind} (template 5.{template})"));
    }
    if s.len() < 21 {
        return Err("a damaged data representation section".to_string());
    }
    let r = f64::from(f32::from_bits(be(s, 11, 4)));
    Ok((be(s, 5, 4) as usize, r, signed(s, 15, 2), signed(s, 17, 2), u32::from(s[19])))
}

#[cfg(test)]
#[path = "grib_tests.rs"]
mod tests;
