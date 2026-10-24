//! Data cubes (opengeos/RustGIS#579): rasters on one grid at a series of dates, whatever they come
//! from, and the trend of each cell over them.
//!
//! A [`TimeCube`] gives its dates and the slice at each one. The sources:
//! - a Zarr variable with a time dimension (#554), read a slice at a time;
//! - a stack of raster layers, each a date (from the `dates` given, or found in the layer's name
//!   or file);
//! - a raster layer of several bands, each a date (from `dates`, or the bands' names).
//!
//! `cube.trend` reads the slices into one block, cell by date, and works out each cell's trend
//! with [`rustgis_analysis::trend`]: Sen's slope and the modified Mann–Kendall test.
//! `cube.reduce` folds the slices into running totals as they are read: a statistic over time,
//! for the whole period or a year (or a month of the year) at a time.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rustgis_analysis::trend::{self, Test};
use rustgis_data::raster::{self, Raster};
use serde_json::{Value, json};

use super::ps;
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![C::new("cube.trend", "Trend Analysis", "Analyze › Image Analysis", trend_cmd).params(
        r#"{"layers"?: [raster layers, a date each] | "layer"?: a Zarr layer (or "path" | "url", "variable", "index", "crs" as for zarr.reduce) or a raster of several bands, a date each (default: the active layer) | "collection": a STAC collection (desktop; read in place over an area, in pieces when large; with url, asset, bbox, maxCloud, cellSize and resampling below, and "start", "end"), "url"?: the collection's STAC API (as for stac.addCube), "asset"?: the collection's band, such as NDVI, "bbox"?: [w, s, e, n] | "view" (the collection's area), "maxCloud"?: a collection's cloud cover cap, "cellSize"?: a collection's cell size in the files' units (the overview level nearest it is read), "resampling"?: "nearest" (default: that overview's pixels as stored) | "average" (each cell the mean of the full images' pixels in it, which reads them), "band"?: 1-based (a stack's band), "dates"?: [dates] (a stack's or the bands' dates, in their order; default: read from the layers' names or files, or the bands' names), "start"?, "end"?: dates (a whole number from 1000 to 9999 is a year; an end given as a year or a month takes all of it), "months"?: [1–12] (one season across years), "test"?: "hamedRao" (default) | "yueWang" | "seasonal" | "original", "seasons"?: parts of a year for the seasonal test (default 12), "alpha"?: 0.05 (above 0, below 0.5), "minValid"?: dates a cell needs (default: half of them, at least 4; the seasonal test also needs 4 in seasons of two or more), "per"?: "year" (default) | "decade", "name"?: text, "ran"?: (internal: a trend the app has worked out)} → four raster layers: Sen's slope, the test's Z and p-value, and the slope where p < alpha"#,
    ),
    C::new("cube.reduce", "Summarise a Data Cube over Time", "Analyze › Image Analysis", reduce_cmd).params(
        r#"{"layers"?: [raster layers, a date each] | "layer"?: a Zarr layer or a raster of several bands, a date each (default: the active layer) | "path"?: a Zarr store's folder | "url"?: a Zarr store's address, or a STAC collection's API (as for stac.addCube) | "collection": a STAC collection (desktop; read in place over an area, in pieces when large; with url, asset, bbox, maxCloud, cellSize and resampling below, and "start", "end"), "variable"?, "index"?, "crs"?: a Zarr variable's (as for zarr.reduce), "asset"?: the collection's band, such as NDVI, "bbox"?: [w, s, e, n] | "view" (the collection's area), "maxCloud"?: a collection's cloud cover cap, "cellSize"?: a collection's cell size in the files' units (the overview level nearest it is read), "resampling"?: "nearest" (default) | "average" (as for cube.trend), "band"?: 1-based (a stack's band), "dates"?: [dates] (a stack's or the bands' dates, in their order; default: read from the layers' names or files, or the bands' names), "stat": "mean"|"min"|"max"|"sum"|"std" (the population standard deviation, n)|"count"|"countAbove", "above"?: number (countAbove's threshold), "by"?: "all" (default: one layer) | "year" (a layer for each calendar year) | "month" (a layer for each month of the year, across years), "start"?, "end"?: dates (as for cube.trend), "months"?: [1–12] (a season across years), "minValid"?: 0–1 (a cell with a smaller share of its group's dates is left empty; default 0), "name"?: text} → raster layers of the statistic, one for each group"#,
    )]
}

/// The most cell values a cube is read into at once (cells × dates): 64 million, 256 MB, on the
/// desktop; 16 million, 64 MB, in the browser's smaller memory. A run also takes, on each of its
/// threads (up to 16 on the desktop, one in the browser), a cell's Sen slopes at a time (dates² ⁄ 2:
/// at most 16 MB at [`MAX_DATES`], so up to 256 MB more at the peak), freed cell by cell; and its
/// four outputs, four values a cell, made once the block is dropped.
pub const MAX_VALUES: u64 = if cfg!(target_arch = "wasm32") { 16_000_000 } else { 64_000_000 };
/// The most pairs of dates a trend compares in all (cells × dates² ⁄ 2), which bounds the time
/// Sen's slope takes: far fewer in the browser, which works them out on one thread. At the cap a
/// run can take minutes on the desktop, which the app runs on threads with progress and Cancel;
/// `cube.trend` called directly (a script, the assistant, an MCP client) takes
/// [`MAX_WAITED_PAIRS`], but where only a shell waits on it (`rustgis-cli run`:
/// [`Session::long_runs`](crate::Session::long_runs)) it takes this.
pub const MAX_PAIRS: u64 = if cfg!(target_arch = "wasm32") { 50_000_000 } else { 20_000_000_000 };
/// The most pairs a trend worked out where it's called compares (`cube.trend` without `ran`: a
/// script, the assistant, an MCP client), which waits for it with no Cancel: a tenth of the app's
/// on the desktop. The browser works every trend out where it's called, within its own lower cap.
pub const MAX_WAITED_PAIRS: u64 = if cfg!(target_arch = "wasm32") { MAX_PAIRS } else { MAX_PAIRS / 10 };
/// The most dates a cube is read at.
pub const MAX_DATES: usize = 2_000;

/// Rasters on one grid at a series of dates.
pub trait TimeCube: Send + Sync {
    /// The dates, in milliseconds since 1970, in the cube's order.
    fn dates(&self) -> &[f64];
    /// The slice at date `i`: a raster and the band holding the values, or `None` for a date
    /// with nothing written (a Zarr step never written), which has no value for any cell.
    fn slice(&self, i: usize) -> Result<Option<(Arc<Raster>, usize)>, String>;
    /// What the cube holds, for the outputs' names.
    fn label(&self) -> String;
    /// The steps taken when no period is asked for (a following Zarr layer's), as a range.
    fn period(&self) -> (usize, usize) {
        (0, self.dates().len())
    }
    /// The pieces it is read in, each a band of rows of the whole grid from the top: one for a cube
    /// read whole. A cube too large for one block of [`MAX_VALUES`] (a STAC collection over a
    /// state) is worked out piece by piece, each within it.
    fn pieces(&self) -> usize {
        1
    }
    /// Date `i`'s slice, piece `piece` of it (the whole slice for a cube read whole).
    fn slice_of(&self, i: usize, piece: usize) -> Result<Option<(Arc<Raster>, usize)>, String> {
        let _ = piece;
        self.slice(i)
    }
    /// The whole grid's columns and rows, when known before any slice is read (a STAC cube's area),
    /// so a run too long is refused before it reads.
    fn size(&self) -> Option<(usize, usize)> {
        None
    }
    /// How many dates are read at once: files read over the network wait mostly on it. Up to
    /// about twice as many slices are held at a time (one in each reader's hands, as many waiting
    /// to be placed), so a cube with more than one reader keeps its slices small: a STAC cube's are a
    /// million pixels at most, about 4 MB each.
    fn readers(&self) -> usize {
        1
    }
    /// Why the run read nothing, asked once every slice is read: a cube none of whose files met
    /// the area (a STAC cube's), so its outputs would be empty. `None` when something was read.
    fn nothing_read(&self) -> Option<String> {
        None
    }
}

/// A stack of rasters (or a raster's bands), a date each.
pub struct Stack {
    /// Each date's raster and band.
    items: Vec<(f64, Arc<Raster>, usize)>,
    dates: Vec<f64>,
    label: String,
}

impl Stack {
    /// Sorted by date.
    fn new(mut items: Vec<(f64, Arc<Raster>, usize)>, label: String) -> Stack {
        items.sort_by(|a, b| a.0.total_cmp(&b.0));
        let dates = items.iter().map(|i| i.0).collect();
        Stack { items, dates, label }
    }
}

