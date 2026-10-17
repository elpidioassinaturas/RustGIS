//! Coordinate systems from WKT1, the text of a `.prj` file next to a Shapefile or a grid.
//!
//! An EPSG authority is used when the text names one. The `.prj` files ArcGIS and GDAL write have
//! none (`PROJCS["WGS_1984_UTM_Zone_31N",…]`), so Web Mercator and UTM are also known by name, and
//! any other projection is turned into a PROJ definition from its parameters.

use super::crs::{Crs, CustomCrs};

/// The most nested brackets read.
const MAX_DEPTH: usize = 32;

/// One value inside a WKT node's brackets.
#[derive(Debug)]
enum Item {
    Node(Node),
    Text(String),
    Num(f64),
    /// A bare word, such as `EAST` in `AXIS["X",EAST]`.
    Word,
}

/// `NAME[item, item, …]`.
#[derive(Debug)]
struct Node {
    name: String,
    items: Vec<Item>,
}

impl Node {
    /// The first child node of this name (case-insensitive).
    fn child(&self, name: &str) -> Option<&Node> {
        self.items.iter().find_map(|i| match i {
            Item::Node(n) if n.name.eq_ignore_ascii_case(name) => Some(n),
            _ => None,
        })
    }

    /// The node's quoted name: `"WGS_1984"` in `DATUM["WGS_1984",…]`.
    fn title(&self) -> &str {
        match self.items.first() {
            Some(Item::Text(t)) => t,
            _ => "",
        }
    }

    /// The `n`th number among the node's values.
    fn num(&self, n: usize) -> Option<f64> {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::Num(v) => Some(*v),
                _ => None,
            })
            .nth(n)
            .filter(|v| v.is_finite())
    }

    /// Every number among the node's values.
    fn nums(&self) -> Vec<f64> {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::Num(v) => Some(*v),
                _ => None,
            })
            .collect()
    }
}

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn peek(&mut self) -> Option<u8> {
        while self.s.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
        self.s.get(self.at).copied()
    }

    fn word(&mut self) -> String {
        let start = self.at;
        while self.s.get(self.at).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_') {
            self.at += 1;
        }
        String::from_utf8_lossy(self.s.get(start..self.at).unwrap_or_default()).into_owned()
    }

    fn node(&mut self, depth: usize) -> Result<Node, String> {
        if depth > MAX_DEPTH {
            return Err("the coordinate system is nested too deeply".into());
        }
        let name = self.word();
        if name.is_empty() {
            return Err("the coordinate system isn't WKT".into());
        }
        let close = match self.peek() {
            Some(b'[') => b']',
            Some(b'(') => b')',
            _ => return Err(format!("expected '[' after {name}")),
        };
        self.at += 1;
        let mut items = Vec::new();
        loop {
            let item = match self.peek() {
                Some(b'"') => Item::Text(self.text()?),
                Some(b) if b == b'-' || b == b'+' || b == b'.' || b.is_ascii_digit() => Item::Num(self.number()?),
                Some(b) if b.is_ascii_alphabetic() => {
                    let save = self.at;
                    self.word();
                    if matches!(self.peek(), Some(b'[' | b'(')) {
                        self.at = save;
                        Item::Node(self.node(depth + 1)?)
                    } else {
                        Item::Word
                    }
                }
                Some(b) if b == close && items.is_empty() => {
                    self.at += 1;
                    return Ok(Node { name, items });
                }
                _ => return Err(format!("unexpected text in {name}")),
            };
            items.push(item);
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b) if b == close => {
                    self.at += 1;
                    return Ok(Node { name, items });
                }
                _ => return Err(format!("expected ',' in {name}")),
            }
        }
    }

    /// A quoted string; `""` inside it is a quote.
    fn text(&mut self) -> Result<String, String> {
        self.at += 1;
        let mut out = Vec::new();
        loop {
            match self.s.get(self.at) {
                None => return Err("a quoted name isn't closed".into()),
                Some(b'"') if self.s.get(self.at + 1) == Some(&b'"') => {
                    out.push(b'"');
                    self.at += 2;
                }
                Some(b'"') => {
                    self.at += 1;
                    return Ok(String::from_utf8_lossy(&out).into_owned());
                }
                Some(b) => {
                    out.push(*b);
                    self.at += 1;
                }
            }
        }
    }

    fn number(&mut self) -> Result<f64, String> {
        let start = self.at;
        while self.s.get(self.at).is_some_and(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E')) {
            self.at += 1;
        }
        let t = std::str::from_utf8(self.s.get(start..self.at).unwrap_or_default()).unwrap_or("");
        t.parse().map_err(|_| format!("'{t}' isn't a number"))
    }
}

