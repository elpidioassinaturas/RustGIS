//! Raster operations: terrain (slope, aspect, hillshade), band arithmetic, remapping, resampling
//! onto another grid, weighted overlay, viewsheds, lines of sight, profiles, IDW interpolation,
//! k-means classification and change detection. Outputs are new pixel arrays on a raster's grid.

use std::ops::Range;

use rustgis_data::raster::{ColorHint, Crs, GeoTransform, Raster};
use rustgis_geom::{Coord, Extent};

/// The no-data value written by these operations.
pub const NODATA: f32 = -3.4e38;
/// Largest output raster built (pixels).
pub const MAX_PIXELS: usize = 64_000_000;

/// A new single- or multi-band raster on `r`'s grid.
pub fn derive(r: &Raster, name: &str, bands: Vec<Vec<f32>>) -> Result<Raster, String> {
    r.require_coordinates()?;
    Raster::new(name, r.width, r.height, bands, r.transform, r.crs, Some(NODATA as f64), ColorHint::Gray, "Float32")
        .map_err(|e| e.to_string())
}

fn band(r: &Raster, b: usize) -> Result<&[f32], String> {
    r.bands.get(b).map(|b| b.data.as_slice()).ok_or_else(|| format!("{} has no band {}", r.name, b + 1))
}

/// Elevation at (col, row), clamped to the grid edge; `None` for no-data.
fn z(r: &Raster, data: &[f32], col: i64, row: i64) -> Option<f32> {
    let c = col.clamp(0, r.width as i64 - 1) as usize;
    let rr = row.clamp(0, r.height as i64 - 1) as usize;
    data.get(rr * r.width as usize + c).copied().filter(|v| r.is_valid(*v))
}

/// The surface gradient (dz/dx east, dz/dy north) at a cell (Horn's method), in metres per metre.
fn gradient(r: &Raster, data: &[f32], col: i64, row: i64, zf: f64) -> Option<(f64, f64)> {
    let (cx, cy) = r.cell_size_m();
    let g = |dc: i64, dr: i64| z(r, data, col + dc, row + dr).or_else(|| z(r, data, col, row)).map(|v| v as f64 * zf);
    let (a, b, c) = (g(-1, -1)?, g(0, -1)?, g(1, -1)?);
    let (d, f) = (g(-1, 0)?, g(1, 0)?);
    let (gg, h, i) = (g(-1, 1)?, g(0, 1)?, g(1, 1)?);
    let dzdx = ((c + 2.0 * f + i) - (a + 2.0 * d + gg)) / (8.0 * cx);
    // Rows run south, so north is up: the difference is top minus bottom.
    let dzdy = ((a + 2.0 * b + c) - (gg + 2.0 * h + i)) / (8.0 * cy);
    Some((dzdx, dzdy))
}

fn per_cell(r: &Raster, f: impl Fn(i64, i64) -> Option<f64>) -> Vec<f32> {
    let mut out = Vec::with_capacity(r.width as usize * r.height as usize);
    for row in 0..r.height as i64 {
        for col in 0..r.width as i64 {
            out.push(f(col, row).filter(|v| v.is_finite()).map(|v| v as f32).unwrap_or(NODATA));
        }
    }
    out
}

/// Slope in degrees.
pub fn slope(r: &Raster, b: usize, zf: f64) -> Result<Vec<f32>, String> {
    let d = band(r, b)?;
    Ok(per_cell(r, |c, row| {
        z(r, d, c, row)?;
        let (gx, gy) = gradient(r, d, c, row, zf)?;
        Some((gx.hypot(gy)).atan().to_degrees())
    }))
}

/// Aspect: the compass direction the slope faces, in degrees (flat cells are -1).
pub fn aspect(r: &Raster, b: usize) -> Result<Vec<f32>, String> {
    let d = band(r, b)?;
    Ok(per_cell(r, |c, row| {
        z(r, d, c, row)?;
        let (gx, gy) = gradient(r, d, c, row, 1.0)?;
        if gx.abs() < 1e-12 && gy.abs() < 1e-12 {
            return Some(-1.0);
        }
        // Downhill is -gradient; its compass bearing.
        Some((-gx).atan2(-gy).to_degrees().rem_euclid(360.0))
    }))
}

/// Hillshade (0–255) lit from `azimuth` and `altitude` degrees.
pub fn hillshade(r: &Raster, b: usize, azimuth: f64, altitude: f64, zf: f64) -> Result<Vec<f32>, String> {
    let d = band(r, b)?;
    let (zen, az) = ((90.0 - altitude).to_radians(), azimuth.to_radians());
    Ok(per_cell(r, |c, row| {
        z(r, d, c, row)?;
        let (gx, gy) = gradient(r, d, c, row, zf)?;
        let slope = gx.hypot(gy).atan();
        let aspect = (-gx).atan2(-gy);
        let v = zen.cos() * slope.cos() + zen.sin() * slope.sin() * (az - aspect).cos();
        Some((v.max(0.0) * 255.0).round())
    }))
}