impl TimeCube for Stack {
    fn dates(&self) -> &[f64] {
        &self.dates
    }
    fn slice(&self, i: usize) -> Result<Option<(Arc<Raster>, usize)>, String> {
        self.items.get(i).map(|(_, r, b)| Some((r.clone(), *b))).ok_or_else(|| format!("no date {i}"))
    }
    fn label(&self) -> String {
        self.label.clone()
    }
}

/// A Zarr variable along its time dimension.
#[cfg(not(target_arch = "wasm32"))]
struct ZarrCube(super::zarr::ZarrCube);

#[cfg(not(target_arch = "wasm32"))]
impl TimeCube for ZarrCube {
    fn dates(&self) -> &[f64] {
        &self.0.zt.dates
    }
    fn slice(&self, i: usize) -> Result<Option<(Arc<Raster>, usize)>, String> {
        // A slice already kept is used; the others are read without being kept, so a trend over
        // many dates doesn't push out the slices a layer following the slider holds. A step
        // never written is `None`: no value that date, as the Zarr summary takes it.
        let zt = &self.0.zt;
        let r = match super::zarr::kept_slice(&super::zarr::slice_key(zt, i)) {
            Some(r) => r,
            None => match super::zarr::read_slice(zt, i)? {
                Some(r) => Arc::new(r),
                None => return Ok(None),
            },
        };
        Ok(Some((r, 0)))
    }
    fn label(&self) -> String {
        self.0.zt.variable.clone()
    }
    fn period(&self) -> (usize, usize) {
        self.0.period
    }
}

/// A date written in a name: `2024-06-01` (or with `_`, `.` or nothing between), MODIS's
/// `A2024153` (year and day of the year), `2024-06`, or a year alone. Milliseconds since 1970.
pub fn date_in_text(text: &str) -> Option<f64> {
    let b = text.as_bytes();
    let digits = |from: usize, n: usize| -> Option<u32> {
        let s = text.get(from..from + n)?;
        s.bytes().all(|c| c.is_ascii_digit()).then(|| s.parse().ok()).flatten()
    };
    // Digits at `i` not run on from a digit before.
    let before = |i: usize| i.checked_sub(1).and_then(|j| b.get(j)).copied();
    let starts = |i: usize| !before(i).is_some_and(|c| c.is_ascii_digit());
    let ends = |i: usize| !b.get(i).is_some_and(u8::is_ascii_digit);
    let day = |y: u32, m: u32, d: u32| {
        ((1800..=2200).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d))
            .then(|| rustgis_data::time::parse_text(&format!("{y:04}-{m:02}-{d:02}")))
            .flatten()
    };
    // MODIS: A + year + day of the year.
    for (i, c) in b.iter().enumerate() {
        if *c == b'A'
            && ends(i + 8)
            && let (Some(y), Some(doy)) = (digits(i + 1, 4), digits(i + 5, 3))
            && (1..=366).contains(&doy)
            && let Some(jan1) = day(y, 1, 1)
        {
            return Some(jan1 + f64::from(doy - 1) * 86_400_000.0);
        }
    }
    // A year alone, only once no fuller date is found anywhere in the text, and only standing
    // apart (not `res2000` or `v2015`, which name something else).
    let mut bare = None;
    let apart = |i: usize| !before(i).is_some_and(|c| c.is_ascii_alphanumeric()) && !b.get(i + 4).is_some_and(u8::is_ascii_alphanumeric);
    for i in 0..b.len() {
        if !starts(i) {
            continue;
        }
        let Some(y) = digits(i, 4) else { continue };
        // Year, month and day, with one separator or none.
        let sep = b.get(i + 4).copied().filter(|c| matches!(c, b'-' | b'_' | b'.'));
        let at = i + 4 + usize::from(sep.is_some());
        if let Some(m) = digits(at, 2) {
            let next = at + 2;
            let dsep = if sep.is_some() { b.get(next).copied().filter(|c| Some(*c) == sep) } else { None };
            let dat = next + usize::from(dsep.is_some());
            if (sep.is_none() || dsep.is_some())
                && let Some(d) = digits(dat, 2)
                && ends(dat + 2)
                && let Some(ms) = day(y, m, d)
            {
                return Some(ms);
            }
            if sep.is_some()
                && ends(next)
                && let Some(ms) = day(y, m, 1)
            {
                return Some(ms);
            }
        }
        if bare.is_none()
            && ends(i + 4)
            && apart(i)
            && let Some(ms) = day(y, 1, 1)
        {
            bare = Some(ms);
        }
    }
    bare
}

fn date_list(p: &Value) -> Result<Option<Vec<f64>>, String> {
    match p.get("dates").filter(|v| !v.is_null()) {
        None => Ok(None),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| rustgis_data::time::parse_input(v).ok_or_else(|| format!("{v} isn't a date")))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err("dates is a list of dates".into()),
    }
}

/// What a run asks of its cube, and its name in what it says: a trend needs a few dates and a
/// Zarr axis in time order (the test's sign and count follow it); a summary takes two dates or more
/// in any order (a raster of one band is no cube, and one date has nothing to summarise).
#[derive(Clone, Copy, Debug)]
pub struct Needs {
    pub what: &'static str,
    pub min_dates: usize,
    pub ordered: bool,
}

/// What `cube.trend` asks of its cube.
pub const TREND: Needs = Needs { what: "trend", min_dates: trend::MIN_VALUES, ordered: true };
/// What `cube.reduce` asks of its cube.
pub const SUMMARY: Needs = Needs { what: "summary", min_dates: 2, ordered: false };

/// The cube a command's parameters name: a stack of layers, a Zarr variable, or a raster's bands.
pub fn cube_from(s: &Session, p: &Value, needs: Needs) -> Result<Box<dyn TimeCube>, String> {
    let dates = date_list(p)?;
    let what = needs.what;
    if let Some(list) = p.get("layers").filter(|v| !v.is_null()) {
        let list = list.as_array().ok_or("layers is a list of raster layers")?;
        if list.len() < needs.min_dates {
            return Err(format!("a {what}'s stack needs {} layers or more, a date each", needs.min_dates));
        }
        // At most a run's dates, refused before each is looked up and checked against the others.
        if list.len() > MAX_DATES {
            return Err(format!("{} layers; a {what} takes {MAX_DATES} dates at most", list.len()));
        }
        if dates.as_ref().is_some_and(|d| d.len() != list.len()) {
            return Err("dates has a date for each layer, in their order".into());
        }
        let band = band_index(p)?;
        let mut items = Vec::with_capacity(list.len());
        let mut label = String::new();
        for (k, v) in list.iter().enumerate() {
            let id = s.layer_id(&json!({ "layer": v }))?;
            let l = s.map().layer(id).ok_or("no such layer")?;
            let r = l.raster.as_ref().and_then(|r| r.data.clone()).ok_or_else(|| format!("{} isn't a raster read into the map", l.name))?;
            if band >= r.bands.len() {
                return Err(format!("{} has {} bands", l.name, r.bands.len()));
            }
            // The file's own name, not its folders' (`/data/2020/ndvi_a.tif` would date them all).
            let file = l
                .raster
                .as_ref()
                .and_then(|r| r.path.as_deref())
                .and_then(|f| std::path::Path::new(f).file_name())
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            let date = match &dates {
                Some(d) => d.get(k).copied().ok_or("dates has a date for each layer, in their order")?,
                None => date_in_text(&l.name)
                    .or_else(|| date_in_text(&file))
                    .ok_or_else(|| format!("no date in the name of {}: give \"dates\"", l.name))?,
            };
            if label.is_empty() {
                label = l.name.clone();
            }
            // One layer a date: two on a day would count as two ordered values in the test.
            if items.iter().any(|(d, ..): &(f64, _, usize)| *d == date) {
                return Err(format!(
                    "{} and another layer have the same date ({}): one layer a date",
                    l.name,
                    rustgis_data::time::format(date, "YYYY-MM-DD")
                ));
            }
            items.push((date, r, band));
        }
        return Ok(Box::new(Stack::new(items, common_label(&label))));
    }
    let given = |k: &str| p.get(k).is_some_and(|v| !v.is_null());
    // A STAC collection's band, read from its files over an area (#579): named by its collection.
    if given("collection") {
        #[cfg(target_arch = "wasm32")]
        return Err(format!("in the web app, a STAC collection's {what} can't be worked out yet (the desktop app can)"));
        #[cfg(not(target_arch = "wasm32"))]
        return Ok(Box::new(super::stac::stac_cube(s, p)?));
    }
    let zarr_layer = !given("path") && !given("url") && {
        let id = s.layer_id(p)?;
        // A Zarr layer: one following the slider, or a slice, named by its steps (`t2m[time=3]`);
        // another file's subdataset (GDAL's) goes the bands' way below.
        s.map()
            .layer(id)
            .and_then(|l| l.raster.as_ref())
            .is_some_and(|r| r.time_zarr.is_some() || r.subdataset.as_deref().is_some_and(|d| d.contains('[')))
    };
    if given("path") || given("url") || zarr_layer {
        // The web app reads no Zarr store along its time axis yet: said, not taken for bands.
        #[cfg(target_arch = "wasm32")]
        return Err(format!("in the web app, a Zarr variable's {what} can't be worked out yet (the desktop app can)"));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let cube = super::zarr::zarr_cube(s, p)?;
            // A trend takes the steps in time order: an axis out of order (descending) or with a
            // date twice would turn the test's sign or count a date twice. A summary doesn't mind.
            if needs.ordered && !cube.zt.dates.windows(2).all(|w| w[0] < w[1]) {
                return Err(format!(
                    "{}'s time axis isn't in increasing order without repeats, so a trend can't be read from it",
                    cube.zt.variable
                ));
            }
            return Ok(Box::new(ZarrCube(cube)));
        }
    }
    let id = s.layer_id(p)?;
    let l = s.map().layer(id).ok_or("no such layer")?;
    let r = l.raster.as_ref().and_then(|r| r.data.clone()).ok_or_else(|| format!("{} isn't a raster", l.name))?;
    if r.bands.len() < needs.min_dates {
        return Err(format!(
            "{} has {} band(s): a {what}'s cube is a raster of {} bands or more (a date each), a Zarr variable with a time dimension, or \"layers\"",
            l.name,
            r.bands.len(),
            needs.min_dates
        ));
    }
    if dates.as_ref().is_some_and(|d| d.len() != r.bands.len()) {
        return Err(format!("dates has a date for each of the {} bands", r.bands.len()));
    }
    let mut items = Vec::with_capacity(r.bands.len());
    for (b, band) in r.bands.iter().enumerate() {
        let date = match &dates {
            Some(d) => d.get(b).copied().ok_or("dates has a date for each band, in their order")?,
            None => {
                date_in_text(&band.name).ok_or_else(|| format!("band {} ({}) has no date in its name: give \"dates\"", b + 1, band.name))?
            }
        };
        // One band a date, as one layer a date in a stack.
        if items.iter().any(|(d, ..): &(f64, _, usize)| *d == date) {
            return Err(format!(
                "band {} and another band have the same date ({}): one band a date",
                b + 1,
                rustgis_data::time::format(date, "YYYY-MM-DD")
            ));
        }
        items.push((date, r.clone(), b));
    }
    Ok(Box::new(Stack::new(items, l.name.clone())))
}