fn parse(text: &str) -> Result<Node, String> {
    let mut p = Parser { s: text.trim_start_matches('\u{feff}').as_bytes(), at: 0 };
    p.peek();
    let root = p.node(0)?;
    match p.peek() {
        None => Ok(root),
        Some(_) => Err("text follows the coordinate system".into()),
    }
}

/// Upper case with spaces, dashes and slashes as underscores, runs of them as one: Esri and EPSG
/// spellings of a name compare equal (`WGS 84 / UTM zone 31N` → `WGS_84_UTM_ZONE_31N`).
fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let c = if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' };
        if c != '_' || !(out.is_empty() || out.ends_with('_')) {
            out.push(c);
        }
    }
    out.trim_end_matches('_').to_string()
}

/// The coordinate system a WKT1 text (OGC or Esri) describes.
pub fn crs_from_wkt(text: &str) -> Result<Crs, String> {
    read_wkt(text).map(|(crs, _)| crs)
}

/// The coordinate system a WKT1 text describes, with a note when its datum has no conversion to
/// WGS 84 given (NAD 27, OSGB 36…), so positions may be off by up to a few hundred metres.
pub fn read_wkt(text: &str) -> Result<(Crs, Option<String>), String> {
    let root = parse(text)?;
    if let Some(a) = root.child("AUTHORITY")
        && a.title().eq_ignore_ascii_case("EPSG")
        && let Some(crs) = authority_code(a).and_then(Crs::from_epsg)
    {
        return Ok((crs, None));
    }
    match root.name.to_ascii_uppercase().as_str() {
        "GEOGCS" => Ok((geographic(&root)?, datum_note(Some(&root)))),
        "PROJCS" => {
            let crs = projected(&root)?;
            let note = if matches!(crs, Crs::Custom(_)) { datum_note(root.child("GEOGCS")) } else { None };
            Ok((crs, note))
        }
        other => Err(format!("{other} coordinate systems aren't supported")),
    }
}

/// The code in `AUTHORITY["EPSG","32631"]` (quoted or not).
fn authority_code(a: &Node) -> Option<u32> {
    match a.items.get(1)? {
        Item::Text(t) => t.trim().parse().ok(),
        Item::Num(n) if n.fract() == 0.0 && *n > 0.0 && *n <= f64::from(u32::MAX) => Some(*n as u32),
        _ => None,
    }
}

/// Longitude/latitude in degrees: WGS 84 itself for WGS 84, NAD 83, ETRS 89 and SIRGAS 2000
/// (which differ by a metre or so) from Greenwich, otherwise on the file's own datum and prime
/// meridian.
fn geographic(g: &Node) -> Result<Crs, String> {
    if let Some(f) = g.child("UNIT").and_then(|u| u.num(0))
        && (f - 1f64.to_radians()).abs() > 1e-9
    {
        return Err(format!("angles in {} aren't supported", g.child("UNIT").map_or("", Node::title)));
    }
    let datum = g.child("DATUM");
    let greenwich = g.child("PRIMEM").and_then(|m| m.num(0)).is_none_or(|pm| pm == 0.0);
    if greenwich && (datum.is_none() || wgs84_like(datum).is_some()) {
        return Ok(Crs::LonLat);
    }
    let label = match g.title().trim() {
        "" => "Custom coordinate system".to_string(),
        t => t.to_string(),
    };
    Crs::custom(CustomCrs {
        proj4: format!("+proj=longlat {} +no_defs", earth(Some(g))),
        label,
        geokeys: Vec::new(),
        doubles: Vec::new(),
        ascii: String::new(),
    })
}

/// A projected system: by EPSG code for the common names, else as a PROJ definition.
fn projected(p: &Node) -> Result<Crs, String> {
    let name = norm(p.title());
    let method = norm(p.child("PROJECTION").map_or("", Node::title));
    if name.contains("WEB_MERCATOR")
        || name.contains("PSEUDO_MERCATOR")
        || method == "MERCATOR_AUXILIARY_SPHERE"
        || method == "POPULAR_VISUALISATION_PSEUDO_MERCATOR"
    {
        return Ok(Crs::WebMercator);
    }
    if let Some(crs) = utm(&name).and_then(Crs::from_epsg) {
        return Ok(crs);
    }
    custom(p, &method)
}