/// A band arithmetic expression over `b1`, `b2`… (+ - * / and parentheses, numbers).
pub fn band_math(r: &Raster, expr: &str) -> Result<Vec<f32>, String> {
    let e = Expr::parse(expr, r.band_count())?;
    let n = r.width as usize * r.height as usize;
    let mut vals = vec![0.0f64; r.band_count()];
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let mut ok = true;
        for (bi, b) in r.bands.iter().enumerate() {
            let v = b.data.get(i).copied().unwrap_or(NODATA);
            if !r.is_valid(v) {
                ok = false;
                break;
            }
            if let Some(slot) = vals.get_mut(bi) {
                *slot = v as f64;
            }
        }
        let v = if ok { e.eval(&vals) } else { f64::NAN };
        out.push(if v.is_finite() { v as f32 } else { NODATA });
    }
    Ok(out)
}

/// Remaps value ranges `[lo, hi)` to new values; other values become no-data unless `keep`.
pub fn remap(r: &Raster, b: usize, ranges: &[(f64, f64, f64)], keep: bool) -> Result<Vec<f32>, String> {
    let d = band(r, b)?;
    Ok(d.iter()
        .map(|v| {
            if !r.is_valid(*v) {
                return NODATA;
            }
            let x = *v as f64;
            match ranges.iter().find(|(lo, hi, _)| x >= *lo && x < *hi) {
                Some((_, _, new)) => *new as f32,
                None if keep => *v,
                None => NODATA,
            }
        })
        .collect())
}

/// `band`'s values resampled (nearest neighbour) onto `target`'s grid, through longitude/latitude.
pub fn resample_to(src: &Raster, b: usize, target: &Raster) -> Result<Vec<f32>, String> {
    src.require_coordinates()?;
    target.require_coordinates()?;
    band(src, b)?;
    if src.width == target.width && src.height == target.height && src.transform == target.transform && src.crs == target.crs {
        return Ok(band(src, b)?.iter().map(|v| if src.is_valid(*v) { *v } else { NODATA }).collect());
    }
    Ok(per_cell(target, |c, row| {
        let w = target.transform.to_world(c as f64 + 0.5, row as f64 + 0.5);
        let ll = target.transformer().to_lonlat(w)?;
        let (sc, sr) = src.pixel_at(ll)?;
        src.value(b, sc, sr).map(f64::from)
    }))
}

/// A weighted sum of criteria (each already scored, e.g. 1–10) on one grid; no-data anywhere
/// gives no-data. Weights are normalised to sum to 1. Inputs mark no-data with [`NODATA`] (as
/// `resample_to`, `remap` and the other functions here do).
pub fn weighted_overlay(criteria: &[(Vec<f32>, f64)]) -> Result<Vec<f32>, String> {
    let n = criteria.first().map(|c| c.0.len()).ok_or("give at least one criterion")?;
    if criteria.iter().any(|c| c.0.len() != n) {
        return Err("criteria must share a grid".into());
    }
    let total: f64 = criteria.iter().map(|c| c.1.max(0.0)).sum();
    if total <= 0.0 {
        return Err("the weights must add up to more than zero".into());
    }
    Ok((0..n)
        .map(|i| {
            let mut s = 0.0;
            for (vals, w) in criteria {
                let v = vals.get(i).copied().unwrap_or(NODATA);
                if v == NODATA || !v.is_finite() {
                    return NODATA;
                }
                s += v as f64 * w.max(0.0) / total;
            }
            s as f32
        })
        .collect())
}

/// Metres between two longitude/latitude points.
fn dist_m(a: Coord, b: Coord) -> f64 {
    rustgis_geom::haversine_m(a, b)
}

/// Elevation (bilinear) at a longitude/latitude point.
pub fn elevation_at(r: &Raster, b: usize, ll: Coord) -> Option<f64> {
    let w = r.transformer().from_lonlat(ll)?;
    let (x, y) = r.transform.to_pixel(w);
    let (x, y) = (x - 0.5, y - 0.5);
    let (c0, r0) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - c0 as f64, y - r0 as f64);
    let v = |c: i64, rr: i64| r.value(b, c, rr).map(f64::from);
    let (a, b2, c, d) = (v(c0, r0), v(c0 + 1, r0), v(c0, r0 + 1), v(c0 + 1, r0 + 1));
    match (a, b2, c, d) {
        (Some(a), Some(b2), Some(c), Some(d)) => {
            Some(a * (1.0 - fx) * (1.0 - fy) + b2 * fx * (1.0 - fy) + c * (1.0 - fx) * fy + d * fx * fy)
        }
        _ => v(x.round() as i64, y.round() as i64),
    }
}