fn band_index(p: &Value) -> Result<usize, String> {
    match p.get("band").filter(|v| !v.is_null()) {
        None => Ok(0),
        Some(v) => v.as_u64().filter(|b| *b >= 1).map(|b| b as usize - 1).ok_or_else(|| "band is a number from 1".into()),
    }
}

/// A name with its date taken out (`NDVI_2024-06-01` → `NDVI`, `MOD13Q1.A2024153` → `MOD13Q1`),
/// for the outputs' names: cut where the date [`date_in_text`] finds begins, so a product name with
/// digits of its own keeps them.
fn common_label(name: &str) -> String {
    // The date's start: the last place from which the same date is still found (past it, the date
    // is gone or another). Names are short; a long one takes the simpler cut below.
    if let Some(date) = date_in_text(name).filter(|_| name.len() <= 512) {
        let start = name.char_indices().map(|(i, _)| i).take_while(|i| date_in_text(&name[*i..]) == Some(date)).last().unwrap_or(0);
        let cut = name[..start].trim_end_matches(['_', '-', ' ', '.']);
        if !cut.is_empty() {
            return cut.to_string();
        }
    }
    let lead: String = name.chars().take_while(|c| !c.is_ascii_digit()).collect();
    // MODIS dates its files `A2024153` (year and day of the year): that A goes with the date; any
    // other final A is the name's own (`NDVI_BETA2020`).
    let modis = lead.ends_with('A') && name[lead.len()..].chars().take_while(char::is_ascii_digit).count() == 7;
    let cut = if modis { &lead[..lead.len() - 1] } else { lead.as_str() };
    let cut = cut.trim_end_matches(['_', '-', ' ', '.']);
    if cut.is_empty() { name.to_string() } else { cut.to_string() }
}

/// A date's season among `m` in its calendar year (UTC): its month for 12, otherwise its part of
/// the year by day, so a composite on the same day each year (16-day ones, 23 a year) keeps its
/// season from year to year (a day near a season's edge can move by one in a leap year).
pub fn season_of(ms: f64, m: u32) -> usize {
    let m = m.max(1) as usize;
    let month = rustgis_data::time::format(ms, "MM").parse::<usize>().unwrap_or(1);
    if m == 12 {
        return month.clamp(1, 12) - 1;
    }
    let year = rustgis_data::time::format(ms, "YYYY");
    let start = rustgis_data::time::parse_text(&format!("{year}-01-01")).unwrap_or(ms);
    let next = year
        .parse::<i64>()
        .ok()
        .and_then(|y| rustgis_data::time::parse_text(&format!("{:04}-01-01", y + 1)))
        .unwrap_or(start + 365.0 * 86_400_000.0);
    let fraction = ((ms - start) / (next - start).max(1.0)).clamp(0.0, 1.0);
    ((fraction * m as f64).floor() as usize).min(m - 1)
}

/// The dates of a cube a run takes: those between `start` and `end` and in `months`, or, with
/// no period given, the cube's own `period` of steps (a following Zarr layer's).
pub fn steps(dates: &[f64], period: (usize, usize), p: &Value) -> Result<Vec<usize>, String> {
    let date = |k: &str| -> Result<Option<f64>, String> {
        match p.get(k).filter(|v| !v.is_null()) {
            None => Ok(None),
            Some(v) => rustgis_data::time::parse_input(v)
                .map(Some)
                .ok_or_else(|| format!("{k} is a date (2015-06-01, a year, or milliseconds since 1970)")),
        }
    };
    let months = months(p)?;
    // An end given as a year or a month takes all of it, as a STAC search's does: "end": "2024"
    // keeps 2024's last dates.
    let end = match p.get("end").filter(|v| !v.is_null()).and_then(super::stac::period_after) {
        Some(after) => Some(after - 1.0),
        None => date("end")?,
    };
    let start = date("start")?;
    let (lo, hi) = match (start, end) {
        (None, None) => (period.0.min(dates.len()), period.1.min(dates.len())),
        _ => (0, dates.len()),
    };
    let month = |d: f64| rustgis_data::time::format(d, "MM").parse::<u32>().unwrap_or(0);
    let out: Vec<usize> = (lo..hi)
        .filter(|i| {
            let d = dates.get(*i).copied().unwrap_or(f64::NAN);
            start.is_none_or(|a| d >= a) && end.is_none_or(|z| d <= z) && (months.is_empty() || months.contains(&month(d)))
        })
        .collect();
    if out.is_empty() {
        return Err("no date of the cube falls in the period (and months) asked for".into());
    }
    Ok(out)
}

/// The `months` a run keeps (1 to 12; none for every month): the one reading of the parameter,
/// for the steps and for the outputs' names.
pub(crate) fn months(p: &Value) -> Result<Vec<u32>, String> {
    match p.get("months").filter(|v| !v.is_null()) {
        None => Ok(Vec::new()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|m| {
                m.as_u64().filter(|m| (1..=12).contains(m)).map(|m| m as u32).ok_or_else(|| "months are numbers from 1 to 12".to_string())
            })
            .collect(),
        Some(_) => Err("months is a list of months (1–12)".into()),
    }
}

/// What a trend writes for each cell.
const OUTPUTS: usize = 4;

/// The parts a run's cells count for on a progress bar.
const CELL_PARTS: usize = 100;

/// A trend over a cube, prepared on the session and computed anywhere with [`TrendRun::run`]:
/// the desktop app runs it on a thread of its own, with progress and Cancel.
pub struct TrendRun {
    cube: Box<dyn TimeCube>,
    steps: Vec<usize>,
    test: Test,
    alpha: f64,
    min_valid: Option<usize>,
    /// Years in the slope's unit (1, or 10 for a decade).
    per: f64,
    /// The outputs' names start with this.
    pub name: String,
    /// The most pairs it compares: [`MAX_PAIRS`] on the app's threads, with Cancel; fewer for a
    /// call that waits for it ([`MAX_WAITED_PAIRS`]).
    max_pairs: u64,
}