/// The EPSG code of a UTM zone on WGS 84, NAD 83, ETRS 89 or SIRGAS 2000 from its name
/// (`WGS_1984_UTM_Zone_31N`, `WGS 84 / UTM zone 31N`, `NAD83 / UTM zone 17N`,
/// `SIRGAS_2000_UTM_Zone_24S`).
fn utm(name: &str) -> Option<u32> {
    let (datum, zone) = name.split_once("_UTM_ZONE_")?;
    let digits: String = zone.chars().take_while(char::is_ascii_digit).collect();
    let z: u32 = digits.parse().ok().filter(|z| (1..=60).contains(z))?;
    let north = match zone.get(digits.len()..)? {
        "N" => true,
        "S" => false,
        _ => return None,
    };
    match datum {
        "WGS_1984" | "WGS_84" => Some(if north { 32600 } else { 32700 } + z),
        "NAD_1983" | "NAD83" if north => Some(26900 + z),
        "ETRS_1989" | "ETRS89" if north => Some(25800 + z),
        "SIRGAS_2000" | "SIRGAS2000" => match (north, z) {
            (true, 11..=22) => Some(31954 + z),
            (true, 23) => Some(6210),
            (true, 24) => Some(6211),
            (false, 17..=25) => Some(31960 + z),
            (false, 26) => Some(5396),
            _ => None,
        },
        _ => None,
    }
}

