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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rustgis_analysis::trend::{self, Test};
use rustgis_data::raster::{self, Raster};
use serde_json::{Value, json};

use super::ps;
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![C::new("cube.trend", "Trend Analysis", "Analyze › Image Analysis", trend_cmd).params(
        r#"{"layers"?: [raster layers, a date each] | "layer"?: a Zarr layer (or "path" | "url", "variable", "index", "crs" as for zarr.reduce) or a raster of several bands, a date each (default: the active layer), "band"?: 1-based (a stack's band), "dates"?: [dates] (a stack's or the bands' dates, in their order; default: read from the layers' names or files, or the bands' names), "start"?, "end"?: dates, "months"?: [1–12] (one season across years), "test"?: "hamedRao" (default) | "yueWang" | "seasonal" | "original", "seasons"?: parts of a year for the seasonal test (default 12), "alpha"?: 0.05, "minValid"?: dates a cell needs (default: half of them, at least 4), "per"?: "year" (default) | "decade", "name"?: text, "ran"?: (internal: a trend the app has worked out)} → four raster layers: Sen's slope, the test's Z and p-value, and the slope where p < alpha"#,
    )]
}

/// The most cell values a cube is read into at once (cells × dates): 64 million, 256 MB.
pub const MAX_VALUES: u64 = 64_000_000;
/// The most pairs of dates a trend compares in all (cells × dates² ⁄ 2), which bounds the time
/// Sen's slope takes.
pub const MAX_PAIRS: u64 = 20_000_000_000;
/// The most dates a cube is read at.
pub const MAX_DATES: usize = 2_000;