impl TrendRun {
    /// The work, for a progress bar: the slices read (every piece's), then the cells worked out,
    /// in [`CELL_PARTS`] parts.
    pub fn total(&self) -> usize {
        self.steps.len() * self.cube.pieces().max(1) + CELL_PARTS
    }

    /// The pairs of dates one cell compares: all of them, or for the seasonal test only those of
    /// a season (`seasons`, a step's season each), which is what bounds its time.
    fn pairs_per_cell(&self, seasons: &[usize]) -> u64 {
        let n = self.steps.len() as u64;
        match self.test {
            Test::Seasonal(_) => {
                let mut by: std::collections::HashMap<usize, u64> = std::collections::HashMap::new();
                for s in seasons {
                    *by.entry(*s).or_default() += 1;
                }
                by.values().map(|k| k * k.saturating_sub(1) / 2).sum()
            }
            _ => n * n.saturating_sub(1) / 2,
        }
    }

    /// How many dates it reads.
    pub fn dates(&self) -> usize {
        self.steps.len()
    }

    /// Reads the slices into one block, cell by date, then works each cell's trend out on as
    /// many threads as there are cores; a cube read in pieces ([`TimeCube::pieces`]) does so piece
    /// by piece, the outputs put together row by row. `done` counts the slices read, and the last
    /// unit once the cells are done; `cancel` stops it between slices or rows.
    pub fn run(&self, cancel: &AtomicBool, done: &AtomicUsize) -> Result<Vec<Raster>, String> {
        let n = self.steps.len();
        // Times in the slope's unit.
        let year = 365.2425 * 86_400_000.0 * self.per;
        let date = |i: usize| self.cube.dates().get(i).copied().unwrap_or(f64::NAN);
        let times: Vec<f64> = self.steps.iter().map(|i| date(*i) / year).collect();
        // The seasonal test's seasons, from each date's place in its calendar year.
        let seasons: Vec<usize> = match self.test {
            Test::Seasonal(m) => self.steps.iter().map(|i| season_of(date(*i), m)).collect(),
            _ => Vec::new(),
        };
        let per_cell = self.pairs_per_cell(&seasons);
        let too_long = |cells: usize| {
            let pairs = (cells as u64).saturating_mul(per_cell);
            (pairs > self.max_pairs).then(|| {
                let how = if self.max_pairs < MAX_PAIRS { " (or run it from the Workflows pane, which can take more)" } else { "" };
                format!("{cells} cells × {n} dates is too long a run: take a smaller area, a coarser cell size or fewer dates{how}")
            })
        };
        // A cube that knows its size is refused before it reads anything.
        let whole = self.cube.size().map(|(w, h)| w.saturating_mul(h));
        if let Some(e) = whole.and_then(too_long) {
            return Err(e);
        }
        let need = self.min_valid.unwrap_or((n / 2).max(trend::MIN_VALUES)).max(trend::MIN_VALUES);
        let mut stitch = Stitch::default();
        // The four outputs, a band each, filled piece by piece (never held interleaved whole).
        let mut bands: [Vec<f32>; OUTPUTS] = Default::default();
        let mut cells_done = 0usize;
        for piece in 0..self.cube.pieces().max(1) {
            let (grid, block, cells) = self.read_piece(piece, cancel, done, whole.is_none().then_some(&too_long))?;
            stitch.check(&grid)?;
            // Progress through all the cells: a part of the bar every `tick` cells.
            let tick = whole.unwrap_or(cells).div_ceil(CELL_PARTS).max(1);
            let part = self.work_out(&block, cells, cells_done, tick, need, &times, &seasons, cancel, done)?;
            // The values are read: their block goes before the next piece's is read.
            drop(block);
            // Their full length when the cube knows it, so they don't grow by doubling: once the
            // first piece is worked out, so a run cancelled or refused before then hasn't taken it.
            if let (0, Some(total)) = (piece, whole) {
                for band in &mut bands {
                    band.reserve_exact(total);
                }
            }
            for (k, band) in bands.iter_mut().enumerate() {
                band.extend(part.chunks(OUTPUTS).map(|o| o.get(k).copied().unwrap_or(f32::NAN)));
            }
            cells_done += cells;
            stitch.add(grid);
        }
        let (grid, height) = stitch.whole()?;
        if let Some(e) = self.cube.nothing_read() {
            return Err(e);
        }
        done.store(self.total(), Ordering::Relaxed);
        let unit = if self.per == 10.0 { "decade" } else { "year" };
        let names = [
            format!("{} Sen slope per {unit}", self.name),
            format!("{} Z ({})", self.name, self.test.code()),
            format!("{} p-value ({})", self.name, self.test.code()),
            format!("{} significant slope per {unit} (p < {})", self.name, self.alpha),
        ];
        names
            .into_iter()
            .zip(bands)
            .map(|(name, band)| {
                Raster::new(name, grid.width, height, vec![band], grid.transform, grid.crs, None, raster::ColorHint::Gray, "Float32")
                    .map_err(|e| e.to_string())
            })
            .collect()
    }

    /// One piece's slices read into a block, cell by date: the piece's first slice (its grid), the
    /// block, and its cells. Dates are read [`TimeCube::readers`] at a time; `too_long` checks the
    /// run's length on the first slice when the cube didn't know its size.
    #[allow(clippy::type_complexity)]
    fn read_piece(
        &self,
        piece: usize,
        cancel: &AtomicBool,
        done: &AtomicUsize,
        too_long: Option<&dyn Fn(usize) -> Option<String>>,
    ) -> Result<(Arc<Raster>, Vec<f32>, usize), String> {
        let n = self.steps.len();
        let mut grid: Option<Arc<Raster>> = None;
        let mut block: Vec<f32> = Vec::new();
        let mut cells = 0usize;
        // Step `k`'s slice put in the block (a date with nothing written leaves its cells empty).
        let mut place = |k: usize, slice: Option<(Arc<Raster>, usize)>| -> Result<(), String> {
            let i = self.steps.get(k).copied().unwrap_or(0);
            let Some((r, b)) = slice else {
                done.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            };
            let vals = &r.bands.get(b).ok_or("the slice has no such band")?.data;
            match &grid {
                None => {
                    cells = vals.len();
                    let values = (cells as u64).saturating_mul(n as u64);
                    if values > MAX_VALUES {
                        return Err(format!(
                            "{cells} cells × {n} dates is {values} values, more than {MAX_VALUES}: take a smaller area or fewer dates"
                        ));
                    }
                    if let Some(e) = too_long.and_then(|f| f(cells)) {
                        return Err(e);
                    }
                    block = vec![f32::NAN; cells * n];
                    grid = Some(r.clone());
                }
                Some(g) => off_grid(self.cube.as_ref(), i, g, &r)?,
            }
            let nodata = r.nodata.map(|v| v as f32);
            for (c, v) in vals.iter().enumerate().take(cells) {
                if v.is_finite()
                    && Some(*v) != nodata
                    && let Some(slot) = block.get_mut(c * n + k)
                {
                    *slot = *v;
                }
            }
            done.fetch_add(1, Ordering::Relaxed);
            Ok(())
        };
        read_slices(self.cube.as_ref(), &self.steps, piece, cancel, &mut place)?;
        let grid = grid.ok_or("no date to read")?;
        Ok((grid, block, cells))
    }