/// A projection's PROJ definition built from its WKT parameters.
fn custom(p: &Node, method: &str) -> Result<Crs, String> {
    let params: Vec<(String, f64)> = p
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Node(n) if n.name.eq_ignore_ascii_case("PARAMETER") => Some((norm(n.title()), n.num(0)?)),
            _ => None,
        })
        .collect();
    let get = |names: &[&str]| names.iter().find_map(|n| params.iter().find(|(k, _)| k == n).map(|(_, v)| *v));
    // Metres per linear unit: false eastings and northings are in the system's units, PROJ's in
    // metres.
    let to_m = p.child("UNIT").and_then(|u| u.num(0)).filter(|m| *m > 0.0).unwrap_or(1.0);
    let lat0 = get(&["LATITUDE_OF_ORIGIN", "LATITUDE_OF_CENTER"]).unwrap_or(0.0);
    let lon0 = get(&["CENTRAL_MERIDIAN", "LONGITUDE_OF_CENTER", "LONGITUDE_OF_ORIGIN"]).unwrap_or(0.0);
    let x0 = get(&["FALSE_EASTING"]).unwrap_or(0.0) * to_m;
    let y0 = get(&["FALSE_NORTHING"]).unwrap_or(0.0) * to_m;
    let k0 = get(&["SCALE_FACTOR"]).unwrap_or(1.0);
    let lat1 = get(&["STANDARD_PARALLEL_1"]);
    let lat2 = get(&["STANDARD_PARALLEL_2"]);
    let at = format!("+lon_0={lon0} +x_0={x0} +y_0={y0}");
    let proj = match method {
        "TRANSVERSE_MERCATOR" | "GAUSS_KRUGER" => format!("+proj=tmerc +lat_0={lat0} +k_0={k0} {at}"),
        "MERCATOR" | "MERCATOR_1SP" | "MERCATOR_2SP" => match lat1 {
            Some(ts) => format!("+proj=merc +lat_ts={ts} {at}"),
            None => format!("+proj=merc +k_0={k0} {at}"),
        },
        "LAMBERT_CONFORMAL_CONIC" | "LAMBERT_CONFORMAL_CONIC_1SP" | "LAMBERT_CONFORMAL_CONIC_2SP" => {
            let l1 = lat1.unwrap_or(lat0);
            let l2 = lat2.unwrap_or(l1);
            format!("+proj=lcc +lat_1={l1} +lat_2={l2} +lat_0={lat0} +k_0={k0} {at}")
        }
        "ALBERS" | "ALBERS_CONIC_EQUAL_AREA" => {
            let l1 = lat1.unwrap_or(lat0);
            let l2 = lat2.unwrap_or(l1);
            format!("+proj=aea +lat_1={l1} +lat_2={l2} +lat_0={lat0} {at}")
        }
        "LAMBERT_AZIMUTHAL_EQUAL_AREA" => format!("+proj=laea +lat_0={lat0} {at}"),
        "AZIMUTHAL_EQUIDISTANT" => format!("+proj=aeqd +lat_0={lat0} {at}"),
        "STEREOGRAPHIC" => format!("+proj=stere +lat_0={lat0} +k_0={k0} {at}"),
        "OBLIQUE_STEREOGRAPHIC" | "DOUBLE_STEREOGRAPHIC" => format!("+proj=sterea +lat_0={lat0} +k_0={k0} {at}"),
        "POLAR_STEREOGRAPHIC" | "STEREOGRAPHIC_NORTH_POLE" | "STEREOGRAPHIC_SOUTH_POLE" => {
            // The pole on the side of the latitude of true scale. A latitude of origin at the pole
            // (UPS, EPSG:32761) gives the scale there instead.
            let ts = lat1.unwrap_or(lat0);
            let pole = if method.ends_with("SOUTH_POLE") || ts < 0.0 { -90.0 } else { 90.0 };
            if ts.abs() >= 90.0 || ts == 0.0 {
                format!("+proj=stere +lat_0={pole} +k_0={k0} {at}")
            } else {
                format!("+proj=stere +lat_0={pole} +lat_ts={ts} {at}")
            }
        }
        "EQUIRECTANGULAR" | "EQUIDISTANT_CYLINDRICAL" | "PLATE_CARREE" => format!("+proj=eqc +lat_ts={} {at}", lat1.unwrap_or(0.0)),
        "CYLINDRICAL_EQUAL_AREA" | "LAMBERT_CYLINDRICAL_EQUAL_AREA" | "BEHRMANN" => {
            let ts = lat1.unwrap_or(if method == "BEHRMANN" { 30.0 } else { 0.0 });
            format!("+proj=cea +lat_ts={ts} {at}")
        }
        "MOLLWEIDE" => format!("+proj=moll {at}"),
        "MILLER_CYLINDRICAL" => format!("+proj=mill {at}"),
        "" => return Err("the projection isn't named".into()),
        _ => return Err(format!("the {} projection isn't supported", p.child("PROJECTION").map_or("", Node::title))),
    };
    let units = if (to_m - 1.0).abs() > 1e-12 { format!("+to_meter={to_m}") } else { "+units=m".to_string() };
    let label = match p.title().trim() {
        "" => "Custom projection".to_string(),
        t => t.to_string(),
    };
    Crs::custom(CustomCrs {
        proj4: format!("{proj} {} {units} +no_defs", earth(p.child("GEOGCS"))),
        label,
        geokeys: Vec::new(),
        doubles: Vec::new(),
        ascii: String::new(),
    })
}

/// The PROJ datum for WGS 84 and the datums that differ from it by a metre or so.
fn wgs84_like(datum: Option<&Node>) -> Option<&'static str> {
    match norm(datum.map_or("", Node::title)).trim_start_matches("D_") {
        "WGS_1984" | "WGS84" | "WORLD_GEODETIC_SYSTEM_1984" => Some("+datum=WGS84"),
        "NORTH_AMERICAN_1983"
        | "NORTH_AMERICAN_DATUM_1983"
        | "NAD_1983"
        | "NAD83"
        | "ETRS_1989"
        | "ETRS89"
        | "EUROPEAN_TERRESTRIAL_REFERENCE_SYSTEM_1989"
        | "GDA_1994"
        | "GDA94"
        // SIRGAS 2000, the datum of Brazil and most of Latin America (EPSG:4674): its
        // transformation to WGS 84 is null, like the others here.
        | "SIRGAS_2000"
        | "SIRGAS2000"
        | "SISTEMA_DE_REFERENCIA_GEOCENTRICO_PARA_LAS_AMERICAS_2000" => Some("+ellps=GRS80 +towgs84=0,0,0"),
        _ => None,
    }
}

/// The datum's `TOWGS84` shift, when it gives a usable one.
fn towgs84(datum: Option<&Node>) -> Option<Vec<f64>> {
    datum.and_then(|d| d.child("TOWGS84")).map(Node::nums).filter(|t| (t.len() == 3 || t.len() == 7) && t.iter().all(|v| v.is_finite()))
}