/// Rasters on one grid at a series of dates.
pub trait TimeCube: Send + Sync {
    /// The dates, in milliseconds since 1970, in the cube's order.
    fn dates(&self) -> &[f64];
    /// The slice at date `i`: a raster and the band holding the values.
    fn slice(&self, i: usize) -> Result<(Arc<Raster>, usize), String>;
    /// What the cube holds, for the outputs' names.
    fn label(&self) -> String;
    /// The steps taken when no period is asked for (a following Zarr layer's), as a range.
    fn period(&self) -> (usize, usize) {
        (0, self.dates().len())
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
    fn slice(&self, i: usize) -> Result<(Arc<Raster>, usize), String> {
        self.items.get(i).map(|(_, r, b)| (r.clone(), *b)).ok_or_else(|| format!("no date {i}"))
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
    fn slice(&self, i: usize) -> Result<(Arc<Raster>, usize), String> {
        Ok((super::zarr::read_step(&self.0.zt, i)?, 0))
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
    let starts = |i: usize| i == 0 || !b[i - 1].is_ascii_digit();
    let ends = |i: usize| i >= b.len() || !b[i].is_ascii_digit();
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
        if ends(i + 4)
            && let Some(ms) = day(y, 1, 1)
        {
            return Some(ms);
        }
    }
    None
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

/// The cube a command's parameters name: a stack of layers, a Zarr variable, or a raster's bands.
pub fn cube_from(s: &Session, p: &Value) -> Result<Box<dyn TimeCube>, String> {
    let dates = date_list(p)?;
    if let Some(list) = p.get("layers").filter(|v| !v.is_null()) {
        let list = list.as_array().ok_or("layers is a list of raster layers")?;
        if list.len() < trend::MIN_VALUES {
            return Err(format!("a stack needs {} layers or more, a date each", trend::MIN_VALUES));
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
            let file = l.raster.as_ref().and_then(|r| r.path.clone()).unwrap_or_default();
            let date = match &dates {
                Some(d) => d[k],
                None => date_in_text(&l.name)
                    .or_else(|| date_in_text(&file))
                    .ok_or_else(|| format!("no date in the name of {}: give \"dates\"", l.name))?,
            };
            if label.is_empty() {
                label = l.name.clone();
            }
            items.push((date, r, band));
        }
        return Ok(Box::new(Stack::new(items, common_label(&label))));
    }
    let given = |k: &str| p.get(k).is_some_and(|v| !v.is_null());
    #[cfg(not(target_arch = "wasm32"))]
    {
        let zarr_layer = !given("path") && !given("url") && {
            let id = s.layer_id(p)?;
            s.map().layer(id).and_then(|l| l.raster.as_ref()).is_some_and(|r| r.time_zarr.is_some() || r.subdataset.is_some())
        };
        if given("path") || given("url") || zarr_layer {
            return Ok(Box::new(ZarrCube(super::zarr::zarr_cube(s, p)?)));
        }
    }
    let id = s.layer_id(p)?;
    let l = s.map().layer(id).ok_or("no such layer")?;
    let r = l.raster.as_ref().and_then(|r| r.data.clone()).ok_or_else(|| format!("{} isn't a raster", l.name))?;
    if r.bands.len() < trend::MIN_VALUES {
        return Err(format!(
            "{} has {} band(s): a cube is a raster of {} bands or more (a date each), a Zarr variable with a time dimension, or \"layers\"",
            l.name,
            r.bands.len(),
            trend::MIN_VALUES
        ));
    }
    if dates.as_ref().is_some_and(|d| d.len() != r.bands.len()) {
        return Err(format!("dates has a date for each of the {} bands", r.bands.len()));
    }
    let mut items = Vec::with_capacity(r.bands.len());
    for (b, band) in r.bands.iter().enumerate() {
        let date = match &dates {
            Some(d) => d[b],
            None => {
                date_in_text(&band.name).ok_or_else(|| format!("band {} ({}) has no date in its name: give \"dates\"", b + 1, band.name))?
            }
        };
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

/// A name with its date taken out (`NDVI_2024-06-01` → `NDVI`), for the outputs' names.
fn common_label(name: &str) -> String {
    let cut: String = name.chars().take_while(|c| !c.is_ascii_digit()).collect();
    let cut = cut.trim_end_matches(['_', '-', ' ', '.', 'A']);
    if cut.is_empty() { name.to_string() } else { cut.to_string() }
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
    let months: Vec<u32> = match p.get("months").filter(|v| !v.is_null()) {
        None => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|m| m.as_u64().filter(|m| (1..=12).contains(m)).map(|m| m as u32).ok_or("months are numbers from 1 to 12"))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("months is a list of months (1–12)".into()),
    };
    let (start, end) = (date("start")?, date("end")?);
    let (lo, hi) = match (start, end) {
        (None, None) => (period.0.min(dates.len()), period.1.min(dates.len())),
        _ => (0, dates.len()),
    };
    let month = |d: f64| rustgis_data::time::format(d, "MM").parse::<u32>().unwrap_or(0);
    let out: Vec<usize> = (lo..hi)
        .filter(|i| {
            let d = dates[*i];
            start.is_none_or(|a| d >= a) && end.is_none_or(|z| d <= z) && (months.is_empty() || months.contains(&month(d)))
        })
        .collect();
    if out.is_empty() {
        return Err("no date of the cube falls in the period (and months) asked for".into());
    }
    Ok(out)
}

/// What a trend writes for each cell.
const OUTPUTS: usize = 4;

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
}

impl TrendRun {
    /// The work, for a progress bar: the slices read, then the cells worked out.
    pub fn total(&self) -> usize {
        self.steps.len() + 1
    }

    /// How many dates it reads.
    pub fn dates(&self) -> usize {
        self.steps.len()
    }

    /// Reads the slices into one block, cell by date, then works each cell's trend out on as
    /// many threads as there are cores. `done` counts the slices read, and the last unit once
    /// the cells are done; `cancel` stops it between slices or rows.
    pub fn run(&self, cancel: &AtomicBool, done: &AtomicUsize) -> Result<Vec<Raster>, String> {
        let n = self.steps.len();
        let mut grid: Option<Arc<Raster>> = None;
        let mut block: Vec<f32> = Vec::new();
        let mut cells = 0usize;
        for (k, &i) in self.steps.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let (r, b) = self.cube.slice(i)?;
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
                    let pairs = (cells as u64).saturating_mul((n * n.saturating_sub(1) / 2) as u64);
                    if pairs > MAX_PAIRS {
                        return Err(format!("{cells} cells × {n} dates is too long a run: take a smaller area or fewer dates"));
                    }
                    block = vec![f32::NAN; cells * n];
                    grid = Some(r.clone());
                }
                Some(g) if (g.width, g.height, g.transform) != (r.width, r.height, r.transform) => {
                    return Err(format!(
                        "the slice at {} is {} × {} on another grid than the first ({} × {}): put them on one grid first (Raster Functions › Resample)",
                        rustgis_data::time::format(self.cube.dates()[i], "YYYY-MM-DD"),
                        r.width,
                        r.height,
                        g.width,
                        g.height
                    ));
                }
                Some(_) => {}
            }
            let nodata = r.nodata.map(|v| v as f32);
            for (c, v) in vals.iter().enumerate().take(cells) {
                if v.is_finite() && Some(*v) != nodata {
                    block[c * n + k] = *v;
                }
            }
            done.fetch_add(1, Ordering::Relaxed);
        }
        let grid = grid.ok_or("no date to read")?;
        // Times in the slope's unit.
        let year = 365.2425 * 86_400_000.0 * self.per;
        let times: Vec<f64> = self.steps.iter().map(|i| self.cube.dates()[*i] / year).collect();
        let need = self.min_valid.unwrap_or((n / 2).max(trend::MIN_VALUES)).max(trend::MIN_VALUES);
        let mut out = vec![f32::NAN; cells * OUTPUTS];
        let stopped = AtomicBool::new(false);
        // A part of the cells, worked out in place.
        let work = |block: &[f32], out: &mut [f32]| {
            let mut ys = vec![0.0f64; n];
            for (c, (vals, o)) in block.chunks(n).zip(out.chunks_mut(OUTPUTS)).enumerate() {
                if c % 4096 == 0 && (cancel.load(Ordering::Relaxed) || stopped.load(Ordering::Relaxed)) {
                    stopped.store(true, Ordering::Relaxed);
                    return;
                }
                for (y, v) in ys.iter_mut().zip(vals) {
                    *y = f64::from(*v);
                }
                if vals.iter().filter(|v| v.is_finite()).count() < need {
                    continue;
                }
                if let Some(t) = trend::trend(&times, &ys, self.test, self.alpha) {
                    o[0] = t.slope as f32;
                    o[1] = t.z as f32;
                    o[2] = t.p as f32;
                    o[3] = if t.p < self.alpha { t.slope as f32 } else { f32::NAN };
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
                for (block, out) in block.chunks(part * n).zip(out.chunks_mut(part * OUTPUTS)) {
                    let work = &work;
                    scope.spawn(move || work(block, out));
                }
            });
        }
        #[cfg(target_arch = "wasm32")]
        work(&block, &mut out);
        if stopped.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        done.fetch_add(1, Ordering::Relaxed);
        let unit = if self.per == 10.0 { "decade" } else { "year" };
        let names = [
            format!("{} Sen slope per {unit}", self.name),
            format!("{} Z ({})", self.name, self.test.code()),
            format!("{} p-value ({})", self.name, self.test.code()),
            format!("{} significant slope per {unit} (p < {})", self.name, self.alpha),
        ];
        names
            .into_iter()
            .enumerate()
            .map(|(k, name)| {
                let band: Vec<f32> = out.chunks(OUTPUTS).map(|o| o[k]).collect();
                Raster::new(name, grid.width, grid.height, vec![band], grid.transform, grid.crs, None, raster::ColorHint::Gray, "Float32")
                    .map_err(|e| e.to_string())
            })
            .collect()
    }
}