    /// Each cell's trend in `block` (cells × dates), worked out on as many threads as there are
    /// cores: its four outputs a cell. `first` is the block's first cell among all of the run's,
    /// so progress is counted over the whole grid.
    #[allow(clippy::too_many_arguments)]
    fn work_out(
        &self,
        block: &[f32],
        cells: usize,
        first: usize,
        tick: usize,
        need: usize,
        times: &[f64],
        seasons: &[usize],
        cancel: &AtomicBool,
        done: &AtomicUsize,
    ) -> Result<Vec<f32>, String> {
        let n = self.steps.len();
        let mut out = vec![f32::NAN; cells * OUTPUTS];
        let stopped = AtomicBool::new(false);
        // Cancel is looked at after about the same work whatever the dates: a cell compares n²⁄2
        // pairs, so every 4096 cells for a short series, every cell for a long one.
        let every = (4_000_000 / (n * n).max(1)).clamp(1, 4096);
        // `first`: the part's first cell among all of them, so the parts are counted over the
        // whole grid, not again from 0 on each thread.
        let work = |first: usize, block: &[f32], out: &mut [f32]| {
            let mut ys = vec![0.0f64; n];
            // Sen's slopes, one buffer per thread, cleared cell by cell rather than made anew.
            let mut slopes: Vec<f64> = Vec::new();
            for (c, (vals, o)) in block.chunks(n).zip(out.chunks_mut(OUTPUTS)).enumerate() {
                if first + c > 0 && (first + c).is_multiple_of(tick) {
                    done.fetch_add(1, Ordering::Relaxed);
                }
                if c % every == 0 && (cancel.load(Ordering::Relaxed) || stopped.load(Ordering::Relaxed)) {
                    stopped.store(true, Ordering::Relaxed);
                    return;
                }
                for (y, v) in ys.iter_mut().zip(vals) {
                    *y = f64::from(*v);
                }
                if vals.iter().filter(|v| v.is_finite()).count() < need {
                    continue;
                }
                let found = match self.test {
                    Test::Seasonal(_) => trend::trend_seasonal_with(times, &ys, seasons, self.alpha, &mut slopes),
                    test => trend::trend_with(times, &ys, test, self.alpha, &mut slopes),
                };
                if let Some(t) = found
                    && let [slope, z, p, significant, ..] = o
                {
                    *slope = t.slope as f32;
                    *z = t.z as f32;
                    *p = t.p as f32;
                    *significant = if t.p < self.alpha { t.slope as f32 } else { f32::NAN };
                }
            }
        };
        // On the desktop, the cells in as many parts as there are cores; the browser has no
        // threads, so there they are worked out here.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let threads = std::thread::available_parallelism().map_or(1, |t| t.get()).clamp(1, 16);
            let part = cells.div_ceil(threads).max(1);
            std::thread::scope(|scope| {
                for (k, (block, out)) in block.chunks(part * n).zip(out.chunks_mut(part * OUTPUTS)).enumerate() {
                    let work = &work;
                    scope.spawn(move || work(first + k * part, block, out));
                }
            });
        }
        #[cfg(target_arch = "wasm32")]
        work(first, block, &mut out);
        if stopped.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        Ok(out)
    }
}

/// Reads piece `piece` of each of `steps`' slices and hands it to `place` with its place among the
/// steps: in their order when read one at a time, as they come when the cube's
/// [`TimeCube::readers`] read several at once. `cancel` stops it between slices.
fn read_slices(
    cube: &dyn TimeCube,
    steps: &[usize],
    piece: usize,
    cancel: &AtomicBool,
    place: &mut dyn FnMut(usize, Option<(Arc<Raster>, usize)>) -> Result<(), String>,
) -> Result<(), String> {
    // One at a time in the browser, which has no threads to read on.
    let readers = if cfg!(target_arch = "wasm32") { 1 } else { cube.readers().clamp(1, 16).min(steps.len().max(1)) };
    if readers == 1 {
        for (k, &i) in steps.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            place(k, cube.slice_of(i, piece)?)?;
        }
    } else {
        // Several dates read at once, placed here as they come; a few wait at most, so the
        // slices held stay a handful. An error (or Cancel) drops the receiver, which stops the
        // readers at their next date. A reader in a read when that happens finishes it first:
        // on a stalled server the scope waits out its timeout (two minutes, `cog`'s blocking
        // reads have no cancel hook), so Cancel can take that long to return.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let next = AtomicUsize::new(0);
            std::thread::scope(|scope| -> Result<(), String> {
                type Read = Result<Option<(Arc<Raster>, usize)>, String>;
                let (tx, rx) = std::sync::mpsc::sync_channel::<(usize, Read)>(readers);
                for _ in 0..readers {
                    let (tx, next) = (tx.clone(), &next);
                    scope.spawn(move || {
                        loop {
                            let k = next.fetch_add(1, Ordering::Relaxed);
                            let Some(&i) = steps.get(k) else { break };
                            if cancel.load(Ordering::Relaxed) || tx.send((k, cube.slice_of(i, piece))).is_err() {
                                break;
                            }
                        }
                    });
                }
                drop(tx);
                for (k, read) in rx {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("cancelled".into());
                    }
                    place(k, read?)?;
                }
                Ok(())
            })?;
        }
    }
    // Cancel stops the readers, which ends their dates early without an error: the slices are
    // only partly read, so the run stops here rather than work out what it has.
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    Ok(())
}

/// Refuses slice `r`, at date `i`, when it isn't on the grid of the run's first slice `g`.
fn off_grid(cube: &dyn TimeCube, i: usize, g: &Raster, r: &Raster) -> Result<(), String> {
    if (g.width, g.height, g.crs) == (r.width, r.height, r.crs) && same_grid(&g.transform, &r.transform) {
        return Ok(());
    }
    Err(format!(
        "the slice at {} is {} × {} on another grid than the first ({} × {}): put them on one grid first (Raster Functions › Resample)",
        rustgis_data::time::format(cube.dates().get(i).copied().unwrap_or(f64::NAN), "YYYY-MM-DD"),
        r.width,
        r.height,
        g.width,
        g.height
    ))
}

/// A run's pieces put together row by row, under the first piece's georeferencing.
#[derive(Default)]
struct Stitch {
    first: Option<Arc<Raster>>,
    rows: usize,
}

impl Stitch {
    /// Whether the next piece follows on from those above it: on the first's columns, pixel size
    /// and coordinate system, its top on the row after theirs.
    fn check(&self, grid: &Raster) -> Result<(), String> {
        let Some(f) = &self.first else { return Ok(()) };
        let near = |a: f64, b: f64| (a - b).abs() <= f.transform.pixel_width.abs() * 1e-3;
        let (a, b) = (&f.transform, &grid.transform);
        let top = a.origin_y + self.rows as f64 * a.pixel_height;
        if (f.width, f.crs) != (grid.width, grid.crs)
            || !near(a.origin_x, b.origin_x)
            || !near(top, b.origin_y)
            || !near(a.pixel_width, b.pixel_width)
            || !near(a.pixel_height, b.pixel_height)
        {
            return Err("the cube's pieces aren't one grid".into());
        }
        Ok(())
    }

    /// The next piece, below the others.
    fn add(&mut self, grid: Arc<Raster>) {
        self.rows += grid.height as usize;
        self.first.get_or_insert(grid);
    }

    /// The whole grid: the first piece's georeferencing, and the rows of them all.
    fn whole(self) -> Result<(Arc<Raster>, u32), String> {
        let grid = self.first.ok_or("no date to read")?;
        Ok((grid, u32::try_from(self.rows).map_err(|_| "too many rows")?))
    }
}

/// Prepares `cube.trend`: the cube, the dates, the test and its options checked.
pub fn trend_start(s: &Session, p: &Value) -> Result<TrendRun, String> {
    // Read for the seasonal test only: another test leaves it alone, whatever it holds.
    let seasonal = ps(p, "test").and_then(|t| Test::parse(t, 12)).is_some_and(|t| matches!(t, Test::Seasonal(_)));
    let seasons = match p.get("seasons").filter(|v| seasonal && !v.is_null()) {
        None => 12,
        Some(v) => v.as_u64().filter(|m| (2..=366).contains(m)).ok_or("seasons is a number from 2 to 366")? as u32,
    };
    let test = match ps(p, "test") {
        None => Test::HamedRao,
        Some(t) => Test::parse(t, seasons).ok_or_else(|| format!("unknown test '{t}' (hamedRao, yueWang, seasonal or original)"))?,
    };
    let alpha = match p.get("alpha").filter(|v| !v.is_null()) {
        None => 0.05,
        Some(v) => v.as_f64().filter(|a| *a > 0.0 && *a < 0.5).ok_or("alpha is a level between 0 and 0.5, such as 0.05")?,
    };
    let min_valid = match p.get("minValid").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => Some(v.as_u64().filter(|m| *m >= 1).ok_or("minValid is a number of dates")? as usize),
    };
    let per = match ps(p, "per").map(str::trim) {
        None | Some("") | Some("year") => 1.0,
        Some("decade") => 10.0,
        Some(x) => return Err(format!("per is year or decade, not {x}")),
    };
    let cube = cube_from(s, p, TREND)?;
    let period = cube.period();
    let steps = steps(cube.dates(), period, p)?;
    if steps.len() < trend::MIN_VALUES {
        return Err(format!("{} dates; a trend needs {} or more", steps.len(), trend::MIN_VALUES));
    }
    if steps.len() > MAX_DATES {
        return Err(format!("{} dates; a trend takes {MAX_DATES} at most: take a shorter period", steps.len()));
    }
    // More valid dates asked for than there are would leave every cell out.
    if let Some(m) = min_valid.filter(|m| *m > steps.len()) {
        return Err(format!("minValid is {m}, more than the {} dates", steps.len()));
    }
    let day = |i: usize| rustgis_data::time::format(cube.dates().get(i).copied().unwrap_or(f64::NAN), "YYYY-MM-DD");
    let (a, z) = (steps.first().copied().unwrap_or(0), steps.last().copied().unwrap_or(0));
    let label = format!("{} {}–{}", cube.label(), day(a), day(z));
    let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).map_or(label, |n| n.chars().take(200).collect());
    Ok(TrendRun { cube, steps, test, alpha, min_valid, per, name, max_pairs: MAX_PAIRS })
}

