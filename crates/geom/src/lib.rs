//! L0: coordinates, extents, the Web Mercator projection, geodesic measurement and map scale.
//!
//! Everything here is plain math with no I/O. Longitude/latitude are WGS 84 degrees; projected
//! coordinates are Web Mercator metres (EPSG:3857).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod body;
pub mod sinusoidal;
pub mod triangulate;

pub use body::{BODIES, Body};
pub use sinusoidal::Sinusoidal;

use serde::{Deserialize, Serialize};

/// Mean Earth radius used by the haversine formula (metres).
pub const EARTH_RADIUS_M: f64 = 6_371_008.8;
/// Web Mercator sphere radius (metres).
pub const MERCATOR_R: f64 = 6_378_137.0;
/// Latitude limit of Web Mercator.
pub const MAX_LAT: f64 = 85.051_128_78;
/// Screen pixel size assumed when converting resolution to a map scale (0.28 mm, OGC).
pub const PIXEL_M: f64 = 0.000_28;

/// A 2-D coordinate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Coord {
    pub x: f64,
    pub y: f64,
}

impl Coord {
    pub const fn new(x: f64, y: f64) -> Self {
        Coord { x, y }
    }
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// An axis-aligned rectangle. An "empty" extent has `min > max`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Extent {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl Default for Extent {
    fn default() -> Self {
        Extent::EMPTY
    }
}

impl Extent {
    pub const EMPTY: Extent = Extent { min_x: f64::INFINITY, min_y: f64::INFINITY, max_x: f64::NEG_INFINITY, max_y: f64::NEG_INFINITY };
    /// The whole world in longitude/latitude.
    pub const WORLD_LONLAT: Extent = Extent { min_x: -180.0, min_y: -85.0, max_x: 180.0, max_y: 85.0 };

    pub const fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Self {
        Extent { min_x, min_y, max_x, max_y }
    }
    pub fn from_points(a: Coord, b: Coord) -> Self {
        Extent { min_x: a.x.min(b.x), min_y: a.y.min(b.y), max_x: a.x.max(b.x), max_y: a.y.max(b.y) }
    }
    pub fn is_empty(&self) -> bool {
        !(self.min_x <= self.max_x && self.min_y <= self.max_y)
    }
    pub fn width(&self) -> f64 {
        if self.is_empty() { 0.0 } else { self.max_x - self.min_x }
    }
    pub fn height(&self) -> f64 {
        if self.is_empty() { 0.0 } else { self.max_y - self.min_y }
    }
    pub fn center(&self) -> Coord {
        Coord::new((self.min_x + self.max_x) / 2.0, (self.min_y + self.max_y) / 2.0)
    }
    pub fn expand(&mut self, c: Coord) {
        if !c.is_finite() {
            return;
        }
        self.min_x = self.min_x.min(c.x);
        self.min_y = self.min_y.min(c.y);
        self.max_x = self.max_x.max(c.x);
        self.max_y = self.max_y.max(c.y);
    }
    pub fn union(&self, o: &Extent) -> Extent {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        Extent::new(self.min_x.min(o.min_x), self.min_y.min(o.min_y), self.max_x.max(o.max_x), self.max_y.max(o.max_y))
    }
    pub fn intersects(&self, o: &Extent) -> bool {
        !self.is_empty()
            && !o.is_empty()
            && self.min_x <= o.max_x
            && o.min_x <= self.max_x
            && self.min_y <= o.max_y
            && o.min_y <= self.max_y
    }
    pub fn contains(&self, c: Coord) -> bool {
        c.x >= self.min_x && c.x <= self.max_x && c.y >= self.min_y && c.y <= self.max_y
    }
    /// Grows every side by `f` of the size (0.1 = 10 %), with a minimum pad for points.
    pub fn padded(&self, f: f64, min_pad: f64) -> Extent {
        if self.is_empty() {
            return *self;
        }
        let px = (self.width() * f).max(min_pad);
        let py = (self.height() * f).max(min_pad);
        Extent::new(self.min_x - px, self.min_y - py, self.max_x + px, self.max_y + py)
    }
}