/// A note when the datum (of a `GEOGCS`) is neither WGS 84-like nor shifted to it by `TOWGS84`.
fn datum_note(geog: Option<&Node>) -> Option<String> {
    let datum = geog?.child("DATUM")?;
    if wgs84_like(Some(datum)).is_some() || towgs84(Some(datum)).is_some() {
        return None;
    }
    Some(format!(
        "the datum {} has no conversion to WGS 84 in the file, so positions may be off by up to a few hundred metres",
        datum.title()
    ))
}

/// The datum of a `GEOGCS`, by name for the common ones, else by its spheroid's axes; and its
/// prime meridian.
fn earth(geog: Option<&Node>) -> String {
    let datum = geog.and_then(|g| g.child("DATUM"));
    let mut out = match wgs84_like(datum) {
        Some(d) => d.to_string(),
        None => {
            let sph = datum.and_then(|d| d.child("SPHEROID").or_else(|| d.child("ELLIPSOID")));
            let mut s = match (sph.and_then(|s| s.num(0)), sph.and_then(|s| s.num(1))) {
                (Some(a), Some(rf)) if a > 0.0 && rf > 0.0 => format!("+a={a} +rf={rf}"),
                (Some(a), _) if a > 0.0 => format!("+a={a} +b={a}"),
                _ => "+datum=WGS84".to_string(),
            };
            if let Some(t) = towgs84(datum) {
                s.push_str(&format!(" +towgs84={}", t.iter().map(f64::to_string).collect::<Vec<_>>().join(",")));
            }
            s
        }
    };
    if let Some(pm) = geog.and_then(|g| g.child("PRIMEM")).and_then(|m| m.num(0))
        && pm != 0.0
    {
        out.push_str(&format!(" +pm={pm}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use rustgis_geom::Coord;

    use super::*;

    const GEOG_4326: &str = r#"GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]]"#;
    const ESRI_3857: &str = r#"PROJCS["WGS_1984_Web_Mercator_Auxiliary_Sphere",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Mercator_Auxiliary_Sphere"],PARAMETER["False_Easting",0.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",0.0],PARAMETER["Standard_Parallel_1",0.0],PARAMETER["Auxiliary_Sphere_Type",0.0],UNIT["Meter",1.0]]"#;
    const ESRI_32631: &str = r#"PROJCS["WGS_1984_UTM_Zone_31N",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",3.0],PARAMETER["Scale_Factor",0.9996],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#;

    #[test]
    fn esri_names() {
        assert_eq!(crs_from_wkt(GEOG_4326), Ok(Crs::LonLat));
        assert_eq!(crs_from_wkt(ESRI_3857), Ok(Crs::WebMercator));
        assert_eq!(crs_from_wkt(ESRI_32631), Ok(Crs::Epsg(32631)));
        assert_eq!(utm("WGS_84_UTM_ZONE_17S"), Some(32717));
        assert_eq!(utm(&norm("NAD83 / UTM zone 17N")), Some(26917));
        assert_eq!(utm("ETRS_1989_UTM_ZONE_32N"), Some(25832));
        assert_eq!(utm("WGS_1984_UTM_ZONE_61N"), None);
    }

    #[test]
    fn epsg_authority() {
        let wkt = r#"PROJCS["anything",GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563,AUTHORITY["EPSG","7030"]],AUTHORITY["EPSG","6326"]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433],AUTHORITY["EPSG","4326"]],PROJECTION["Transverse_Mercator"],PARAMETER["central_meridian",-81],UNIT["metre",1],AXIS["Easting",EAST],AXIS["Northing",NORTH],AUTHORITY["EPSG","32617"]]"#;
        assert_eq!(crs_from_wkt(wkt), Ok(Crs::Epsg(32617)));
    }

    #[test]
    fn projection_parameters() {
        // NAD 1983 StatePlane New York Long Island FIPS 3104 (US feet), as ArcGIS writes it.
        let wkt = r#"PROJCS["NAD_1983_StatePlane_New_York_Long_Island_FIPS_3104_Feet",GEOGCS["GCS_North_American_1983",DATUM["D_North_American_1983",SPHEROID["GRS_1980",6378137.0,298.257222101]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Lambert_Conformal_Conic"],PARAMETER["False_Easting",984250.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",-74.0],PARAMETER["Standard_Parallel_1",40.66666666666666],PARAMETER["Standard_Parallel_2",41.03333333333333],PARAMETER["Latitude_Of_Origin",40.16666666666666],UNIT["Foot_US",0.3048006096012192]]"#;
        let crs = crs_from_wkt(wkt).unwrap();
        assert!(matches!(crs, Crs::Custom(_)));
        assert_eq!(crs.label(), "NAD_1983_StatePlane_New_York_Long_Island_FIPS_3104_Feet");
        // Times Square, from PROJ: 988_267 ft E, 215_437 ft N.
        let ll = crs.transformer().unwrap().to_lonlat(Coord::new(988_267.0, 215_437.1)).unwrap();
        assert!((ll.x + 73.9855).abs() < 1e-5 && (ll.y - 40.758).abs() < 1e-5, "{ll:?}");

        // The UTM zone by its parameters, when the name isn't one known.
        let renamed = ESRI_32631.replace("WGS_1984_UTM_Zone_31N", "My_Grid");
        let crs = crs_from_wkt(&renamed).unwrap();
        let ll = crs.transformer().unwrap().to_lonlat(Coord::new(500_000.0, 5_000_000.0)).unwrap();
        assert!((ll.x - 3.0).abs() < 1e-6 && (ll.y - 45.153_477).abs() < 1e-5, "{ll:?}");
    }

    #[test]
    fn datums_and_prime_meridians() {
        // A prime meridian east of Greenwich moves longitudes east.
        let paris = r#"GEOGCS["Paris lon/lat",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Paris",2.33722917],UNIT["Degree",0.0174532925199433]]"#;
        let (crs, note) = read_wkt(paris).unwrap();
        assert!(matches!(crs, Crs::Custom(_)) && note.is_none());
        let ll = crs.transformer().unwrap().to_lonlat(Coord::new(0.0, 48.85)).unwrap();
        assert!((ll.x - 2.33722917).abs() < 1e-6 && (ll.y - 48.85).abs() < 1e-6, "{ll:?}");
        // NAD 27 with no TOWGS84: read, with a note.
        let nad27 = r#"GEOGCS["GCS_North_American_1927",DATUM["D_North_American_1927",SPHEROID["Clarke_1866",6378206.4,294.9786982]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]]"#;
        let (crs, note) = read_wkt(nad27).unwrap();
        assert!(crs.is_geographic());
        assert!(note.is_some_and(|n| n.contains("D_North_American_1927")));
        // With TOWGS84 there's nothing to say.
        let shifted = nad27.replace("294.9786982]]", "294.9786982],TOWGS84[-8,160,176,0,0,0,0]]");
        assert_eq!(read_wkt(&shifted).map(|r| r.1), Ok(None));
        // Esri names for WGS 84 systems need no note.
        assert_eq!(read_wkt(GEOG_4326), Ok((Crs::LonLat, None)));
        assert_eq!(read_wkt(ESRI_32631), Ok((Crs::Epsg(32631), None)));
    }

    #[test]
    fn sirgas_2000_reads_as_wgs_84_without_a_note() {
        // As FBDS ships its hydrography of Brazil (Esri names), projected and geographic.
        let utm_24s = r#"PROJCS["SIRGAS_2000_UTM_Zone_24S",GEOGCS["GCS_SIRGAS_2000",DATUM["D_SIRGAS_2000",SPHEROID["GRS_1980",6378137.0,298.257222101]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",10000000.0],PARAMETER["Central_Meridian",-39.0],PARAMETER["Scale_Factor",0.9996],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#;
        assert_eq!(read_wkt(utm_24s), Ok((Crs::Epsg(31984), None)));
        let geographic = r#"GEOGCS["GCS_SIRGAS_2000",DATUM["D_SIRGAS_2000",SPHEROID["GRS_1980",6378137.0,298.257222101]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]]"#;
        assert_eq!(read_wkt(geographic), Ok((Crs::LonLat, None)));
        // The EPSG datum name, as OGC WKT writes it.
        let ogc = geographic.replace("D_SIRGAS_2000", "Sistema_de_Referencia_Geocentrico_para_las_AmericaS_2000");
        assert_eq!(read_wkt(&ogc), Ok((Crs::LonLat, None)));
        // Every SIRGAS 2000 UTM zone with an EPSG code, by its name.
        assert_eq!(utm(&norm("SIRGAS 2000 / UTM zone 11N")), Some(31965));
        assert_eq!(utm("SIRGAS_2000_UTM_ZONE_22N"), Some(31976));
        assert_eq!(utm("SIRGAS_2000_UTM_ZONE_23N"), Some(6210));
        assert_eq!(utm("SIRGAS_2000_UTM_ZONE_24N"), Some(6211));
        assert_eq!(utm("SIRGAS_2000_UTM_ZONE_17S"), Some(31977));
        assert_eq!(utm("SIRGAS_2000_UTM_ZONE_23S"), Some(31983));
        assert_eq!(utm("SIRGAS_2000_UTM_ZONE_25S"), Some(31985));
        assert_eq!(utm("SIRGAS_2000_UTM_ZONE_26S"), Some(5396));
        assert_eq!(utm("SIRGAS_2000_UTM_ZONE_10S"), None);
        for code in [31965, 31976, 6210, 6211, 31977, 31985, 5396] {
            assert!(Crs::from_epsg(code).is_some(), "EPSG:{code}");
        }
        // A zone without an EPSG code is still read, by its parameters, and without a note.
        let zone_10s = utm_24s.replace("Zone_24S", "Zone_10S").replace("-39.0", "-123.0");
        let (crs, note) = read_wkt(&zone_10s).unwrap();
        assert!(matches!(crs, Crs::Custom(_)) && note.is_none());
        // Brazil's older datums keep the note: without TOWGS84 they're tens of metres off.
        for (datum, spheroid) in [
            ("D_South_American_1969", r#"SPHEROID["GRS_1967_Truncated",6378160.0,298.25]"#),
            ("D_Corrego_Alegre", r#"SPHEROID["International_1924",6378388.0,297.0]"#),
        ] {
            let old = format!(r#"GEOGCS["GCS_old",DATUM["{datum}",{spheroid}],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]]"#);
            assert!(read_wkt(&old).unwrap().1.is_some_and(|n| n.contains(datum)), "{datum}");
        }
    }

    #[test]
    fn polar_stereographic() {
        // GDAL's WKT for EPSG:3031 (true scale at 71°S) and EPSG:32761 (UPS South, scale 0.994 at
        // the pole), without their authorities; the point is 45°E 75°S, from PROJ.
        let geog = r#"GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
        let antarctic = format!(
            r#"PROJCS["Antarctic",{geog},PROJECTION["Polar_Stereographic"],PARAMETER["latitude_of_origin",-71],PARAMETER["central_meridian",0],PARAMETER["false_easting",0],PARAMETER["false_northing",0],UNIT["metre",1]]"#
        );
        let ups = format!(
            r#"PROJCS["UPS South",{geog},PROJECTION["Polar_Stereographic"],PARAMETER["latitude_of_origin",-90],PARAMETER["central_meridian",0],PARAMETER["scale_factor",0.994],PARAMETER["false_easting",2000000],PARAMETER["false_northing",2000000],UNIT["metre",1]]"#
        );
        for (wkt, xy) in [(antarctic, 1_158_794.740_772_6), (ups, 3_184_085.797_412_3)] {
            let ll = crs_from_wkt(&wkt).unwrap().transformer().unwrap().to_lonlat(Coord::new(xy, xy)).unwrap();
            assert!((ll.x - 45.0).abs() < 1e-6 && (ll.y + 75.0).abs() < 1e-6, "{ll:?}");
        }
    }

    #[test]
    fn hostile_text() {
        for bad in
            ["", "garbage", "PROJCS[", "PROJCS[\"x\"", "PROJCS[\"x\",PROJECTION[\"Hotine\"]]", "GEOGCS[\"x\"] trailing", "VERT_CS[\"x\"]"]
        {
            assert!(crs_from_wkt(bad).is_err(), "{bad}");
        }
        let deep = format!("{}{}", "A[".repeat(10_000), "]".repeat(10_000));
        assert!(crs_from_wkt(&deep).is_err());
        assert_eq!(crs_from_wkt(r#"GEOGCS["a ""quoted"" name",UNIT["Degree",0.0174532925199433]]"#), Ok(Crs::LonLat));
    }
}