/// Whether two slices' grids are one: origins and pixel sizes within a thousandth of a pixel, so
/// files written apart (an ulp off) still stack.
pub(crate) fn same_grid(a: &rustgis_data::raster::GeoTransform, b: &rustgis_data::raster::GeoTransform) -> bool {
    let (w, h) = (a.pixel_width.abs().max(f64::MIN_POSITIVE), a.pixel_height.abs().max(f64::MIN_POSITIVE));
    let near = |x: f64, y: f64, unit: f64| (x - y).abs() <= unit * 1e-3;
    near(a.origin_x, b.origin_x, w)
        && near(a.origin_y, b.origin_y, h)
        && near(a.pixel_width, b.pixel_width, w)
        && near(a.pixel_height, b.pixel_height, h)
}

/// Keeps a trend worked out elsewhere (its layers and how many dates) in this session, for
/// `cube.trend` with `"ran"`: the id returned. The last two are kept (each is four full rasters),
/// and another project opened doesn't see them.
pub fn land(s: &mut Session, rasters: Vec<Raster>, dates: usize) -> u64 {
    s.next_pending = s.next_pending.wrapping_add(1);
    if s.trend_results.len() >= 2 {
        s.trend_results.remove(0);
    }
    s.trend_results.push((s.next_pending, rasters, dates));
    s.next_pending
}

/// `cube.trend`: the trend computed here (blocking), or the one the app worked out (`ran`), and
/// its four layers added, kept in the project as GeoTIFFs.
fn trend_cmd(s: &mut Session, p: &Value) -> CmdResult {
    // A run the app worked out: added if it is still kept (the last few are), never worked out
    // again here, which would block.
    if let Some(ran) = p.get("ran").filter(|v| !v.is_null()) {
        let id = ran.as_u64().ok_or("ran is the number of a run the app worked out")?;
        let at = s.trend_results.iter().position(|(k, ..)| *k == id).ok_or("that trend is no longer kept: run it again")?;
        let (_, rasters, dates) = s.trend_results.remove(at);
        return add_outputs(s, rasters, dates);
    }
    // Worked out here, waited for with no Cancel: in the app (a script, the assistant) and Python a
    // run of seconds, not minutes; where only a shell waits on it (`rustgis-cli run`), the panes'
    // bound, its progress told.
    let mut run = trend_start(s, p)?;
    run.max_pairs = if s.long_runs.is_some() { MAX_PAIRS } else { MAX_WAITED_PAIRS };
    let dates = run.steps.len();
    let rasters = run_here(s.long_runs, "cube.trend", run.total(), |cancel, done| run.run(cancel, done))?;
    let added = add_outputs(s, rasters, dates);
    told_end(s.long_runs, "cube.trend", added)
}

/// A run worked out where it's called: on a thread of its own telling its progress where only a
/// shell waits on it ([`Session::long_runs`](crate::Session::long_runs)), here
/// otherwise (and always in the browser, which has no threads). Its end is told by
/// [`told_end`], once the command has added what it made.
fn run_here<T: Send>(
    long_runs: Option<fn(&str, f64)>,
    what: &str,
    total: usize,
    work: impl FnOnce(&AtomicBool, &AtomicUsize) -> Result<T, String> + Send,
) -> Result<T, String> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(tell) = long_runs {
        return run_telling(what, total, tell, work);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (long_runs, what, total);
    work(&AtomicBool::new(false), &AtomicUsize::new(0))
}

/// A long command's result, its end (a share of 1) told first when it succeeded: after its layers
/// were added, so a failure to add them isn't told as done.
fn told_end(long_runs: Option<fn(&str, f64)>, what: &str, r: CmdResult) -> CmdResult {
    if r.is_ok()
        && let Some(tell) = long_runs
    {
        tell(what, 1.0);
    }
    r
}

/// A long run on a thread of its own, its progress told every half second when it has moved by a
/// percent, up to 99% (its outputs are still being made, and added, when every slice is read).
#[cfg(not(target_arch = "wasm32"))]
fn run_telling<T: Send>(
    what: &str,
    total: usize,
    tell: fn(&str, f64),
    work: impl FnOnce(&AtomicBool, &AtomicUsize) -> Result<T, String> + Send,
) -> Result<T, String> {
    use std::sync::mpsc::{RecvTimeoutError, channel};
    let (cancel, done) = (AtomicBool::new(false), AtomicUsize::new(0));
    let total = total.max(1);
    std::thread::scope(|scope| {
        let (sent, ended) = channel();
        let (c, d) = (&cancel, &done);
        let worker = scope.spawn(move || {
            let _ = sent.send(work(c, d));
        });
        let mut told = usize::MAX;
        // Waited for, not polled, so a short run returns as soon as it ends.
        let r = loop {
            match ended.recv_timeout(std::time::Duration::from_millis(500)) {
                Ok(r) => break Some(r),
                Err(RecvTimeoutError::Timeout) => {
                    let percent = (done.load(Ordering::Relaxed).min(total) * 100 / total).min(99);
                    if percent != told {
                        told = percent;
                        tell(what, percent as f64 / 100.0);
                    }
                }
                // The worker panicked before it sent anything.
                Err(RecvTimeoutError::Disconnected) => break None,
            }
        };
        // Joined here, so its panic is taken and said, not raised again when the scope ends.
        match (r, worker.join()) {
            (Some(r), _) => r,
            (None, Err(panic)) => {
                let why = panic.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| panic.downcast_ref::<String>().cloned());
                Err(match why {
                    Some(why) => format!("{what} failed unexpectedly: {why}"),
                    None => format!("{what} failed unexpectedly"),
                })
            }
            (None, Ok(())) => Err(format!("{what} failed unexpectedly")),
        }
    })
}

/// Adds a trend's (or a summary's) layers, each as a GeoTIFF the project keeps.
pub fn add_outputs(s: &mut Session, rasters: Vec<Raster>, dates: usize) -> CmdResult {
    let mut layers = Vec::new();
    for r in rasters {
        let name = r.name.clone();
        let tif = raster::geotiff::write(&r, 0).map_err(|e| e.to_string())?;
        let file: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect();
        let layer = rustgis_map::raster::RasterLayer::from_bytes(r, format!("{file}.tif"), tif);
        let out = super::raster::add_raster(s, &name, layer)?;
        layers.push(out["layer"].clone());
    }
    Ok(json!({ "layers": layers, "dates": dates }))
}

// --- A summary over time of any cube -------------------------------------------------------------

/// What a summary over time takes from each cell's values (`cube.reduce`, `zarr.reduce`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stat {
    Mean,
    Min,
    Max,
    Sum,
    Std,
    Count,
    /// How many of the values are above the threshold.
    CountAbove(f64),
}

impl Stat {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Stat::Mean => "mean",
            Stat::Min => "min",
            Stat::Max => "max",
            Stat::Sum => "sum",
            Stat::Std => "std",
            Stat::Count => "count",
            Stat::CountAbove(_) => "countAbove",
        }
    }

    /// The statistic asked for: `stat`, and countAbove's threshold, `above`.
    pub(crate) fn parse(p: &Value) -> Result<Stat, String> {
        Ok(match ps(p, "stat").map(str::trim).ok_or("stat is required (mean, min, max, sum, std, count or countAbove)")? {
            x if x.eq_ignore_ascii_case("mean") => Stat::Mean,
            x if x.eq_ignore_ascii_case("min") || x.eq_ignore_ascii_case("minimum") => Stat::Min,
            x if x.eq_ignore_ascii_case("max") || x.eq_ignore_ascii_case("maximum") => Stat::Max,
            x if x.eq_ignore_ascii_case("sum") => Stat::Sum,
            x if x.eq_ignore_ascii_case("std") => Stat::Std,
            x if x.eq_ignore_ascii_case("count") => Stat::Count,
            x if x.eq_ignore_ascii_case("countAbove") => {
                Stat::CountAbove(super::f(p, "above").ok_or("countAbove takes \"above\", the threshold")?)
            }
            x => return Err(format!("unknown stat '{x}' (mean, min, max, sum, std, count or countAbove)")),
        })
    }
}