/// An elevation profile along a path: (distance from the start in metres, point, elevation).
pub fn profile(r: &Raster, b: usize, path: &[Coord], samples: usize) -> Vec<(f64, Coord, Option<f64>)> {
    let samples = samples.clamp(2, 10_000);
    let legs: Vec<f64> = path
        .windows(2)
        .map(|w| match w {
            [a, c] => dist_m(*a, *c),
            _ => 0.0,
        })
        .collect();
    let total: f64 = legs.iter().sum();
    if total <= 0.0 {
        return path.first().map(|p| vec![(0.0, *p, elevation_at(r, b, *p))]).unwrap_or_default();
    }
    // Samples are in order along the path, so the legs are walked once.
    let mut out = Vec::with_capacity(samples);
    let (mut li, mut acc) = (0usize, 0.0);
    for i in 0..samples {
        let d = total * i as f64 / (samples - 1) as f64;
        while li + 1 < legs.len() && d > acc + legs.get(li).copied().unwrap_or(0.0) {
            acc += legs.get(li).copied().unwrap_or(0.0);
            li += 1;
        }
        let len = legs.get(li).copied().unwrap_or(0.0);
        let t = if len > 0.0 { ((d - acc) / len).clamp(0.0, 1.0) } else { 0.0 };
        let at = match (path.get(li), path.get(li + 1)) {
            (Some(a), Some(c)) => Coord::new(a.x + (c.x - a.x) * t, a.y + (c.y - a.y) * t),
            _ => path.last().copied().unwrap_or_default(),
        };
        out.push((d, at, elevation_at(r, b, at)));
    }
    out
}

/// Whether `b` is visible from `a` (observer and target heights above the ground, in metres),
/// and each profile sample with whether it can be seen. Earth curvature is included.
pub fn line_of_sight(
    r: &Raster,
    band: usize,
    a: Coord,
    b: Coord,
    ha: f64,
    hb: f64,
    samples: usize,
) -> Option<(bool, Vec<(f64, Coord, Option<f64>, bool)>)> {
    let prof = profile(r, band, &[a, b], samples);
    let (_, _, za) = *prof.first()?;
    let eye = za? + ha;
    const R: f64 = 6_371_000.0 * 4.0 / 3.0; // refraction-adjusted Earth radius
    let drop = |d: f64| d * d / (2.0 * R);
    let mut max_angle = f64::NEG_INFINITY;
    let mut out = Vec::with_capacity(prof.len());
    let mut target_visible = false;
    for (i, (d, p, zz)) in prof.iter().enumerate() {
        let last = i + 1 == prof.len();
        let h = zz.map(|z| z - drop(*d) + if last { hb } else { 0.0 });
        let visible = match h {
            Some(h) if *d > 0.0 => {
                let ang = (h - eye) / d;
                let v = ang >= max_angle;
                if !last || hb == 0.0 {
                    max_angle = max_angle.max(ang);
                }
                v
            }
            Some(_) => true,
            None => false,
        };
        if last {
            target_visible = visible;
        }
        out.push((*d, *p, *zz, visible));
    }
    Some((target_visible, out))
}

/// Most ray steps a viewshed takes, and cell-point pairs an interpolation weighs (each about a
/// second or two).
const MAX_VIEWSHED_WORK: i64 = 400_000_000;
pub(crate) const MAX_IDW_WORK: usize = 400_000_000;

/// Cells visible from an observer (1) or not (0) within `radius_m`; outside the radius no-data.
pub fn viewshed(r: &Raster, b: usize, observer: Coord, height: f64, radius_m: f64) -> Result<Vec<f32>, String> {
    let (oc, or) = r.pixel_at(observer).ok_or("the observer is outside the raster")?;
    let z0 = r.value(b, oc, or).ok_or("the observer stands on no-data")? as f64 + height;
    let (cx, cy) = r.cell_size_m();
    if !(cx > 0.0 && cy > 0.0 && radius_m.is_finite() && radius_m > 0.0) {
        return Err("the raster's cells or the radius have no size".into());
    }
    // A tiny declared cell size would make the window overflow: bound it before converting.
    if radius_m / cx > 100_000.0 || radius_m / cy > 100_000.0 {
        return Err("the radius covers too many cells; use a smaller radius".into());
    }
    let rad_c = (radius_m / cx).ceil() as i64;
    let rad_r = (radius_m / cy).ceil() as i64;
    // Each cell walks a ray back to the observer: bound cells × ray length, not just cells.
    let cells = (2 * rad_c + 1).saturating_mul(2 * rad_r + 1);
    if cells > 25_000_000 || cells.saturating_mul(rad_c.max(rad_r) / 2 + 1) > MAX_VIEWSHED_WORK {
        return Err("the radius covers too many cells; use a smaller radius".into());
    }
    let w = r.width as usize;
    let mut out = vec![NODATA; w * r.height as usize];
    const R: f64 = 6_371_000.0 * 4.0 / 3.0;
    for row in (or - rad_r).max(0)..=(or + rad_r).min(r.height as i64 - 1) {
        for col in (oc - rad_c).max(0)..=(oc + rad_c).min(r.width as i64 - 1) {
            let (dx, dy) = ((col - oc) as f64 * cx, (row - or) as f64 * cy);
            let dist = dx.hypot(dy);
            if dist > radius_m {
                continue;
            }
            let idx = row as usize * w + col as usize;
            let Some(zt) = r.value(b, col, row) else { continue };
            if dist == 0.0 {
                if let Some(o) = out.get_mut(idx) {
                    *o = 1.0;
                }
                continue;
            }
            let target = (zt as f64 - dist * dist / (2.0 * R) - z0) / dist;
            // Walk the ray from the observer; any cell higher than the line hides the target.
            let steps = ((col - oc).abs().max((row - or).abs())) as usize;
            let mut visible = true;
            for s in 1..steps {
                let t = s as f64 / steps as f64;
                let (c, rr) = ((oc as f64 + (col - oc) as f64 * t).round() as i64, (or as f64 + (row - or) as f64 * t).round() as i64);
                let Some(zi) = r.value(b, c, rr) else { continue };
                let di = dist * t;
                if (zi as f64 - di * di / (2.0 * R) - z0) / di > target {
                    visible = false;
                    break;
                }
            }
            if let Some(o) = out.get_mut(idx) {
                *o = if visible { 1.0 } else { 0.0 };
            }
        }
    }
    Ok(out)
}