/// Prepares `cube.trend`: the cube, the dates, the test and its options checked.
pub fn trend_start(s: &Session, p: &Value) -> Result<TrendRun, String> {
    let seasons = match p.get("seasons").filter(|v| !v.is_null()) {
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
    let cube = cube_from(s, p)?;
    let period = cube.period();
    let steps = steps(cube.dates(), period, p)?;
    if steps.len() < trend::MIN_VALUES {
        return Err(format!("{} dates; a trend needs {} or more", steps.len(), trend::MIN_VALUES));
    }
    if steps.len() > MAX_DATES {
        return Err(format!("{} dates; a trend takes {MAX_DATES} at most: take a shorter period", steps.len()));
    }
    let day = |i: usize| rustgis_data::time::format(cube.dates()[i], "YYYY-MM-DD");
    let (a, z) = (steps.first().copied().unwrap_or(0), steps.last().copied().unwrap_or(0));
    let label = format!("{} {}–{}", cube.label(), day(a), day(z));
    let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).map_or(label, |n| n.chars().take(200).collect());
    Ok(TrendRun { cube, steps, test, alpha, min_valid, per, name })
}

/// Trends the desktop app worked out on a thread, waiting to be added by `cube.trend`'s `ran`.
fn landed() -> &'static std::sync::Mutex<Vec<(u64, Vec<Raster>, usize)>> {
    static LANDED: std::sync::OnceLock<std::sync::Mutex<Vec<(u64, Vec<Raster>, usize)>>> = std::sync::OnceLock::new();
    LANDED.get_or_init(Default::default)
}

/// Keeps a trend worked out elsewhere (its layers and how many dates), for `cube.trend` with
/// `"ran"`: the id returned. The last few are kept.
pub fn land(rasters: Vec<Raster>, dates: usize) -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut l) = landed().lock() {
        if l.len() >= 4 {
            l.remove(0);
        }
        l.push((id, rasters, dates));
    }
    id
}

/// `cube.trend`: the trend computed here (blocking), or the one the app worked out (`ran`), and
/// its four layers added, kept in the project as GeoTIFFs.
fn trend_cmd(s: &mut Session, p: &Value) -> CmdResult {
    if let Some(id) = p.get("ran").and_then(Value::as_u64)
        && let Some((rasters, dates)) = landed().lock().ok().and_then(|mut l| {
            let at = l.iter().position(|(k, ..)| *k == id)?;
            let (_, r, n) = l.remove(at);
            Some((r, n))
        })
    {
        return add_outputs(s, rasters, dates);
    }
    let run = trend_start(s, p)?;
    let rasters = run.run(&AtomicBool::new(false), &AtomicUsize::new(0))?;
    add_outputs(s, rasters, run.steps.len())
}

/// Adds a trend's layers, each as a GeoTIFF the project keeps.
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