/// The share of its dates a cell needs for a summary's value (`minValid`, 0 to 1; 0 by default).
pub(crate) fn min_share(p: &Value) -> Result<f64, String> {
    match p.get("minValid").filter(|v| !v.is_null()) {
        None => Ok(0.0),
        Some(v) => v.as_f64().filter(|v| (0.0..=1.0).contains(v)).ok_or_else(|| "minValid is a share from 0 to 1".into()),
    }
}

/// A summary's running totals over a run of cells, only those its statistic needs: each cell's
/// count of values, one running value (the mean, the least, the most, the sum or the count above)
/// and, for std, the squared deviations (Welford's method). 4 to 20 bytes a cell.
pub(crate) struct Totals {
    stat: Stat,
    n: Vec<u32>,
    acc: Vec<f64>,
    m2: Vec<f64>,
}

impl Totals {
    pub(crate) fn new(stat: Stat, cells: usize) -> Totals {
        let acc = match stat {
            Stat::Count => Vec::new(),
            Stat::Min => vec![f64::INFINITY; cells],
            Stat::Max => vec![f64::NEG_INFINITY; cells],
            _ => vec![0.0; cells],
        };
        let m2 = if matches!(stat, Stat::Std) { vec![0.0; cells] } else { Vec::new() };
        Totals { stat, n: vec![0; cells], acc, m2 }
    }

    /// How many cells it totals.
    pub(crate) fn cells(&self) -> usize {
        self.n.len()
    }

    /// Cell `c`'s next value, a finite one.
    pub(crate) fn add(&mut self, c: usize, v: f64) {
        let Some(k) = self.n.get_mut(c) else { return };
        *k += 1;
        let k = f64::from(*k);
        let (Some(a), m) = (self.acc.get_mut(c), self.m2.get_mut(c)) else { return };
        match self.stat {
            Stat::Mean | Stat::Std => {
                let d = v - *a;
                *a += d / k;
                if let Some(m) = m {
                    *m += d * (v - *a);
                }
            }
            Stat::Min => *a = a.min(v),
            Stat::Max => *a = a.max(v),
            Stat::Sum => *a += v,
            Stat::CountAbove(t) if v > t => *a += 1.0,
            Stat::CountAbove(_) | Stat::Count => {}
        }
    }

    /// Each cell's statistic over `dates` dates: empty with fewer values than `min_share` of them,
    /// except that with no share asked for a count is 0 where a cell never had a value.
    pub(crate) fn finish(&self, min_share: f64, dates: usize) -> Vec<f32> {
        let need = (min_share.clamp(0.0, 1.0) * dates as f64 - 1e-9).ceil().max(1.0) as u32;
        let count = matches!(self.stat, Stat::Count | Stat::CountAbove(_));
        self.n
            .iter()
            .enumerate()
            .map(|(c, k)| {
                if *k < need && !(count && min_share == 0.0) {
                    return f32::NAN;
                }
                let a = self.acc.get(c).copied().unwrap_or(f64::NAN);
                (match self.stat {
                    Stat::Std => (self.m2.get(c).copied().unwrap_or(f64::NAN) / f64::from((*k).max(1))).sqrt(),
                    Stat::Count => f64::from(*k),
                    _ => a,
                }) as f32
            })
            .collect()
    }
}

/// The most values a summary's outputs hold (cells × outputs): 32 million on the desktop, 8
/// million in the browser, checked before reading when the cube knows its size and as the pieces
/// are read when it doesn't. Its running totals are a piece's at a time and only for the groups
/// whose dates are being read: one year's in a summary by year read one date at a time (those of
/// the dates in flight with several readers), but every month's in one by month. They take 4 bytes
/// a cell for count, 12 for mean, min, max, sum and countAbove, and 20 for std, so a summary by
/// month at the cap holds up to five times its outputs' 4 bytes a value: some 640 MB for std on the
/// desktop. A STAC cube is read in pieces whose cells × dates stay within [`MAX_VALUES`], so a long
/// series keeps each piece's totals small.
pub const MAX_REDUCED: u64 = MAX_VALUES / 2;

/// How a summary groups its dates: one output for them all, one for each calendar year, or one for
/// each month of the year across the years (a climatology).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum By {
    All,
    Year,
    Month,
}

/// A summary over time of any cube (#579), prepared on the session and computed anywhere with
/// [`ReduceRun::run`]: the slices folded into running totals as they are read, so a long period
/// takes no more memory than a short one.
pub struct ReduceRun {
    cube: Box<dyn TimeCube>,
    steps: Vec<usize>,
    stat: Stat,
    min_share: f64,
    /// Each step's group, an index into `groups`.
    group_of: Vec<usize>,
    /// Each group's output name and how many of the steps it has.
    groups: Vec<(String, usize)>,
}

impl ReduceRun {
    /// The work, for a progress bar: the slices read, every piece's.
    pub fn total(&self) -> usize {
        self.steps.len() * self.cube.pieces().max(1)
    }

    /// How many dates it reads.
    pub fn dates(&self) -> usize {
        self.steps.len()
    }

    /// Reads the slices piece by piece ([`TimeCube::pieces`]), each folded into its group's
    /// running totals, and puts the pieces' outputs together row by row: a raster for each group.
    /// `done` counts the slices read; `cancel` stops it between slices.
    pub fn run(&self, cancel: &AtomicBool, done: &AtomicUsize) -> Result<Vec<Raster>, String> {
        let outputs = self.groups.len();
        let too_many = |cells: usize| {
            let values = (cells as u64).saturating_mul(outputs as u64);
            (values > MAX_REDUCED).then(|| {
                format!("{cells} cells × {outputs} outputs is {values} values, more than {MAX_REDUCED}: take a smaller area, a coarser cell size or fewer outputs")
            })
        };
        // A cube that knows its size is refused before it reads anything.
        let whole = self.cube.size().map(|(w, h)| w.saturating_mul(h));
        if let Some(e) = whole.and_then(too_many) {
            return Err(e);
        }
        let mut stitch = Stitch::default();
        let mut bands: Vec<Vec<f32>> = vec![Vec::new(); outputs];
        // The cells of the pieces read so far, for a cube that didn't know its size.
        let mut read_cells = 0usize;
        for piece in 0..self.cube.pieces().max(1) {
            let mut grid: Option<Arc<Raster>> = None;
            // A group's totals are made at its first date read and turned into its output at its
            // last. Dates are asked for in order: read one at a time they come in order, so a
            // summary by year holds one year's totals at a time; with several readers (a STAC
            // cube's) they come as read, and the groups open are those of the dates in flight, a
            // couple of years' for 16-day composites. Totals are f64 and the outputs f32, so the
            // order of arrival can move an output's last bit at most, and only then.
            let mut totals: Vec<Option<Totals>> = (0..outputs).map(|_| None).collect();
            let mut seen = vec![0usize; outputs];
            let mut finished: Vec<Option<Vec<f32>>> = vec![None; outputs];
            let mut place = |k: usize, slice: Option<(Arc<Raster>, usize)>| -> Result<(), String> {
                let g = self.group_of.get(k).copied().filter(|g| *g < outputs).ok_or("a date in no group")?;
                // A date with nothing written only lowers the counts.
                if let Some((r, b)) = slice {
                    let i = self.steps.get(k).copied().unwrap_or(0);
                    let vals = &r.bands.get(b).ok_or("the slice has no such band")?.data;
                    match &grid {
                        None => {
                            if let Some(e) = too_many(vals.len()) {
                                return Err(e);
                            }
                            grid = Some(r.clone());
                        }
                        Some(first) => off_grid(self.cube.as_ref(), i, first, &r)?,
                    }
                    let t = totals.get_mut(g).ok_or("a date in no group")?.get_or_insert_with(|| Totals::new(self.stat, vals.len()));
                    let nodata = r.nodata.map(|v| v as f32);
                    for (c, v) in vals.iter().enumerate().take(t.cells()) {
                        if v.is_finite() && Some(*v) != nodata {
                            t.add(c, f64::from(*v));
                        }
                    }
                }
                let n = seen.get_mut(g).ok_or("a date in no group")?;
                *n += 1;
                let dates = self.groups.get(g).map_or(0, |(_, d)| *d);
                if *n >= dates
                    && let (Some(t), Some(out)) = (totals.get_mut(g).and_then(Option::take), finished.get_mut(g))
                {
                    *out = Some(t.finish(self.min_share, dates));
                }
                done.fetch_add(1, Ordering::Relaxed);
                Ok(())
            };
            read_slices(self.cube.as_ref(), &self.steps, piece, cancel, &mut place)?;
            let grid = grid.ok_or("none of the dates in the period has data written")?;
            stitch.check(&grid)?;
            // Their full length when the cube knows it, so they don't grow by doubling.
            if let (0, Some(total)) = (piece, whole) {
                for band in &mut bands {
                    band.reserve_exact(total);
                }
            }
            let cells = (grid.width as usize).saturating_mul(grid.height as usize);
            read_cells = read_cells.saturating_add(cells);
            if whole.is_none()
                && let Some(e) = too_many(read_cells)
            {
                return Err(e);
            }
            for (g, band) in bands.iter_mut().enumerate() {
                let dates = self.groups.get(g).map_or(0, |(_, d)| *d);
                // A group with no data written in this piece: no values, as its totals would give.
                let out = match (finished.get_mut(g).and_then(Option::take), totals.get_mut(g).and_then(Option::take)) {
                    (Some(out), _) => out,
                    (None, Some(t)) => t.finish(self.min_share, dates),
                    (None, None) => Totals::new(self.stat, cells).finish(self.min_share, dates),
                };
                band.extend(out);
            }
            stitch.add(grid);
        }
        let (grid, height) = stitch.whole()?;
        if let Some(e) = self.cube.nothing_read() {
            return Err(e);
        }
        done.store(self.total(), Ordering::Relaxed);
        self.groups
            .iter()
            .zip(bands)
            .map(|((name, _), band)| {
                Raster::new(
                    name.clone(),
                    grid.width,
                    height,
                    vec![band],
                    grid.transform,
                    grid.crs,
                    None,
                    raster::ColorHint::Gray,
                    "Float32",
                )
                .map_err(|e| e.to_string())
            })
            .collect()
    }
}