/// Inverse distance weighted interpolation of point values onto a longitude/latitude grid, from
/// each cell's `neighbours` nearest points, or from every point (as QGIS does) when it is 0.
pub fn idw(points: &[(Coord, f64)], extent: &Extent, cell_deg: f64, power: f64, neighbours: usize, name: &str) -> Result<Raster, String> {
    if points.len() < 2 {
        return Err("interpolation needs at least two points with values".into());
    }
    if !(cell_deg.is_finite() && cell_deg > 0.0) {
        return Err("the cell size must be positive".into());
    }
    if extent.is_empty() {
        return Err("the area to interpolate over is empty".into());
    }
    let (w, h) = ((extent.width() / cell_deg).ceil().max(1.0) as usize, (extent.height() / cell_deg).ceil().max(1.0) as usize);
    if w.saturating_mul(h) > MAX_PIXELS / 4 {
        return Err(format!("{w} × {h} cells is too many; use a larger cell size"));
    }
    // Every cell weighs every point.
    if w.saturating_mul(h).saturating_mul(points.len()) > MAX_IDW_WORK {
        return Err(format!("{w} × {h} cells from {} points is too much work; use a larger cell size", points.len()));
    }
    if !(power.is_finite() && power >= 0.0) {
        return Err("the distance coefficient must be zero or more".into());
    }
    let k = if neighbours == 0 { points.len() } else { neighbours.clamp(1, points.len()) };
    let mut data = Vec::with_capacity(w * h);
    let mut near: Vec<(f64, f64)> = Vec::with_capacity(points.len());
    for row in 0..h {
        // East-west degrees shrink with latitude, which only changes by row.
        let kx = (extent.max_y - (row as f64 + 0.5) * cell_deg).to_radians().cos();
        for col in 0..w {
            let c = Coord::new(extent.min_x + (col as f64 + 0.5) * cell_deg, extent.max_y - (row as f64 + 0.5) * cell_deg);
            near.clear();
            near.extend(points.iter().map(|(p, v)| {
                let dx = (p.x - c.x) * kx;
                ((dx * dx + (p.y - c.y).powi(2)).sqrt(), *v)
            }));
            if k < near.len() {
                near.select_nth_unstable_by(k - 1, |a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                near.truncate(k);
            }
            let v = if let Some(exact) = near.iter().find(|(d, _)| *d < 1e-12) {
                exact.1
            } else {
                // Weights relative to the nearest point's: the same ratios as 1/dᵖ, without
                // overflowing for a high power.
                let nearest = near.iter().map(|(d, _)| *d).fold(f64::INFINITY, f64::min);
                let (num, den) = near.iter().fold((0.0, 0.0), |(n, d), (dist, v)| {
                    let wgt = (nearest / dist).powf(power);
                    (n + wgt * v, d + wgt)
                });
                num / den
            };
            data.push(if v.is_finite() { v as f32 } else { NODATA });
        }
    }
    let gt = GeoTransform { origin_x: extent.min_x, origin_y: extent.max_y, pixel_width: cell_deg, pixel_height: -cell_deg };
    Raster::new(name, w as u32, h as u32, vec![data], gt, Crs::LonLat, Some(NODATA as f64), ColorHint::Gray, "Float32")
        .map_err(|e| e.to_string())
}

/// Unsupervised k-means classes (1..=k) of the pixels over the given bands.
pub fn kmeans(r: &Raster, bands: &[usize], k: usize, iterations: usize) -> Result<Vec<f32>, String> {
    let k = k.clamp(2, 50);
    let data: Vec<&[f32]> = bands.iter().map(|b| band(r, *b)).collect::<Result<_, _>>()?;
    if data.is_empty() {
        return Err("give at least one band".into());
    }
    let n = r.width as usize * r.height as usize;
    let pixel =
        |i: usize| -> Option<Vec<f64>> { data.iter().map(|d| d.get(i).copied().filter(|v| r.is_valid(*v)).map(f64::from)).collect() };
    // The same, into a buffer reused across pixels (the full passes touch every pixel).
    let pixel_into = |i: usize, buf: &mut Vec<f64>| -> bool {
        buf.clear();
        for d in &data {
            match d.get(i).copied().filter(|v| r.is_valid(*v)) {
                Some(v) => buf.push(f64::from(v)),
                None => return false,
            }
        }
        true
    };
    // Seeds spread across the value range of the first band.
    let valid: Vec<usize> = (0..n).step_by((n / 20_000).max(1)).filter(|i| pixel(*i).is_some()).collect();
    if valid.len() < k {
        return Err("too few valid pixels to classify".into());
    }
    let mut sorted = valid.clone();
    sorted.sort_by(|a, b| {
        let (va, vb) =
            (pixel(*a).and_then(|p| p.first().copied()).unwrap_or(0.0), pixel(*b).and_then(|p| p.first().copied()).unwrap_or(0.0));
        va.partial_cmp(&vb).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut centres: Vec<Vec<f64>> =
        (0..k).filter_map(|i| sorted.get((i * 2 + 1) * sorted.len() / (2 * k)).and_then(|j| pixel(*j))).collect();
    let nearest = |p: &[f64], cs: &[Vec<f64>]| -> usize {
        cs.iter()
            .enumerate()
            .map(|(i, c)| (i, c.iter().zip(p).map(|(a, b)| (a - b).powi(2)).sum::<f64>()))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map_or(0, |x| x.0)
    };
    for _ in 0..iterations.clamp(1, 50) {
        let mut sums = vec![vec![0.0; data.len()]; centres.len()];
        let mut counts = vec![0usize; centres.len()];
        let mut p = Vec::with_capacity(data.len());
        for i in &valid {
            if pixel_into(*i, &mut p) {
                let c = nearest(&p, &centres);
                if let (Some(s), Some(n)) = (sums.get_mut(c), counts.get_mut(c)) {
                    s.iter_mut().zip(&p).for_each(|(a, b)| *a += b);
                    *n += 1;
                }
            }
        }
        for ((c, s), n) in centres.iter_mut().zip(&sums).zip(&counts) {
            if *n > 0 {
                *c = s.iter().map(|v| v / *n as f64).collect();
            }
        }
    }
    // Classes numbered by the first band's centre, low to high.
    let mut order: Vec<usize> = (0..centres.len()).collect();
    order.sort_by(|a, b| {
        centres.get(*a).and_then(|c| c.first()).partial_cmp(&centres.get(*b).and_then(|c| c.first())).unwrap_or(std::cmp::Ordering::Equal)
    });
    let rank: Vec<usize> = (0..centres.len()).map(|c| order.iter().position(|o| *o == c).unwrap_or(0)).collect();
    let mut p = Vec::with_capacity(data.len());
    Ok((0..n)
        .map(|i| if pixel_into(i, &mut p) { (rank.get(nearest(&p, &centres)).copied().unwrap_or(0) + 1) as f32 } else { NODATA })
        .collect())
}

/// Minimum-distance classification: each pixel takes the class whose training mean is closest.
pub fn classify_min_distance(r: &Raster, bands: &[usize], means: &[(f32, Vec<f64>)]) -> Result<Vec<f32>, String> {
    let data: Vec<&[f32]> = bands.iter().map(|b| band(r, *b)).collect::<Result<_, _>>()?;
    if means.is_empty() {
        return Err("no training classes".into());
    }
    let n = r.width as usize * r.height as usize;
    let mut p: Vec<f64> = Vec::with_capacity(data.len());
    Ok((0..n)
        .map(|i| {
            p.clear();
            for d in &data {
                match d.get(i).copied().filter(|v| r.is_valid(*v)) {
                    Some(v) => p.push(f64::from(v)),
                    None => return NODATA,
                }
            }
            means
                .iter()
                .map(|(c, m)| (*c, m.iter().zip(&p).map(|(a, b)| (a - b).powi(2)).sum::<f64>()))
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map_or(NODATA, |x| x.0)
        })
        .collect())
}

/// The mean of each band over the pixels whose centres fall in a polygon (lon/lat rings).
pub fn zonal_means(r: &Raster, bands: &[usize], rings: &[Vec<Coord>]) -> Option<(Vec<f64>, usize)> {
    let mut e = Extent::EMPTY;
    for ring in rings {
        for c in ring {
            e.expand(*c);
        }
    }
    let mut sums = vec![0.0; bands.len()];
    let mut n = 0usize;
    for row in 0..r.height as i64 {
        for col in 0..r.width as i64 {
            let w = r.transform.to_world(col as f64 + 0.5, row as f64 + 0.5);
            let Some(ll) = r.transformer().to_lonlat(w) else { continue };
            if !e.contains(ll) || !rustgis_data::geometry::polygon_contains(rings, ll) {
                continue;
            }
            let vals: Option<Vec<f64>> = bands.iter().map(|b| r.value(*b, col, row).map(f64::from)).collect();
            if let Some(v) = vals {
                sums.iter_mut().zip(&v).for_each(|(s, x)| *s += x);
                n += 1;
            }
        }
    }
    (n > 0).then(|| (sums.into_iter().map(|s| s / n as f64).collect(), n))
}

/// The cells of `r` whose centres fall inside any of `polygons` (each an exterior ring and its
/// holes, in longitude/latitude): the column and row window around the polygons, clamped to the
/// raster, and a row-major mask over it. `None` when no cell centre falls inside.
///
/// The rings are projected onto the raster's own grid once and filled by scanline, so a large
/// raster costs a pass over the polygon edges per row rather than a point test per cell.
pub fn polygon_mask(r: &Raster, polygons: &[Vec<Vec<Coord>>]) -> Option<(Range<u32>, Range<u32>, Vec<bool>)> {
    let tr = r.transformer();
    let projected: Vec<Vec<Vec<(f64, f64)>>> = polygons
        .iter()
        .map(|rings| {
            rings
                .iter()
                .map(|ring| ring.iter().filter_map(|c| tr.from_lonlat(*c)).map(|w| r.transform.to_pixel(w)).collect::<Vec<_>>())
                .filter(|ring| ring.len() >= 3)
                .collect::<Vec<_>>()
        })
        .filter(|rings| !rings.is_empty())
        .collect();
    let (mut c0, mut r0, mut c1, mut r1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for &(x, y) in projected.iter().flatten().flatten() {
        (c0, r0, c1, r1) = (c0.min(x), r0.min(y), c1.max(x), r1.max(y));
    }
    if !(c0.is_finite() && r0.is_finite() && c1.is_finite() && r1.is_finite()) {
        return None;
    }
    let clamp = |v: f64, n: u32| v.clamp(0.0, f64::from(n)) as u32;
    let cols = clamp(c0.floor(), r.width)..clamp(c1.ceil(), r.width);
    let rows = clamp(r0.floor(), r.height)..clamp(r1.ceil(), r.height);
    if cols.is_empty() || rows.is_empty() {
        return None;
    }
    let w = (cols.end - cols.start) as usize;
    let mut mask = vec![false; w * (rows.end - rows.start) as usize];
    let mut xs: Vec<f64> = Vec::new();
    for (ri, row) in rows.clone().enumerate() {
        let y = f64::from(row) + 0.5;
        let line = mask.get_mut(ri * w..(ri + 1) * w).unwrap_or_default();
        // Even-odd within each polygon (holes cut out), any polygon over the others.
        for rings in &projected {
            xs.clear();
            for ring in rings {
                for (i, &(ax, ay)) in ring.iter().enumerate() {
                    let (bx, by) = ring.get(i + 1).or(ring.first()).copied().unwrap_or((ax, ay));
                    if (ay <= y) != (by <= y) {
                        xs.push(ax + (y - ay) * (bx - ax) / (by - ay));
                    }
                }
            }
            xs.sort_by(f64::total_cmp);
            for &[x0, x1] in xs.as_chunks::<2>().0 {
                // Cells whose centre (col + 0.5) lies in [x0, x1).
                let a = clamp((x0 - 0.5).ceil(), r.width).max(cols.start);
                let b = clamp((x1 - 0.5).ceil(), r.width).min(cols.end);
                for col in a..b {
                    if let Some(m) = line.get_mut((col - cols.start) as usize) {
                        *m = true;
                    }
                }
            }
        }
    }
    // Trimmed to the cells inside: reprojected vertices on cell edges land a hair either side.
    let (mut tc0, mut tr0, mut tc1, mut tr1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, _) in mask.iter().enumerate().filter(|(_, m)| **m) {
        let (c, rr) = (i % w, i / w);
        (tc0, tr0, tc1, tr1) = (tc0.min(c), tr0.min(rr), tc1.max(c + 1), tr1.max(rr + 1));
    }
    if tc0 >= tc1 || tr0 >= tr1 {
        return None;
    }
    let trimmed: Vec<bool> =
        (tr0..tr1).flat_map(|rr| (tc0..tc1).map(move |c| (rr, c))).map(|(rr, c)| mask.get(rr * w + c).copied().unwrap_or(false)).collect();
    let cols = cols.start + tc0 as u32..cols.start + tc1 as u32;
    let rows = rows.start + tr0 as u32..rows.start + tr1 as u32;
    Some((cols, rows, trimmed))
}

/// A tiny arithmetic expression over band values.
enum Expr {
    Num(f64),
    Band(usize),
    Neg(Box<Expr>),
    Bin(Box<Expr>, char, Box<Expr>),
}

impl Expr {
    fn parse(s: &str, bands: usize) -> Result<Expr, String> {
        let toks: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
        if toks.len() > 2000 {
            return Err("the expression is too long".into());
        }
        let mut pos = 0;
        let e = Self::sum(&toks, &mut pos, bands, 0)?;
        if pos != toks.len() {
            return Err(format!("unexpected '{}' in the expression", toks.get(pos).copied().unwrap_or(' ')));
        }
        Ok(e)
    }
    fn sum(t: &[char], p: &mut usize, n: usize, depth: usize) -> Result<Expr, String> {
        let mut e = Self::product(t, p, n, depth)?;
        while let Some(op) = t.get(*p).copied().filter(|c| matches!(c, '+' | '-')) {
            *p += 1;
            e = Expr::Bin(Box::new(e), op, Box::new(Self::product(t, p, n, depth)?));
        }
        Ok(e)
    }
    fn product(t: &[char], p: &mut usize, n: usize, depth: usize) -> Result<Expr, String> {
        let mut e = Self::atom(t, p, n, depth)?;
        while let Some(op) = t.get(*p).copied().filter(|c| matches!(c, '*' | '/')) {
            *p += 1;
            e = Expr::Bin(Box::new(e), op, Box::new(Self::atom(t, p, n, depth)?));
        }
        Ok(e)
    }
    fn atom(t: &[char], p: &mut usize, n: usize, depth: usize) -> Result<Expr, String> {
        if depth > 50 {
            return Err("the expression nests too deeply".into());
        }
        match t.get(*p).copied() {
            Some('(') => {
                *p += 1;
                let e = Self::sum(t, p, n, depth + 1)?;
                if t.get(*p) != Some(&')') {
                    return Err("a parenthesis isn't closed".into());
                }
                *p += 1;
                Ok(e)
            }
            Some('-') => {
                *p += 1;
                Ok(Expr::Neg(Box::new(Self::atom(t, p, n, depth + 1)?)))
            }
            Some('b' | 'B') => {
                *p += 1;
                let start = *p;
                while t.get(*p).is_some_and(char::is_ascii_digit) {
                    *p += 1;
                }
                let num: String = t.get(start..*p).unwrap_or(&[]).iter().collect();
                let i: usize = num.parse().map_err(|_| "bands are written b1, b2…".to_string())?;
                if i == 0 || i > n {
                    return Err(format!("the raster has no band {i}"));
                }
                Ok(Expr::Band(i - 1))
            }
            Some(c) if c.is_ascii_digit() || c == '.' => {
                let start = *p;
                while t.get(*p).is_some_and(|c| c.is_ascii_digit() || *c == '.') {
                    *p += 1;
                }
                let num: String = t.get(start..*p).unwrap_or(&[]).iter().collect();
                num.parse().map(Expr::Num).map_err(|_| format!("'{num}' isn't a number"))
            }
            other => Err(format!("unexpected '{}' in the expression", other.unwrap_or(' '))),
        }
    }
    fn eval(&self, v: &[f64]) -> f64 {
        match self {
            Expr::Num(x) => *x,
            Expr::Band(i) => v.get(*i).copied().unwrap_or(f64::NAN),
            Expr::Neg(e) => -e.eval(v),
            Expr::Bin(a, op, b) => {
                let (a, b) = (a.eval(v), b.eval(v));
                match op {
                    '+' => a + b,
                    '-' => a - b,
                    '*' => a * b,
                    _ if b != 0.0 => a / b,
                    _ => f64::NAN,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 20×20 lon/lat raster: a ramp rising 10 m per cell to the east, and a second band.
    fn ramp() -> Raster {
        let (w, h) = (20u32, 20u32);
        let a: Vec<f32> = (0..w * h).map(|i| (i % w) as f32 * 10.0).collect();
        let b: Vec<f32> = (0..w * h).map(|i| (i / w) as f32).collect();
        let gt = GeoTransform { origin_x: 0.0, origin_y: 0.02, pixel_width: 0.001, pixel_height: -0.001 };
        Raster::new("ramp", w, h, vec![a, b], gt, Crs::LonLat, None, ColorHint::Gray, "Float32").unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn polygon_mask_fills_polygons_and_cuts_holes() {
        let r = ramp();
        let sq = |x0: f64, y0: f64, x1: f64, y1: f64| {
            vec![Coord::new(x0, y0), Coord::new(x1, y0), Coord::new(x1, y1), Coord::new(x0, y1), Coord::new(x0, y0)]
        };
        // A square with a hole, and a second polygon in the south-west corner.
        let polys = vec![vec![sq(0.005, 0.005, 0.015, 0.015), sq(0.009, 0.009, 0.011, 0.011)], vec![sq(0.0, 0.0, 0.002, 0.002)]];
        let (cols, rows, mask) = polygon_mask(&r, &polys).unwrap_or_else(|| panic!("no mask"));
        assert_eq!((cols.clone(), rows.clone()), (0..15, 5..20));
        assert_eq!(mask.iter().filter(|m| **m).count(), 100 - 4 + 4);
        let at = |col: u32, row: u32| mask[((row - rows.start) * 15 + col - cols.start) as usize];
        // Rows count down from the north edge (0.02): row 5 is the square's northern row.
        assert!(at(5, 5) && at(14, 14) && !at(4, 5) && !at(14, 15));
        assert!(!at(9, 9) && !at(10, 10), "the hole stays out");
        assert!(at(0, 19) && at(1, 18) && !at(2, 18));
        // Outside the raster, or too small to hold a cell centre: nothing.
        assert!(polygon_mask(&r, &[vec![sq(1.0, 1.0, 2.0, 2.0)]]).is_none());
        assert!(polygon_mask(&r, &[vec![sq(0.0051, 0.0051, 0.0052, 0.0052)]]).is_none());
    }

    #[test]
    fn terrain_math_and_classes() {
        let r = ramp();
        let s = slope(&r, 0, 1.0).unwrap_or_default();
        // 10 m rise per ~111 m: about 5.1 degrees.
        let mid = s[10 * 20 + 10];
        assert!((mid - 5.1).abs() < 0.3, "{mid}");
        let a = aspect(&r, 0).unwrap_or_default();
        assert!((a[10 * 20 + 10] - 270.0).abs() < 1.0, "facing west: {}", a[210]);
        let hs = hillshade(&r, 0, 270.0, 45.0, 1.0).unwrap_or_default();
        let hs_e = hillshade(&r, 0, 90.0, 45.0, 1.0).unwrap_or_default();
        assert!(hs[210] > hs_e[210], "lit from the west the west-facing slope is brighter");
        let m = band_math(&r, "(b1 - b2) / (b1 + b2 + 1) * 2").unwrap_or_default();
        assert!((m[20 + 3] - ((30.0 - 1.0) / 32.0 * 2.0)).abs() < 1e-5);
        assert!(band_math(&r, "b3 + 1").is_err() && band_math(&r, "(b1").is_err() && band_math(&r, "b1 $ 2").is_err());
        let rm = remap(&r, 0, &[(0.0, 100.0, 1.0), (100.0, 1000.0, 2.0)], false).unwrap_or_default();
        assert_eq!((rm[5], rm[15]), (1.0, 2.0));
        let o = weighted_overlay(&[(rm.clone(), 3.0), (vec![1.0; 400], 1.0)]).unwrap_or_default();
        assert!((o[15] - 1.75).abs() < 1e-6);
        let k = kmeans(&r, &[0], 2, 10).unwrap_or_default();
        assert_eq!((k[0], k[19]), (1.0, 2.0));
        // A point east of the ridge can't see over a wall.
        let mut wall = ramp();
        if let Some(b) = wall.bands.get_mut(0) {
            for (i, v) in b.data.iter_mut().enumerate() {
                *v = if i % 20 == 10 { 500.0 } else { 0.0 };
            }
        }
        let vs = viewshed(&wall, 0, Coord::new(0.0025, 0.0105), 2.0, 5_000.0).unwrap_or_default();
        assert_eq!(vs[10 * 20 + 5], 1.0);
        assert_eq!(vs[10 * 20 + 15], 0.0);
        let (seen, prof) =
            line_of_sight(&wall, 0, Coord::new(0.0025, 0.0105), Coord::new(0.0175, 0.0105), 2.0, 0.0, 50).unwrap_or((true, vec![]));
        assert!(!seen && !prof.is_empty());
        let g = idw(&[(Coord::new(0.0, 0.0), 0.0), (Coord::new(1.0, 0.0), 10.0)], &Extent::new(0.0, -0.5, 1.0, 0.5), 0.1, 2.0, 8, "idw")
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((g.width, g.height), (10, 10));
        let row = &g.bands[0].data[50..60];
        assert!(row[0] < row[9] && row[0] > 0.0 && row[9] < 10.0, "{row:?}");
        // Every point (0), and a power high enough to overflow 1/dᵖ, still give finite values.
        let all = idw(&[(Coord::new(0.0, 0.0), 0.0), (Coord::new(1.0, 0.0), 10.0)], &Extent::new(0.0, -0.5, 1.0, 0.5), 0.1, 99.0, 0, "idw")
            .unwrap_or_else(|e| panic!("{e}"));
        let row = &all.bands[0].data[50..60];
        assert!(row.iter().all(|v| (0.0..=10.0).contains(v)) && row[0] < 1e-3 && row[9] > 10.0 - 1e-3, "{row:?}");
        // With every point, a far one still pulls each cell off the two near ones' value.
        let pts = [(Coord::new(0.0, 0.0), 0.0), (Coord::new(1.0, 0.0), 0.0), (Coord::new(5.0, 0.0), 100.0)];
        let all = idw(&pts, &Extent::new(0.0, -0.5, 1.0, 0.5), 0.1, 1.0, 0, "idw").unwrap_or_else(|e| panic!("{e}"));
        assert!(all.bands[0].data.iter().all(|v| *v > 0.0 && *v < 100.0), "{:?}", all.bands[0].data);
        let p = profile(&r, 0, &[Coord::new(0.0005, 0.01), Coord::new(0.0195, 0.01)], 20);
        assert_eq!(p.len(), 20);
        assert!(p.last().and_then(|x| x.2).unwrap_or(0.0) > p[0].2.unwrap_or(0.0) + 150.0);
    }
}