/// Longitude/latitude (degrees) → Web Mercator metres. Latitude is clamped to ±85.05°.
pub fn lonlat_to_merc(c: Coord) -> Coord {
    let lat = c.y.clamp(-MAX_LAT, MAX_LAT).to_radians();
    Coord::new(MERCATOR_R * c.x.to_radians(), MERCATOR_R * (std::f64::consts::FRAC_PI_4 + lat / 2.0).tan().ln())
}

/// Web Mercator metres → longitude/latitude (degrees).
pub fn merc_to_lonlat(c: Coord) -> Coord {
    let lon = (c.x / MERCATOR_R).to_degrees();
    let lat = (2.0 * (c.y / MERCATOR_R).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees();
    Coord::new(lon, lat)
}

/// Projects a lon/lat extent to Web Mercator.
pub fn extent_to_merc(e: &Extent) -> Extent {
    if e.is_empty() {
        return *e;
    }
    Extent::from_points(lonlat_to_merc(Coord::new(e.min_x, e.min_y)), lonlat_to_merc(Coord::new(e.max_x, e.max_y)))
}

/// Great-circle distance between two lon/lat points (metres).
pub fn haversine_m(a: Coord, b: Coord) -> f64 {
    let (la1, la2) = (a.y.to_radians(), b.y.to_radians());
    let dlat = la2 - la1;
    let dlon = (b.x - a.x).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * h.sqrt().min(1.0).asin()
}

/// Length of a lon/lat polyline along great circles (metres).
pub fn geodesic_length_m(pts: &[Coord]) -> f64 {
    pts.windows(2)
        .map(|w| match w {
            [a, b] => haversine_m(*a, *b),
            _ => 0.0,
        })
        .sum()
}

/// A lon/lat point as a unit vector from the Earth's centre.
fn unit_vector(c: Coord) -> [f64; 3] {
    let (lon, lat) = (c.x.to_radians(), c.y.to_radians());
    [lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()]
}

/// A vector from the Earth's centre as lon/lat (degrees); `None` for the zero vector.
fn from_vector(v: [f64; 3]) -> Option<Coord> {
    let [x, y, z] = v;
    let h = x.hypot(y);
    (h.hypot(z) > 1e-12).then(|| Coord::new(y.atan2(x).to_degrees(), z.atan2(h).to_degrees()))
}

/// The point a fraction `t` of the way from `a` to `b` along the great circle between them.
pub fn geodesic_interpolate(a: Coord, b: Coord, t: f64) -> Coord {
    let (va, vb) = (unit_vector(a), unit_vector(b));
    let dot = (va[0] * vb[0] + va[1] * vb[1] + va[2] * vb[2]).clamp(-1.0, 1.0);
    let w = dot.acos();
    if w < 1e-12 || w.sin().abs() < 1e-12 {
        // The same point, or antipodes (no single great circle): straight in lon/lat.
        return Coord::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
    }
    let (ka, kb) = (((1.0 - t) * w).sin() / w.sin(), (t * w).sin() / w.sin());
    let v = [ka * va[0] + kb * vb[0], ka * va[1] + kb * vb[1], ka * va[2] + kb * vb[2]];
    match from_vector(v) {
        // Keep the longitude next to `a`'s, so a path across the antimeridian stays continuous.
        Some(c) => Coord::new(a.x + wrap_lon(c.x - a.x), c.y),
        None => a,
    }
}

/// A longitude difference (degrees) folded into −180..180.
fn wrap_lon(d: f64) -> f64 {
    let w = (d + 180.0).rem_euclid(360.0) - 180.0;
    if w.is_finite() { w } else { 0.0 }
}

/// The centre of lon/lat points on the sphere (their mean direction from the Earth's centre),
/// or `None` when there are none or they balance out (such as two antipodes).
pub fn spherical_centre(pts: &[Coord]) -> Option<Coord> {
    let mut s = [0.0; 3];
    for v in pts.iter().filter(|c| c.is_finite()).map(|c| unit_vector(*c)) {
        s = [s[0] + v[0], s[1] + v[1], s[2] + v[2]];
    }
    from_vector(s)
}

/// The point `dist_m` metres from `c` along the great circle leaving it at `azimuth_deg`
/// (clockwise from north): the direct geodesic problem on the sphere.
pub fn geodesic_destination(c: Coord, azimuth_deg: f64, dist_m: f64) -> Coord {
    let (s, co) = azimuth_deg.to_radians().sin_cos();
    Aeqd::new(c).inverse(Coord::new(dist_m * s, dist_m * co))
}

/// The azimuthal equidistant projection on the sphere, centred on a lon/lat point: distances
/// and directions from the centre are true, so a buffer drawn in it is geodesic near the centre.
/// Coordinates are metres east and north of the centre.
#[derive(Clone, Copy, Debug)]
pub struct Aeqd {
    lon0: f64,
    sin0: f64,
    cos0: f64,
}

impl Aeqd {
    pub fn new(centre: Coord) -> Self {
        let lat0 = centre.y.clamp(-90.0, 90.0).to_radians();
        Aeqd { lon0: centre.x, sin0: lat0.sin(), cos0: lat0.cos() }
    }

    /// Lon/lat (degrees) → metres from the centre.
    pub fn forward(&self, c: Coord) -> Coord {
        let lat = c.y.clamp(-90.0, 90.0).to_radians();
        let dlon = (c.x - self.lon0).to_radians();
        let (sl, cl) = lat.sin_cos();
        let east = cl * dlon.sin();
        let north = self.cos0 * sl - self.sin0 * cl * dlon.cos();
        let cos_c = self.sin0 * sl + self.cos0 * cl * dlon.cos();
        let sin_c = east.hypot(north);
        if sin_c < 1e-15 {
            return Coord::new(0.0, 0.0);
        }
        let k = EARTH_RADIUS_M * sin_c.atan2(cos_c) / sin_c;
        Coord::new(k * east, k * north)
    }

    /// Metres from the centre → lon/lat (degrees). The longitude is within 180° of the centre's,
    /// not folded into −180..180, so a shape across the antimeridian stays in one piece.
    pub fn inverse(&self, p: Coord) -> Coord {
        let rho = p.x.hypot(p.y);
        let c = rho / EARTH_RADIUS_M;
        let (sc, cc) = c.sin_cos();
        let (sa, ca) = if rho > 0.0 { (p.x / rho, p.y / rho) } else { (0.0, 1.0) };
        let lat = (self.sin0 * cc + self.cos0 * sc * ca).clamp(-1.0, 1.0).asin();
        // At a pole every direction is south (or north): the azimuth is the meridian.
        let dlon = if self.cos0 < 1e-9 {
            if self.sin0 > 0.0 { p.x.atan2(-p.y) } else { p.x.atan2(p.y) }
        } else {
            (sa * sc * self.cos0).atan2(cc - self.sin0 * lat.sin())
        };
        Coord::new(self.lon0 + dlon.to_degrees(), lat.to_degrees())
    }
}

/// Area of a lon/lat ring on the sphere (square metres, always positive).
///
/// Uses the spherical-excess approximation from Chamberlain & Duquette (JPL, 2007).
pub fn geodesic_ring_area_m2(ring: &[Coord]) -> f64 {
    let n = ring.len();
    if n < 3 {
        return 0.0;
    }
    let mut total = 0.0;
    for i in 0..n {
        let (Some(p1), Some(p2)) = (ring.get(i), ring.get((i + 1) % n)) else { continue };
        total += (p2.x - p1.x).to_radians() * (2.0 + p1.y.to_radians().sin() + p2.y.to_radians().sin());
    }
    (total * EARTH_RADIUS_M * EARTH_RADIUS_M / 2.0).abs()
}

/// Map scale denominator (1:N) for a Web Mercator resolution (metres per screen pixel) at a latitude.
pub fn scale_denominator(merc_m_per_px: f64, lat_deg: f64) -> f64 {
    merc_m_per_px * lat_deg.to_radians().cos().abs() / PIXEL_M
}

/// Rounds a distance down to a "nice" 1, 2 or 5 × 10ⁿ for scale bars.
pub fn nice_distance(d: f64) -> f64 {
    if d <= 0.0 || !d.is_finite() {
        return 0.0;
    }
    let p = 10f64.powf(d.log10().floor());
    let f = d / p;
    let n = if f >= 5.0 {
        5.0
    } else if f >= 2.0 {
        2.0
    } else {
        1.0
    };
    n * p
}

/// Formats a distance in metres as `850 m` / `12.5 km`.
pub fn format_distance(m: f64) -> String {
    if m.round() < 1000.0 {
        format!("{m:.0} m")
    } else if (m / 100.0).round() < 1000.0 {
        format!("{:.1} km", m / 1000.0)
    } else {
        format!("{:.0} km", m / 1000.0)
    }
}

/// Formats an area in square metres as `km²` or `m²`.
pub fn format_area(m2: f64) -> String {
    if m2.round() < 1.0e6 { format!("{m2:.0} m²") } else { format!("{:.1} km²", m2 / 1.0e6) }
}

/// Formats a lon/lat pair in decimal degrees with hemispheres: `12.345° E 45.678° N`.
pub fn format_lonlat(c: Coord) -> String {
    let ew = if c.x < 0.0 { 'W' } else { 'E' };
    let ns = if c.y < 0.0 { 'S' } else { 'N' };
    format!("{:.4}° {ew}  {:.4}° {ns}", c.x.abs(), c.y.abs())
}

/// Formats a lon/lat pair as degrees-minutes-seconds.
pub fn format_dms(c: Coord) -> String {
    fn dms(v: f64) -> String {
        let tenths = (v.abs() * 36_000.0).round();
        let d = (tenths / 36_000.0).floor();
        let m = ((tenths % 36_000.0) / 600.0).floor();
        let s = (tenths % 600.0) / 10.0;
        format!("{d:.0}°{m:02.0}′{s:04.1}″")
    }
    let ew = if c.x < 0.0 { 'W' } else { 'E' };
    let ns = if c.y < 0.0 { 'S' } else { 'N' };
    format!("{} {ew}  {} {ns}", dms(c.x), dms(c.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mercator_roundtrip() {
        for (x, y) in [(0.0, 0.0), (12.5, 41.9), (-122.4, 37.8), (179.0, -60.0)] {
            let m = lonlat_to_merc(Coord::new(x, y));
            let back = merc_to_lonlat(m);
            assert!((back.x - x).abs() < 1e-9 && (back.y - y).abs() < 1e-9);
        }
    }

    #[test]
    fn azimuthal_equidistant_keeps_distances_from_its_centre() {
        let centre = Coord::new(10.0, 60.0);
        let p = Aeqd::new(centre);
        for (x, y) in [(10.0, 70.0), (-170.0, 80.0), (40.0, 20.0), (10.0, 60.0), (179.5, -30.0)] {
            let c = Coord::new(x, y);
            let m = p.forward(c);
            assert!((m.x.hypot(m.y) - haversine_m(centre, c)).abs() < 1e-3, "{c:?}");
            let back = p.inverse(m);
            assert!((back.x - x).abs() < 1e-9 && (back.y - y).abs() < 1e-9, "{c:?} → {back:?}");
        }
        // The inverse stays next to the centre's longitude rather than folding into −180..180.
        assert!((Aeqd::new(Coord::new(179.0, 0.0)).inverse(Coord::new(300_000.0, 0.0)).x - 181.698).abs() < 1e-3);
    }

    #[test]
    fn geodesic_destination_and_interpolation() {
        let start = Coord::new(10.0, 60.0);
        for az in [0.0, 45.0, 90.0, 180.0, 300.0] {
            let d = geodesic_destination(start, az, 1_000_000.0);
            assert!((haversine_m(start, d) - 1_000_000.0).abs() < 1e-3, "{az}: {d:?}");
        }
        let north = geodesic_destination(start, 0.0, 1_000_000.0);
        assert!((north.x - 10.0).abs() < 1e-9 && north.y > 68.9, "{north:?}");
        let (a, b) = (Coord::new(2.3522, 48.8566), Coord::new(-0.1276, 51.5072));
        let mid = geodesic_interpolate(a, b, 0.5);
        assert!((haversine_m(a, mid) - haversine_m(mid, b)).abs() < 1e-3);
        // Across the antimeridian, the path stays continuous.
        assert!((geodesic_interpolate(Coord::new(179.0, 0.0), Coord::new(-179.0, 0.0), 0.5).x - 180.0).abs() < 1e-9);
        assert_eq!(spherical_centre(&[Coord::new(179.0, 0.0), Coord::new(-179.0, 0.0)]).map(|c| c.x.abs().round()), Some(180.0));
        assert!(spherical_centre(&[]).is_none());
    }

    #[test]
    fn haversine_paris_london() {
        let d = haversine_m(Coord::new(2.3522, 48.8566), Coord::new(-0.1276, 51.5072));
        assert!((d - 343_500.0).abs() < 2_000.0, "{d}");
    }

    #[test]
    fn distances_and_areas_switch_unit_when_they_round_up() {
        assert_eq!(format_distance(850.0), "850 m");
        assert_eq!(format_distance(999.4), "999 m");
        assert_eq!(format_distance(999.6), "1.0 km");
        assert_eq!(format_distance(12_500.0), "12.5 km");
        assert_eq!(format_distance(99_960.0), "100 km");
        assert_eq!(format_distance(99_949.0), "99.9 km");
        assert_eq!(format_distance(1_000.0), "1.0 km");
        assert_eq!(format_area(999_999.4), "999999 m²");
        assert_eq!(format_area(999_999.6), "1.0 km²");
        assert_eq!(format_area(1.0e6), "1.0 km²");
    }

    #[test]
    fn destination_lies_at_its_distance_and_bearing() {
        let from = Coord::new(10.0, 60.0);
        for az in [0.0, 45.0, 90.0, 180.0, 270.0, 333.0] {
            let to = geodesic_destination(from, az, 500_000.0);
            assert!((haversine_m(from, to) - 500_000.0).abs() < 1e-3, "{az}: {to:?}");
        }
        // North and east go where they say.
        assert!((geodesic_destination(from, 0.0, 111_195.0).y - 61.0).abs() < 1e-3);
        assert!(geodesic_destination(from, 90.0, 1000.0).x > 10.0);
        // Longitudes stay continuous across the antimeridian, so the points around one centre
        // form one shape (Tissot's indicatrix draws its circles with them).
        assert!(geodesic_destination(Coord::new(179.9, 0.0), 90.0, 100_000.0).x > 180.0);
    }

    #[test]
    fn ring_area_one_degree_square_at_equator() {
        let ring = [Coord::new(0.0, 0.0), Coord::new(1.0, 0.0), Coord::new(1.0, 1.0), Coord::new(0.0, 1.0)];
        let a = geodesic_ring_area_m2(&ring);
        assert!((a - 1.2364e10).abs() / 1.2364e10 < 0.01, "{a}");
    }

    #[test]
    fn nice_distances() {
        assert_eq!(nice_distance(730.0), 500.0);
        assert_eq!(nice_distance(2_400.0), 2_000.0);
        assert_eq!(nice_distance(0.0), 0.0);
        assert_eq!(nice_distance(f64::NAN), 0.0);
    }

    #[test]
    fn dms_carries_rounded_seconds() {
        assert_eq!(format_dms(Coord::new(12.5, -45.25)), "12°30′00.0″ E  45°15′00.0″ S");
        // 59.96″ rounds to 60.0″, which carries into the minutes and then the degrees.
        assert_eq!(format_dms(Coord::new(10.99999, -45.99999)), "11°00′00.0″ E  46°00′00.0″ S");
        assert_eq!(format_dms(Coord::new(-0.5 - 59.97 / 3600.0, 0.0)), "0°31′00.0″ W  0°00′00.0″ N");
    }

    #[test]
    fn extent_ops() {
        let mut e = Extent::EMPTY;
        assert!(e.is_empty());
        e.expand(Coord::new(1.0, 2.0));
        e.expand(Coord::new(f64::NAN, 0.0));
        e.expand(Coord::new(-1.0, 5.0));
        assert_eq!(e, Extent::new(-1.0, 2.0, 1.0, 5.0));
        assert!(e.intersects(&Extent::new(0.0, 0.0, 3.0, 3.0)));
        assert!(!e.intersects(&Extent::EMPTY));
    }
}