/// Prepares `cube.reduce`: the cube, the statistic, and the dates of the period (and months)
/// asked for, in their groups.
pub fn reduce_start(s: &Session, p: &Value) -> Result<ReduceRun, String> {
    let stat = Stat::parse(p)?;
    let min_share = min_share(p)?;
    let by = match ps(p, "by").map(str::trim) {
        None | Some("") => By::All,
        Some(x) if x.eq_ignore_ascii_case("all") => By::All,
        Some(x) if x.eq_ignore_ascii_case("year") => By::Year,
        Some(x) if x.eq_ignore_ascii_case("month") => By::Month,
        Some(x) => return Err(format!("by is all, year or month, not {x}")),
    };
    let cube = cube_from(s, p, SUMMARY)?;
    let steps = steps(cube.dates(), cube.period(), p)?;
    let date = |i: usize| cube.dates().get(i).copied().unwrap_or(f64::NAN);
    // A date that can't be read would go to no year or month: said, not put in a group of its own.
    if let Some(&i) = steps.iter().find(|&&i| !date(i).is_finite()) {
        return Err(format!("the cube's date {} can't be read: give the dates (\"dates\")", i + 1));
    }
    let part = |i: usize, f: &str| rustgis_data::time::format(date(i), f).parse::<u32>().unwrap_or(0);
    // Each step's group: its calendar year, its month, or the one group.
    let keys: Vec<u32> = steps
        .iter()
        .map(|&i| match by {
            By::All => 0,
            By::Year => part(i, "YYYY"),
            By::Month => part(i, "MM"),
        })
        .collect();
    let mut found: Vec<u32> = keys.clone();
    found.sort_unstable();
    found.dedup();
    let group_of: Vec<usize> = keys.iter().map(|k| found.binary_search(k).unwrap_or(0)).collect();
    // The outputs' names: the cube's label, the statistic and the period, or the name given.
    let day = |i: usize| rustgis_data::time::format(date(i), "YYYY-MM-DD");
    // The earliest and the latest date, which a Zarr axis out of order doesn't put first and last.
    let earliest = steps.iter().copied().min_by(|x, y| date(*x).total_cmp(&date(*y)));
    let latest = steps.iter().copied().max_by(|x, y| date(*x).total_cmp(&date(*y)));
    let (a, z) = (earliest.unwrap_or(0), latest.unwrap_or(0));
    let months = months(p)?;
    let season = if months.is_empty() {
        String::new()
    } else {
        format!(" (months {})", months.iter().map(u32::to_string).collect::<Vec<_>>().join(","))
    };
    let given = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(|n| n.chars().take(200).collect::<String>());
    let base = format!("{} {}", cube.label(), stat.code());
    let groups = found
        .iter()
        .map(|&key| {
            let name = match (by, &given) {
                (By::All, Some(n)) => n.clone(),
                (By::All, None) => format!("{base} {}–{}{season}", day(a), day(z)),
                (By::Year, Some(n)) => format!("{n} {key}"),
                (By::Year, None) => format!("{base} {key}{season}"),
                (By::Month, Some(n)) => format!("{n} month {key:02}"),
                (By::Month, None) => format!("{base} {}–{} month {key:02}", part(a, "YYYY"), part(z, "YYYY")),
            };
            (name, keys.iter().filter(|k| **k == key).count())
        })
        .collect();
    Ok(ReduceRun { cube, steps, stat, min_share, group_of, groups })
}

/// `cube.reduce`: the summary computed here (blocking), its progress told where only a shell waits
/// on it (`rustgis-cli run`), its layers added, each kept in the project as a GeoTIFF.
fn reduce_cmd(s: &mut Session, p: &Value) -> CmdResult {
    let run = reduce_start(s, p)?;
    let rasters = run_here(s.long_runs, "cube.reduce", run.total(), |cancel, done| run.run(cancel, done))?;
    let added = add_outputs(s, rasters, run.dates());
    told_end(s.long_runs, "cube.reduce", added)
}

#[cfg(test)]
mod tests {
    /// The outputs' names: the date taken out, a MODIS date's A with it, and no other final A.
    #[test]
    fn labels_lose_their_date() {
        assert_eq!(super::common_label("NDVI_2024-06-01"), "NDVI");
        assert_eq!(super::common_label("MOD13Q1.A2024153"), "MOD13Q1");
        assert_eq!(super::common_label("sentinel2_ndvi_2024-06-01"), "sentinel2_ndvi");
        assert_eq!(super::common_label("ndvi.A2024153.h12v10"), "ndvi");
        assert_eq!(super::common_label("NDVI_BETA2020"), "NDVI_BETA");
        assert_eq!(super::common_label("ETA_2020"), "ETA");
    }

    /// Dates in names: a full date anywhere before a year alone; a year only standing apart.
    #[test]
    fn dates_in_names() {
        let day = |d: &str| rustgis_data::time::parse_text(d);
        assert_eq!(super::date_in_text("ndvi_2015-06-01"), day("2015-06-01"));
        assert_eq!(super::date_in_text("res2000_ndvi_20150601"), day("2015-06-01"));
        assert_eq!(super::date_in_text("NDVI 2015"), day("2015-01-01"));
        assert_eq!(super::date_in_text("ndvi_v2015"), None);
        assert_eq!(super::date_in_text("MOD13Q1.A2024153"), day("2024-06-01"));
    }

    /// A long run's thread that panics gives an error naming the command and the cause, and its
    /// end is never told.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[allow(clippy::panic)]
    fn a_long_run_that_panics_says_why() {
        fn tell(_: &str, share: f64) {
            assert!(share < 1.0, "a run that panicked is told as done");
        }
        let e = super::run_telling::<()>("cube.trend", 10, tell, |_, _| panic!("a band out of range")).unwrap_err();
        assert_eq!(e, "cube.trend failed unexpectedly: a band out of range");
        let e = super::run_telling::<()>("cube.reduce", 10, tell, |_, _| std::panic::panic_any(7u8)).unwrap_err();
        assert_eq!(e, "cube.reduce failed unexpectedly");
    }

    /// Slices an ulp apart are one grid; half a pixel apart, another.
    #[test]
    fn grids_within_a_hair_are_one() {
        use rustgis_data::raster::GeoTransform;
        let a = GeoTransform { origin_x: -60.0, origin_y: 5.0, pixel_width: 0.25, pixel_height: -0.25 };
        let hair = GeoTransform { origin_x: -60.0 + 1e-12, ..a };
        let half = GeoTransform { origin_x: -60.125, ..a };
        assert!(super::same_grid(&a, &hair));
        assert!(!super::same_grid(&a, &half));
    }
}
