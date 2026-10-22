//! The tools.

use geo::{BooleanOps, Contains, ConvexHull, InteriorPoint, Intersects, MultiPolygon, Orient, Simplify, orient::Direction};
use rustgis_data::raster::Crs;
use rustgis_data::{Coord, Dataset, Field, FieldType, Geometry, GeometryType, Value};
use rustgis_geom::{Extent, geodesic_length_m, geodesic_ring_area_m2};
use serde_json::Value as Json;

use crate::convert::{from_line, from_multipolygon, from_point, to_geo, to_multiline, to_multipolygon};
use crate::geodesic::geodesic_buffer;
use crate::{Inputs, Output, OutputLayer, ParamKind as K, ParamSpec as P, ToolResult, ToolSpec};

const UNITS: &[&str] = &["Kilometers", "Meters", "Miles"];
const KRIGING_MODELS: &[&str] = &["Spherical", "Exponential", "Gaussian"];
const MATCH_OPTIONS: &[&str] = &["Intersects", "Contains", "Within"];
const JOIN_OPERATIONS: &[&str] = &["Join one to one", "Join one to many"];
const TARGET_RECORDS: &[&str] = &["Keep all", "Keep only matching"];

pub static TOOLS: &[ToolSpec] = &[
    ToolSpec {
        id: "buffer",
        name: "Buffer",
        toolbox: "Analysis › Proximity",
        summary: "Creates polygons around input features at a given distance.",
        params: &[
            P { name: "input", label: "Input Features", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "distance", label: "Distance", kind: K::Number, required: true, default: "100", help: "" },
            P { name: "units", label: "Units", kind: K::Choice(UNITS), required: false, default: "\"Kilometers\"", help: "" },
            P { name: "dissolve", label: "Dissolve", kind: K::Choice(&["None", "All"]), required: false, default: "\"None\"", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: buffer,
    },
    ToolSpec {
        id: "clip",
        name: "Clip",
        toolbox: "Analysis › Extract",
        summary: "Extracts the parts of input features that fall inside clip polygons.",
        params: &[
            P { name: "input", label: "Input Features", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "clip", label: "Clip Features", kind: K::PolygonLayer, required: true, default: "", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: clip,
    },
    ToolSpec {
        id: "erase",
        name: "Erase",
        toolbox: "Analysis › Overlay",
        summary: "Removes the parts of polygon features that overlap erase polygons.",
        params: &[
            P { name: "input", label: "Input Features", kind: K::PolygonLayer, required: true, default: "", help: "" },
            P { name: "erase", label: "Erase Features", kind: K::PolygonLayer, required: true, default: "", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: erase,
    },
    ToolSpec {
        id: "dissolve",
        name: "Dissolve",
        toolbox: "Data Management › Generalization",
        summary: "Merges polygons that share a field value into one feature per value.",
        params: &[
            P { name: "input", label: "Input Features", kind: K::PolygonLayer, required: true, default: "", help: "" },
            P { name: "field", label: "Dissolve Field", kind: K::Field, required: false, default: "", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: dissolve,
    },
    ToolSpec {
        id: "convexHull",
        name: "Minimum Bounding Geometry",
        toolbox: "Data Management › Features",
        summary: "Encloses each feature, or all features, in its convex hull.",
        params: &[
            P { name: "input", label: "Input Features", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "group", label: "Group Option", kind: K::Choice(&["All", "None"]), required: false, default: "\"All\"", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: convex_hull,
    },
    ToolSpec {
        id: "featureToPoint",
        name: "Feature To Point",
        toolbox: "Data Management › Features",
        summary: "Creates a point inside each input feature.",
        params: &[
            P { name: "input", label: "Input Features", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: feature_to_point,
    },
    ToolSpec {
        id: "xyTableToPoint",
        name: "XY Table To Point",
        toolbox: "Data Management › Features",
        summary: "Creates points from a table's x and y coordinate fields.",
        params: &[
            P { name: "input", label: "Input Table", kind: K::Layer, required: true, default: "", help: "" },
            P {
                name: "x",
                label: "X Field",
                kind: K::CoordinateField,
                required: true,
                default: "",
                help: "Longitude or easting: numbers, or text holding them. A single comma is the decimal separator (1,234 is 1.234).",
            },
            P {
                name: "y",
                label: "Y Field",
                kind: K::CoordinateField,
                required: true,
                default: "",
                help: "Latitude or northing, read as the X field is.",
            },
            P {
                name: "crs",
                label: "Coordinate System",
                kind: K::Text,
                required: false,
                default: "\"EPSG:4326\"",
                help: "The coordinates' system, as EPSG:<code> or WKT; longitude/latitude (EPSG:4326) when empty.",
            },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: xy_table_to_point,
    },
    ToolSpec {
        id: "simplify",
        name: "Simplify",
        toolbox: "Cartography › Generalization",
        summary: "Removes vertices that deviate less than a tolerance (Douglas–Peucker).",
        params: &[
            P { name: "input", label: "Input Features", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "tolerance", label: "Tolerance (km)", kind: K::Number, required: true, default: "25", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: simplify,
    },
    ToolSpec {
        id: "tissot",
        name: "Tissot's Indicatrix",
        toolbox: "Cartography › Projections",
        summary: "Geodesic circles on a grid that show how the map's projection distorts scale, area and angles.",
        params: &[
            P { name: "spacing", label: "Grid Spacing (degrees)", kind: K::Number, required: false, default: "30", help: "" },
            P { name: "radius", label: "Circle Radius (km)", kind: K::Number, required: false, default: "500", help: "" },
            P {
                name: "area",
                label: "Area",
                kind: K::Choice(&["World", "Map Extent"]),
                required: false,
                default: "\"World\"",
                help: "Map Extent fills the view: for a country or a state, use a smaller spacing and radius.",
            },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: crate::tissot::run,
    },
    ToolSpec {
        id: "a5Grid",
        name: "A5 Grid",
        toolbox: "Analysis › DGGS",
        summary: "A5's equal-area pentagonal cells at a resolution, over the map's extent, a layer or the world, as polygons with their ids.",
        params: &[
            P {
                name: "resolution",
                label: "Resolution (0–30)",
                kind: K::Number,
                required: false,
                default: "5",
                help: "Each step divides a cell into about four: resolution 5 cells are about 33,000 km², 9 about 130 km², 12 about 2 km².",
            },
            P {
                name: "area",
                label: "Area",
                kind: K::Choice(&["Map Extent", "Input Layer", "World"]),
                required: false,
                default: "\"Map Extent\"",
                help: "Input Layer covers its polygons (or the extent of its points and lines).",
            },
            P { name: "input", label: "Input Layer", kind: K::Layer, required: false, default: "", help: "" },
            P {
                name: "containment",
                label: "Cells",
                kind: K::Choice(&["Overlapping", "Center"]),
                required: false,
                default: "\"Overlapping\"",
                help: "Overlapping covers the area without gaps; Center keeps the cells whose centre is inside.",
            },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: crate::dggs::grid,
    },
    ToolSpec {
        id: "a5Aggregate",
        name: "Aggregate Points to A5",
        toolbox: "Analysis › DGGS",
        summary: "Counts a layer's points by the A5 cell holding them (cells of equal area, so the count is a density), with a field's sum and mean.",
        params: &[
            P { name: "input", label: "Input Points", kind: K::PointLayer, required: true, default: "", help: "" },
            P { name: "resolution", label: "Resolution (0–30)", kind: K::Number, required: false, default: "9", help: "" },
            P {
                name: "field",
                label: "Field to Sum and Average",
                kind: K::NumericField,
                required: false,
                default: "",
                help: "A feature's value counts once in each cell: a multipoint's, however many of its points fall in the cell.",
            },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: crate::dggs::aggregate,
    },
    ToolSpec {
        id: "selectByLocation",
        name: "Select Layer By Location",
        toolbox: "Data Management › Layers and Table Views",
        summary: "Selects features that intersect features in another layer.",
        params: &[
            P { name: "input", label: "Input Features", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "select", label: "Selecting Features", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "distance", label: "Search Distance (km)", kind: K::Number, required: false, default: "0", help: "" },
            P { name: "invert", label: "Invert", kind: K::Choice(&["No", "Yes"]), required: false, default: "\"No\"", help: "" },
        ],
        run: select_by_location,
    },
    ToolSpec {
        id: "selectByAttributes",
        name: "Select Layer By Attribute",
        toolbox: "Data Management › Layers and Table Views",
        summary: "Selects features with a SQL where clause, e.g. pop_est > 1000000.",
        params: &[
            P { name: "input", label: "Input Rows", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "where", label: "Where Clause", kind: K::Text, required: true, default: "", help: "" },
        ],
        run: select_by_attributes,
    },
    ToolSpec {
        id: "countPoints",
        name: "Summarize Within",
        toolbox: "Analysis › Summarize",
        summary: "Counts the points that fall inside each polygon.",
        params: &[
            P { name: "polygons", label: "Input Polygons", kind: K::PolygonLayer, required: true, default: "", help: "" },
            P { name: "points", label: "Input Summary Features", kind: K::PointLayer, required: true, default: "", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: count_points,
    },
    ToolSpec {
        id: "calculateGeometry",
        name: "Calculate Geometry Attributes",
        toolbox: "Data Management › Features",
        summary: "Adds geodesic area (km²), length (km) or centroid coordinates as fields.",
        params: &[P { name: "input", label: "Input Features", kind: K::Layer, required: true, default: "", help: "" }],
        run: calculate_geometry,
    },
    ToolSpec {
        id: "summaryStatistics",
        name: "Summary Statistics",
        toolbox: "Analysis › Statistics",
        summary: "Count, sum, mean, min, max and standard deviation of a field, optionally per case field.",
        params: &[
            P { name: "input", label: "Input Table", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "field", label: "Statistics Field", kind: K::NumericField, required: true, default: "", help: "" },
            P { name: "case", label: "Case Field", kind: K::Field, required: false, default: "", help: "" },
        ],
        run: summary_statistics,
    },
    ToolSpec {
        id: "exportFeatures",
        name: "Export Features",
        toolbox: "Conversion",
        summary: "Copies the (selected) features of a layer to a new layer.",
        params: &[
            P { name: "input", label: "Input Features", kind: K::Layer, required: true, default: "", help: "" },
            P { name: "where", label: "Expression", kind: K::Text, required: false, default: "", help: "" },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: export_features,
    },
    ToolSpec {
        id: "idwInterpolation",
        name: "IDW Interpolation",
        toolbox: "Analysis › Interpolation",
        summary: "Interpolates a raster surface from feature values by inverse distance weighting, as QGIS does.",
        params: &[
            P {
                name: "input",
                label: "Vector Layer",
                kind: K::VectorLayer,
                required: true,
                default: "",
                help: "Points, or lines and polygons whose vertices are used as points.",
            },
            P { name: "field", label: "Interpolation Attribute", kind: K::NumericField, required: true, default: "", help: "" },
            P {
                name: "power",
                label: "Distance Coefficient P",
                kind: K::Number,
                required: false,
                default: "2",
                help: "How fast a point's weight falls off with distance (weight = 1/distanceᴾ). 0 averages every point.",
            },
            P {
                name: "extent",
                label: "Extent",
                kind: K::Text,
                required: false,
                default: "",
                help: "xmin,ymin,xmax,ymax in degrees; the input's extent when empty.",
            },
            P {
                name: "pixelSize",
                label: "Pixel Size (°)",
                kind: K::Number,
                required: false,
                default: "",
                help: "The output cell size in degrees; 300 cells across the extent when empty.",
            },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: idw_interpolation,
    },
    ToolSpec {
        id: "spatialJoin",
        name: "Spatial Join",
        toolbox: "Analysis › Overlay",
        summary: "Joins attributes from a join layer to a target layer based on their spatial relationship.",
        params: &[
            P {
                name: "target",
                label: "Target Features",
                kind: K::Layer,
                required: true,
                default: "",
                help: "The features that receive joined attributes.",
            },
            P {
                name: "join",
                label: "Join Features",
                kind: K::Layer,
                required: true,
                default: "",
                help: "The features whose attributes are joined.",
            },
            P {
                name: "match",
                label: "Match Option",
                kind: K::Choice(MATCH_OPTIONS),
                required: false,
                default: "\"Intersects\"",
                help: "The spatial relationship between target and join features.",
            },
            P {
                name: "operation",
                label: "Join Operation",
                kind: K::Choice(JOIN_OPERATIONS),
                required: false,
                default: "\"Join one to one\"",
                help: "One to one joins the first match and counts total matches; one to many creates a row for each match.",
            },
            P {
                name: "keepAll",
                label: "Target Records",
                kind: K::Choice(TARGET_RECORDS),
                required: false,
                default: "\"Keep all\"",
                help: "Whether target features without matching join features are kept.",
            },
            P {
                name: "prefix",
                label: "Field Prefix",
                kind: K::Text,
                required: false,
                default: "\"join_\"",
                help: "Prefix added to join feature fields to avoid collision.",
            },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: spatial_join,
    },
    ToolSpec {
        id: "kriging",
        name: "Ordinary Kriging",
        toolbox: "Analysis › Interpolation",
        summary: "Interpolates a 2-band raster surface (Prediction and Variance) from point features using Ordinary Kriging.",
        params: &[
            P {
                name: "input",
                label: "Vector Layer",
                kind: K::VectorLayer,
                required: true,
                default: "",
                help: "Points (or bounding-box centres for lines and polygons) with values to interpolate.",
            },
            P { name: "field", label: "Interpolation Attribute", kind: K::NumericField, required: true, default: "", help: "" },
            P {
                name: "model",
                label: "Variogram Model",
                kind: K::Choice(KRIGING_MODELS),
                required: false,
                default: "\"Spherical\"",
                help: "Spatial autocorrelation model: Spherical, Exponential, or Gaussian.",
            },
            P {
                name: "neighbours",
                label: "Max Neighbors",
                kind: K::Number,
                required: false,
                default: "12",
                help: "Number of nearest points to solve with (default 12, max 32).",
            },
            P {
                name: "nugget",
                label: "Nugget",
                kind: K::Number,
                required: false,
                default: "",
                help: "Micro-scale variance/measurement error at zero distance (default 0).",
            },
            P {
                name: "sill",
                label: "Sill",
                kind: K::Number,
                required: false,
                default: "",
                help: "Total asymptotic variance (defaults to sample variance).",
            },
            P {
                name: "range",
                label: "Range (Degrees)",
                kind: K::Number,
                required: false,
                default: "",
                help: "Spatial correlation distance in degrees (defaults to 1/3 extent diagonal).",
            },
            P {
                name: "extent",
                label: "Extent",
                kind: K::Text,
                required: false,
                default: "",
                help: "xmin,ymin,xmax,ymax in degrees; the input's extent when empty.",
            },
            P {
                name: "pixelSize",
                label: "Pixel Size (°)",
                kind: K::Number,
                required: false,
                default: "",
                help: "The output cell size in degrees; up to 250 cells across the extent when empty.",
            },
            P { name: "output", label: "Output Name", kind: K::Text, required: false, default: "", help: "" },
        ],
        run: kriging_interpolation,
    },
];

// ---------------------------------------------------------------------------------------------
// helpers

fn s<'a>(p: &'a Json, k: &str) -> &'a str {
    p.get(k).and_then(Json::as_str).unwrap_or("")
}

fn num(p: &Json, k: &str) -> Option<f64> {
    p.get(k).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))).filter(|f: &f64| f.is_finite())
}

fn input<'a>(inp: &'a dyn Inputs, p: &Json, k: &str) -> Result<&'a Dataset, String> {
    // A number is a standalone table's id, and only that; text is a name.
    if let Some(n) = p.get(k).filter(|v| v.is_number()) {
        let table = n.as_u64().and_then(|id| u32::try_from(id).ok()).and_then(|id| inp.table(id));
        return table.ok_or_else(|| format!("no standalone table with id {n}"));
    }
    let name = s(p, k);
    inp.dataset(name).ok_or_else(|| format!("no layer named '{name}'"))
}

/// Selected rows, or all rows when nothing is selected.
fn rows(inp: &dyn Inputs, p: &Json, k: &str, ds: &Dataset) -> Vec<usize> {
    let sel = inp.selection(s(p, k));
    if sel.is_empty() { (0..ds.features.len()).collect() } else { sel.iter().copied().filter(|r| *r < ds.features.len()).collect() }
}

/// The name of a tool's new layer, from its `output` parameter. Every built-in tool names its
/// output layer through here, and an `output` is only ever a layer name in the session, never a
/// file: `touches_files` in rustgis-engine relies on this to let tasks, models and notebooks from
/// files name their outputs. A built-in tool that writes a file must take its path under another
/// parameter (`path`), which stays refused.
fn out_name(p: &Json, default: String) -> String {
    let n = s(p, "output").trim();
    if n.is_empty() { default } else { n.to_string() }
}

/// Scale factor of Web Mercator at a latitude (metres on the map per metre on the ground).
fn merc_scale(lat: f64) -> f64 {
    1.0 / lat.clamp(-85.0, 85.0).to_radians().cos().max(0.01)
}

fn feature_lat(g: &Geometry) -> f64 {
    g.label_point().map(|c| c.y).unwrap_or(0.0)
}

/// A distance in `units` (one of [`UNITS`], as `resolve_params` spells it; empty for the default,
/// kilometres) in metres. Any other unit is an error, never kilometres (#406).
fn to_metres(d: f64, units: &str) -> Result<f64, String> {
    match units {
        "Meters" => Ok(d),
        "Miles" => Ok(d * 1609.344),
        "Kilometers" | "" => Ok(d * 1000.0),
        other => Err(format!("Units must be one of {} (not '{other}')", UNITS.join(", "))),
    }
}

fn result(ds: Dataset, msg: String) -> Result<ToolResult, String> {
    Ok(ToolResult { output: Output::Layer(ds), messages: vec![msg] })
}

/// A copy of a feature's attributes for an output with the same schema.
fn copy_schema(ds: &Dataset, name: String, g: GeometryType) -> Dataset {
    Dataset::new(name, g, ds.fields.clone())
}

/// Union of polygons; rings are oriented first because the overlay infers its fill rule from
/// the first ring's winding, and input files mix orientations.
fn union_all<'a>(polys: impl IntoIterator<Item = &'a geo::Polygon<f64>>) -> MultiPolygon<f64> {
    let oriented: Vec<geo::Polygon<f64>> = polys.into_iter().map(|p| p.orient(Direction::Default)).collect();
    geo::unary_union(oriented.iter())
}

// ---------------------------------------------------------------------------------------------
// tools

fn buffer(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let d = num(p, "distance").ok_or("Distance must be a number")?;
    let metres = to_metres(d, s(p, "units"))?;
    if metres.abs() > 5_000_000.0 {
        return Err("Distance is too large (limit 5000 km)".into());
    }
    let mut out = copy_schema(ds, out_name(p, format!("{}_Buffer", ds.name)), GeometryType::Polygon);
    let mut all: Vec<MultiPolygon<f64>> = Vec::new();
    for r in rows(inp, p, "input", ds) {
        let Some(f) = ds.features.get(r) else { continue };
        let Some(g) = &f.geometry else { continue };
        let Some(geom) = geodesic_buffer(g, metres) else { continue };
        if s(p, "dissolve") == "All" {
            all.extend(to_multipolygon(&geom));
        } else {
            out.push(Some(geom), f.attrs.clone());
        }
    }
    if !all.is_empty() {
        out.fields.clear();
        let u = union_all(all.iter().flat_map(|m| m.0.iter()));
        if let Some(geom) = from_multipolygon(&u) {
            out.push(Some(geom), vec![]);
        }
    }
    let n = out.features.len();
    result(out, format!("Buffer: {n} features at {d} {}", if s(p, "units").is_empty() { "Kilometers" } else { s(p, "units") }))
}

/// Union of a polygon layer's (selected) features.
fn union_of(inp: &dyn Inputs, p: &Json, k: &str) -> Result<MultiPolygon<f64>, String> {
    let ds = input(inp, p, k)?;
    if ds.geometry_type != GeometryType::Polygon {
        return Err(format!("'{}' is not a polygon layer", ds.name));
    }
    let polys: Vec<MultiPolygon<f64>> =
        rows(inp, p, k, ds).iter().filter_map(|r| ds.features.get(*r)?.geometry.as_ref()).filter_map(to_multipolygon).collect();
    Ok(union_all(polys.iter().flat_map(|m| m.0.iter())))
}

fn clip(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let mask = union_of(inp, p, "clip")?;
    let mut out = copy_schema(ds, out_name(p, format!("{}_Clip", ds.name)), ds.geometry_type);
    for r in rows(inp, p, "input", ds) {
        let Some(f) = ds.features.get(r) else { continue };
        let Some(g) = &f.geometry else { continue };
        let clipped = match g {
            Geometry::Polygon(_) => to_multipolygon(g).and_then(|m| from_multipolygon(&m.intersection(&mask))),
            Geometry::Line(_) => to_multiline(g).and_then(|m| from_line(&mask.clip(&m, false))),
            Geometry::Point(_) | Geometry::MultiPoint(_) => {
                let pts: Vec<Coord> = match g {
                    Geometry::Point(c) => vec![*c],
                    Geometry::MultiPoint(v) => v.clone(),
                    _ => vec![],
                };
                let inside: Vec<Coord> = pts
                    .into_iter()
                    .filter(|c| {
                        let m = rustgis_geom::lonlat_to_merc(*c);
                        mask.intersects(&geo::Point::new(m.x, m.y))
                    })
                    .collect();
                match inside.len() {
                    0 => None,
                    1 => inside.first().map(|c| Geometry::Point(*c)),
                    _ => Some(Geometry::MultiPoint(inside)),
                }
            }
        };
        if let Some(c) = clipped {
            out.push(Some(c), f.attrs.clone());
        }
    }
    let n = out.features.len();
    result(out, format!("Clip: {n} features kept"))
}

fn erase(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let mask = union_of(inp, p, "erase")?;
    let mut out = copy_schema(ds, out_name(p, format!("{}_Erase", ds.name)), GeometryType::Polygon);
    for r in rows(inp, p, "input", ds) {
        let Some(f) = ds.features.get(r) else { continue };
        if let Some(rest) = f.geometry.as_ref().and_then(to_multipolygon).and_then(|m| from_multipolygon(&m.difference(&mask))) {
            out.push(Some(rest), f.attrs.clone());
        }
    }
    let n = out.features.len();
    result(out, format!("Erase: {n} features remain"))
}

fn dissolve(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    if ds.geometry_type != GeometryType::Polygon {
        return Err("Dissolve needs a polygon layer".into());
    }
    let field = s(p, "field").trim();
    let fi = if field.is_empty() { None } else { Some(ds.field_index(field).ok_or_else(|| format!("no field named '{field}'"))?) };
    let mut groups: Vec<(Value, Vec<MultiPolygon<f64>>)> = Vec::new();
    for r in rows(inp, p, "input", ds) {
        let Some(mp) = ds.features.get(r).and_then(|f| f.geometry.as_ref()).and_then(to_multipolygon) else { continue };
        let key = fi.map(|fi| ds.value(r, fi).clone()).unwrap_or(Value::Null);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some(g) => g.1.push(mp),
            None => groups.push((key, vec![mp])),
        }
    }
    let fields = match fi.and_then(|i| ds.fields.get(i)) {
        Some(f) => vec![f.clone(), Field::new("count", FieldType::Integer)],
        None => vec![Field::new("count", FieldType::Integer)],
    };
    let mut out = Dataset::new(out_name(p, format!("{}_Dissolve", ds.name)), GeometryType::Polygon, fields);
    for (key, parts) in groups {
        let n = parts.len() as i64;
        let u = union_all(parts.iter().flat_map(|m| m.0.iter()));
        let attrs = if fi.is_some() { vec![key, Value::Int(n)] } else { vec![Value::Int(n)] };
        out.push(from_multipolygon(&u), attrs);
    }
    let n = out.features.len();
    result(out, format!("Dissolve: {n} features"))
}

fn convex_hull(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let each = s(p, "group") == "None";
    let mut out = if each {
        copy_schema(ds, out_name(p, format!("{}_Hull", ds.name)), GeometryType::Polygon)
    } else {
        Dataset::new(out_name(p, format!("{}_Hull", ds.name)), GeometryType::Polygon, vec![Field::new("count", FieldType::Integer)])
    };
    let mut pts: Vec<geo::Point<f64>> = Vec::new();
    for r in rows(inp, p, "input", ds) {
        let Some(f) = ds.features.get(r) else { continue };
        let Some(g) = &f.geometry else { continue };
        let mut these = Vec::new();
        g.for_each_coord(|c| {
            let m = rustgis_geom::lonlat_to_merc(c);
            these.push(geo::Point::new(m.x, m.y));
        });
        if each {
            let hull = geo::MultiPoint::new(these).convex_hull();
            out.push(from_multipolygon(&MultiPolygon::new(vec![hull])), f.attrs.clone());
        } else {
            pts.extend(these);
        }
    }
    if !each {
        let n = ds.features.len() as i64;
        let hull = geo::MultiPoint::new(pts).convex_hull();
        out.push(from_multipolygon(&MultiPolygon::new(vec![hull])), vec![Value::Int(n)]);
    }
    let n = out.features.len();
    result(out, format!("Minimum Bounding Geometry: {n} hulls"))
}

fn feature_to_point(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let mut out = copy_schema(ds, out_name(p, format!("{}_Points", ds.name)), GeometryType::Point);
    for r in rows(inp, p, "input", ds) {
        let Some(f) = ds.features.get(r) else { continue };
        let pt = match &f.geometry {
            Some(g @ Geometry::Polygon(_)) => to_multipolygon(g).and_then(|m| m.interior_point()).map(|p| from_point(&p)),
            Some(g) => g.label_point().map(Geometry::Point),
            None => None,
        };
        out.push(pt, f.attrs.clone());
    }
    let n = out.features.len();
    result(out, format!("Feature To Point: {n} points"))
}

/// A coordinate from a table cell: a number, or text holding one, written with a decimal point
/// or (when it has no point) a single decimal comma, as spreadsheets in many languages do. A comma
/// never separates thousands: `1,234` is 1.234 (refusing it would refuse `1234,567` too), and
/// `1,234.5` is not a number.
fn coordinate(v: &Value) -> Option<f64> {
    let n = match v {
        Value::Int(i) => *i as f64,
        Value::Float(f) => *f,
        Value::Text(t) => {
            let t = t.trim();
            match t.parse::<f64>() {
                Ok(n) => n,
                Err(_) if !t.contains('.') && t.matches(',').count() == 1 => t.replace(',', ".").parse().ok()?,
                Err(_) => return None,
            }
        }
        Value::Null | Value::Bool(_) => return None,
    };
    n.is_finite().then_some(n)
}

/// The rows looked at to tell whether a text field holds numbers.
const COORDINATE_SAMPLE: usize = 1000;

/// Whether a field can hold coordinates: a numeric field, or a text field where one of the
/// first rows reads as a number, as [`coordinate`] reads it. The Tools pane lists these fields
/// for a coordinate parameter, so a table without any offers none.
pub fn holds_coordinates(ds: &Dataset, field: usize) -> bool {
    match ds.fields.get(field).map(|f| f.ty) {
        Some(FieldType::Text) => (0..ds.features.len().min(COORDINATE_SAMPLE)).any(|r| coordinate(ds.value(r, field)).is_some()),
        Some(ty) => ty.is_numeric(),
        None => false,
    }
}

fn xy_table_to_point(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let field = |k: &str| {
        let name = s(p, k).trim();
        ds.field_index(name).ok_or_else(|| format!("no field named '{name}'"))
    };
    let (xi, yi) = (field("x")?, field("y")?);
    let crs = match s(p, "crs").trim() {
        "" => Crs::LonLat,
        t => Crs::parse(t)?,
    };
    let to = crs.transformer()?;
    let mut out = copy_schema(ds, out_name(p, format!("{}_Points", ds.name)), GeometryType::Point);
    // Rows with numbers in both fields, and those of them that land on the globe.
    let (mut read, mut placed, mut total) = (0usize, 0usize, 0usize);
    for r in rows(inp, p, "input", ds) {
        let Some(f) = ds.features.get(r) else { continue };
        total += 1;
        let xy = coordinate(ds.value(r, xi)).zip(coordinate(ds.value(r, yi)));
        read += usize::from(xy.is_some());
        let pt = xy.and_then(|(x, y)| to.to_lonlat(Coord::new(x, y))).filter(|c| c.is_finite() && c.x.abs() <= 180.0 && c.y.abs() <= 90.0);
        placed += usize::from(pt.is_some());
        // A row without a usable position stays in the layer's table, without a point.
        out.push_next(pt.map(Geometry::Point), f.attrs.clone());
    }
    if placed == 0 {
        return Err(if read == 0 {
            format!("{}: no row has numbers in both '{}' and '{}'", ds.name, s(p, "x").trim(), s(p, "y").trim())
        } else {
            format!(
                "{}: the coordinates aren't in {}: give their coordinate system (\"crs\", such as EPSG:2263) to place them",
                ds.name,
                crs.label()
            )
        });
    }
    let mut messages = vec![format!("XY Table To Point: {placed} points")];
    if placed < total {
        messages.push(format!("{} of {total} rows have no usable coordinates; they are in the table without a point", total - placed));
    }
    Ok(ToolResult { output: Output::Layer(out), messages })
}

/// A geometry simplified (Douglas–Peucker) to a tolerance in metres.
pub fn simplify_geometry(g: &Geometry, tol_m: f64) -> Geometry {
    let eps = tol_m * merc_scale(feature_lat(g));
    let sg = match g {
        Geometry::Polygon(_) => to_multipolygon(g).and_then(|m| from_multipolygon(&m.simplify(eps))),
        Geometry::Line(_) => to_multiline(g).and_then(|m| from_line(&m.simplify(eps))),
        _ => None,
    };
    sg.unwrap_or_else(|| g.clone())
}

fn simplify(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let tol = num(p, "tolerance").ok_or("Tolerance must be a number")?.abs() * 1000.0;
    let mut out = copy_schema(ds, out_name(p, format!("{}_Simplified", ds.name)), ds.geometry_type);
    let (mut before, mut after) = (0usize, 0usize);
    for r in rows(inp, p, "input", ds) {
        let Some(f) = ds.features.get(r) else { continue };
        let Some(g) = &f.geometry else { continue };
        before += g.coord_count();
        let eps = tol * merc_scale(feature_lat(g));
        let sg = match g {
            Geometry::Polygon(_) => to_multipolygon(g).and_then(|m| from_multipolygon(&m.simplify(eps))),
            Geometry::Line(_) => to_multiline(g).and_then(|m| from_line(&m.simplify(eps))),
            other => Some(other.clone()),
        };
        after += sg.as_ref().map(|g| g.coord_count()).unwrap_or(0);
        out.push(sg, f.attrs.clone());
    }
    result(out, format!("Simplify: {before} → {after} vertices"))
}

fn select_by_location(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let sel_ds = input(inp, p, "select")?;
    let dist = num(p, "distance").unwrap_or(0.0).max(0.0) * 1000.0;
    if dist > 5_000_000.0 {
        return Err("Distance is too large (limit 5000 km)".into());
    }
    let selectors: Vec<geo::Geometry<f64>> = rows(inp, p, "select", sel_ds)
        .iter()
        .filter_map(|r| sel_ds.features.get(*r)?.geometry.as_ref())
        .map(|g| {
            let b = if dist > 0.0 { geodesic_buffer(g, dist) } else { None };
            to_geo(b.as_ref().unwrap_or(g))
        })
        .collect();
    let invert = s(p, "invert") == "Yes";
    let hits: Vec<usize> = (0..ds.features.len())
        .filter(|r| {
            let Some(g) = ds.features.get(*r).and_then(|f| f.geometry.as_ref()) else {
                return false;
            };
            let gg = to_geo(g);
            let hit = selectors.iter().any(|s| s.intersects(&gg));
            hit != invert
        })
        .collect();
    let n = hits.len();
    Ok(ToolResult {
        output: Output::Selection { layer: ds.name.clone(), rows: hits },
        messages: vec![format!("Selected {n} features in {}", ds.name)],
    })
}

fn select_by_attributes(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let hits = rustgis_data::query::select(s(p, "where"), ds)?;
    let n = hits.len();
    Ok(ToolResult {
        output: Output::Selection { layer: ds.name.clone(), rows: hits },
        messages: vec![format!("Selected {n} features in {}", ds.name)],
    })
}

/// The QGIS IDW interpolation: every point weighs on every cell.
fn idw_interpolation(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let field = s(p, "field");
    let fi = ds.field_index(field).ok_or_else(|| format!("no field named '{field}'"))?;
    let mut pts: Vec<(Coord, f64)> = Vec::new();
    for r in rows(inp, p, "input", ds) {
        let Some(g) = ds.features.get(r).and_then(|f| f.geometry.as_ref()) else { continue };
        let Some(v) = ds.value(r, fi).as_f64().filter(|v| v.is_finite()) else { continue };
        g.for_each_coord(|c| {
            if c.is_finite() {
                pts.push((c, v));
            }
        });
    }
    let extent = match s(p, "extent").trim() {
        "" => {
            let mut e = Extent::EMPTY;
            pts.iter().for_each(|(c, _)| e.expand(*c));
            // Points in a row span no area: give the grid some.
            if e.width() > 0.0 && e.height() > 0.0 { e } else { e.padded(0.05, 0.01) }
        }
        t => {
            // Every component must parse: dropping a bad one would shift the rest.
            let v: Option<Vec<f64>> = t.split(',').map(|x| x.trim().parse().ok().filter(|x: &f64| x.is_finite())).collect();
            let Some([x0, y0, x1, y1]) = v.as_deref().and_then(|v| <[f64; 4]>::try_from(v).ok()) else {
                return Err("the extent is xmin,ymin,xmax,ymax".into());
            };
            Extent::new(x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
        }
    };
    // Defaults only when a value is not given: a bad one is refused, not replaced.
    let given = |k: &str| p.get(k).is_some_and(|v| !v.is_null() && v.as_str().is_none_or(|t| !t.trim().is_empty()));
    let cell = match num(p, "pixelSize") {
        Some(c) if c > 0.0 => c,
        _ if given("pixelSize") => return Err("the pixel size must be more than zero".into()),
        _ => idw_auto_cell(&extent, pts.len()),
    };
    let power = match num(p, "power") {
        Some(v) => v,
        None if given("power") => return Err("the distance coefficient must be a number".into()),
        None => 2.0,
    };
    let name = out_name(p, format!("{}_{field}_idw", ds.name));
    let r = crate::rasterops::idw(&pts, &extent, cell, power, 0, &name)?;
    let bytes = rustgis_data::raster::geotiff::write(&r, 0).map_err(|e| e.to_string())?;
    let msg = format!("IDW Interpolation: {} × {} cells from {} points", r.width, r.height, pts.len());
    Ok(ToolResult {
        output: Output::Layers(vec![OutputLayer::Raster { file_name: format!("{name}.tif"), name, bytes }]),
        messages: vec![msg],
    })
}

/// The automatic IDW cell size: 300 cells across the extent, coarsened until every cell weighing
/// every one of `n` points stays within the interpolation's work limit.
fn idw_auto_cell(extent: &Extent, n: usize) -> f64 {
    let mut cell = (extent.width().max(extent.height()) / 300.0).max(1e-5);
    let cells = |c: f64| ((extent.width() / c).ceil().max(1.0) * (extent.height() / c).ceil().max(1.0)) as usize;
    for _ in 0..200 {
        if cells(cell).saturating_mul(n) <= crate::rasterops::MAX_IDW_WORK {
            break;
        }
        cell *= 1.1;
    }
    cell
}

/// Ordinary Kriging interpolation: builds prediction and error surfaces from spatial autocorrelation.
fn kriging_interpolation(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    if let Some(crs) = ds.crs
        && crs != rustgis_data::raster::Crs::LonLat
    {
        return Err("kriging interpolation expects coordinates in degrees (WGS 84 / EPSG:4326)".into());
    }
    let field = s(p, "field");
    let fi = ds.field_index(field).ok_or_else(|| format!("no field named '{field}'"))?;
    let mut raw_pts: Vec<(Coord, f64)> = Vec::new();
    for r in rows(inp, p, "input", ds) {
        let Some(g) = ds.features.get(r).and_then(|f| f.geometry.as_ref()) else { continue };
        let Some(v) = ds.value(r, fi).as_f64().filter(|v| v.is_finite()) else { continue };
        match g {
            Geometry::Point(c) => {
                if c.is_finite() {
                    raw_pts.push((*c, v));
                }
            }
            Geometry::MultiPoint(cs) => {
                for c in cs {
                    if c.is_finite() {
                        raw_pts.push((*c, v));
                    }
                }
            }
            Geometry::Line(_) | Geometry::Polygon(_) => {
                let ext = g.extent();
                if !ext.is_empty() {
                    let center = Coord::new((ext.min_x + ext.max_x) * 0.5, (ext.min_y + ext.max_y) * 0.5);
                    if center.is_finite() {
                        raw_pts.push((center, v));
                    }
                }
            }
        }
    }
    // Deduplicate sample points by coordinate to prevent singular kriging matrix
    raw_pts.sort_by(|a, b| a.0.x.total_cmp(&b.0.x).then_with(|| a.0.y.total_cmp(&b.0.y)));
    let mut accum: Vec<(Coord, f64, usize)> = Vec::with_capacity(raw_pts.len());
    for (c, v) in raw_pts {
        let mut duplicate_idx = None;
        for (i, (last_c, _, _)) in accum.iter().enumerate().rev() {
            if c.x - last_c.x >= 1e-9 {
                break;
            }
            if (c.y - last_c.y).abs() < 1e-9 {
                duplicate_idx = Some(i);
                break;
            }
        }
        if let Some(i) = duplicate_idx {
            if let Some((_, sum, count)) = accum.get_mut(i) {
                *sum += v;
                *count += 1;
            }
            continue;
        }
        accum.push((c, v, 1));
    }
    let pts: Vec<(Coord, f64)> = accum.into_iter().map(|(c, sum, count)| (c, sum / count as f64)).collect();
    if pts.len() < 2 {
        return Err("kriging needs at least two points with values".into());
    }

    let extent = match s(p, "extent").trim() {
        "" => {
            let mut e = Extent::EMPTY;
            pts.iter().for_each(|(c, _)| e.expand(*c));
            if e.width() > 0.0 && e.height() > 0.0 { e } else { e.padded(0.05, 0.01) }
        }
        t => {
            let v: Option<Vec<f64>> = t.split(',').map(|x| x.trim().parse().ok().filter(|x: &f64| x.is_finite())).collect();
            let Some([x0, y0, x1, y1]) = v.as_deref().and_then(|v| <[f64; 4]>::try_from(v).ok()) else {
                return Err("the extent is xmin,ymin,xmax,ymax".into());
            };
            Extent::new(x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
        }
    };
    let given = |k: &str| p.get(k).is_some_and(|v| !v.is_null() && v.as_str().is_none_or(|t| !t.trim().is_empty()));
    let neighbours = match num(p, "neighbours") {
        Some(n) if n >= 2.0 => (n as usize).min(32),
        Some(_) => return Err("neighbours must be at least 2".into()),
        None if given("neighbours") => return Err("neighbours must be a number".into()),
        None => 12,
    };
    let cell = match num(p, "pixelSize") {
        Some(c) if c > 0.0 => c,
        _ if given("pixelSize") => return Err("the pixel size must be more than zero".into()),
        _ => kriging_auto_cell(&extent, pts.len(), neighbours),
    };
    let model = match s(p, "model") {
        "" => crate::kriging::VariogramModel::Spherical,
        m => m.parse::<crate::kriging::VariogramModel>().map_err(|_| format!("unknown variogram model '{m}'"))?,
    };
    let mut params = crate::kriging::auto_variogram(&pts, &extent, model);
    match num(p, "nugget") {
        Some(v) if v >= 0.0 => params.nugget = v,
        Some(_) => return Err("nugget must be a non-negative number".into()),
        None if given("nugget") => return Err("nugget must be a number".into()),
        None => {}
    }
    match num(p, "sill") {
        Some(v) if v > 0.0 => params.sill = v,
        Some(_) => return Err("sill must be a positive number".into()),
        None if given("sill") => return Err("sill must be a number".into()),
        None => {}
    }
    match num(p, "range") {
        Some(v) if v > 0.0 => params.range = v,
        Some(_) => return Err("range must be a positive number".into()),
        None if given("range") => return Err("range must be a number".into()),
        None => {}
    }

    let name = out_name(p, format!("{}_{field}_kriging", ds.name));
    let r = crate::kriging::kriging(&pts, &extent, cell, &params, neighbours, &name)?;
    let pred_bytes = rustgis_data::raster::geotiff::write(&r, 0).map_err(|e| e.to_string())?;
    let var_bytes = rustgis_data::raster::geotiff::write(&r, 1).map_err(|e| e.to_string())?;
    let var_name = format!("{name}_variance");
    let msg = format!("Ordinary Kriging ({}): {} × {} cells from {} points", model.as_str(), r.width, r.height, pts.len());
    Ok(ToolResult {
        output: Output::Layers(vec![
            OutputLayer::Raster { file_name: format!("{name}.tif"), name, bytes: pred_bytes },
            OutputLayer::Raster { file_name: format!("{var_name}.tif"), name: var_name, bytes: var_bytes },
        ]),
        messages: vec![msg],
    })
}

fn kriging_auto_cell(extent: &Extent, n: usize, neighbours: usize) -> f64 {
    let k = if neighbours == 0 { 12.min(n) } else { neighbours.clamp(2, n.clamp(2, 32)) };
    let work_per_cell = n.saturating_add((k * k * k) / 3);
    let mut cell = (extent.width().max(extent.height()) / 250.0).max(1e-5);
    let cells = |c: f64| ((extent.width() / c).ceil().max(1.0) * (extent.height() / c).ceil().max(1.0)) as usize;
    for _ in 0..200 {
        if cells(cell).saturating_mul(work_per_cell) <= crate::kriging::MAX_KRIGING_WORK {
            break;
        }
        cell *= 1.1;
    }
    cell
}

fn count_points(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let polys = input(inp, p, "polygons")?;
    let pts = input(inp, p, "points")?;
    let point_geoms: Vec<geo::Geometry<f64>> =
        rows(inp, p, "points", pts).iter().filter_map(|r| pts.features.get(*r)?.geometry.as_ref()).map(to_geo).collect();
    let mut out = polys.clone();
    out.name = out_name(p, format!("{}_Summarize", polys.name));
    out.source = None;
    let fi = out.ensure_field("point_count", FieldType::Integer);
    let mut total = 0i64;
    for f in &mut out.features {
        let n = match f.geometry.as_ref().and_then(to_multipolygon) {
            Some(mp) => point_geoms.iter().filter(|g| mp.intersects(*g)).count() as i64,
            None => 0,
        };
        total += n;
        if let Some(v) = f.attrs.get_mut(fi) {
            *v = Value::Int(n);
        }
    }
    result(out, format!("Summarize Within: {total} points counted"))
}

fn calculate_geometry(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let mut out = ds.clone();
    match ds.geometry_type {
        GeometryType::Polygon => {
            let fa = out.ensure_field("area_km2", FieldType::Double);
            let fp = out.ensure_field("perimeter_km", FieldType::Double);
            for f in &mut out.features {
                let (mut a, mut per) = (0.0, 0.0);
                if let Some(Geometry::Polygon(polys)) = &f.geometry {
                    for rings in polys {
                        for (i, r) in rings.iter().enumerate() {
                            let ra = geodesic_ring_area_m2(r);
                            a += if i == 0 { ra } else { -ra };
                            per += geodesic_length_m(r);
                        }
                    }
                }
                if let Some(v) = f.attrs.get_mut(fa) {
                    *v = Value::Float(a / 1e6);
                }
                if let Some(v) = f.attrs.get_mut(fp) {
                    *v = Value::Float(per / 1e3);
                }
            }
        }
        GeometryType::Polyline => {
            let fl = out.ensure_field("length_km", FieldType::Double);
            for f in &mut out.features {
                let len: f64 = match &f.geometry {
                    Some(Geometry::Line(parts)) => parts.iter().map(|p| geodesic_length_m(p)).sum(),
                    _ => 0.0,
                };
                if let Some(v) = f.attrs.get_mut(fl) {
                    *v = Value::Float(len / 1e3);
                }
            }
        }
        _ => {
            let fx = out.ensure_field("point_x", FieldType::Double);
            let fy = out.ensure_field("point_y", FieldType::Double);
            for f in &mut out.features {
                let c = f.geometry.as_ref().and_then(|g| g.label_point());
                if let Some(v) = f.attrs.get_mut(fx) {
                    *v = c.map(|c| Value::Float(c.x)).unwrap_or(Value::Null);
                }
                if let Some(v) = f.attrs.get_mut(fy) {
                    *v = c.map(|c| Value::Float(c.y)).unwrap_or(Value::Null);
                }
            }
        }
    }
    Ok(ToolResult {
        output: Output::Update { layer: ds.name.clone(), dataset: out },
        messages: vec![format!("Calculated geometry attributes for {}", ds.name)],
    })
}

fn summary_statistics(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let field = s(p, "field");
    let fi = ds.field_index(field).ok_or_else(|| format!("no field named '{field}'"))?;
    let case = s(p, "case").trim();
    let ci = if case.is_empty() { None } else { Some(ds.field_index(case).ok_or_else(|| format!("no field named '{case}'"))?) };
    let mut groups: Vec<(Value, Vec<f64>)> = Vec::new();
    for r in rows(inp, p, "input", ds) {
        let key = ci.map(|c| ds.value(r, c).clone()).unwrap_or(Value::Null);
        let v = ds.value(r, fi).as_f64();
        let g = match groups.iter_mut().position(|(k, _)| *k == key) {
            Some(i) => i,
            None => {
                groups.push((key, Vec::new()));
                groups.len() - 1
            }
        };
        if let (Some(v), Some(g)) = (v, groups.get_mut(g)) {
            g.1.push(v);
        }
    }
    let fname = ds.fields.get(fi).map(|f| f.name.clone()).unwrap_or_default();
    let mut fields = Vec::new();
    if let Some(c) = ci.and_then(|c| ds.fields.get(c)) {
        fields.push(c.clone());
    }
    for st in ["COUNT", "SUM", "MEAN", "MIN", "MAX", "STD"] {
        fields.push(Field::new(format!("{st}_{fname}"), if st == "COUNT" { FieldType::Integer } else { FieldType::Double }));
    }
    let mut out = Dataset::new(format!("{}_Statistics", ds.name), GeometryType::None, fields);
    groups.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    for (key, v) in groups {
        let n = v.len() as f64;
        let sum: f64 = v.iter().sum();
        let mean = if n > 0.0 { sum / n } else { 0.0 };
        let sd = if n > 0.0 { (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt() } else { 0.0 };
        let min = v.iter().copied().fold(f64::INFINITY, f64::min);
        let max = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let mut row = Vec::new();
        if ci.is_some() {
            row.push(key);
        }
        let f = |x: f64| if x.is_finite() { Value::Float(x) } else { Value::Null };
        row.extend([Value::Int(n as i64), f(sum), f(mean), f(min), f(max), f(sd)]);
        out.push(None, row);
    }
    let n = out.features.len();
    Ok(ToolResult { output: Output::Table(out), messages: vec![format!("Summary Statistics: {n} rows")] })
}

fn export_features(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let ds = input(inp, p, "input")?;
    let mut rs = rows(inp, p, "input", ds);
    let w = s(p, "where").trim();
    if !w.is_empty() {
        let e = rustgis_data::query::parse(w, ds)?;
        rs.retain(|r| rustgis_data::query::eval(&e, ds, *r));
    }
    let mut out = copy_schema(ds, out_name(p, format!("{}_Export", ds.name)), ds.geometry_type);
    for r in rs {
        if let Some(f) = ds.features.get(r) {
            out.push(f.geometry.clone(), f.attrs.clone());
        }
    }
    let n = out.features.len();
    result(out, format!("Export Features: {n} features"))
}

fn extent_contains(a: &Extent, b: &Extent) -> bool {
    !a.is_empty() && !b.is_empty() && a.min_x <= b.min_x && a.max_x >= b.max_x && a.min_y <= b.min_y && a.max_y >= b.max_y
}

fn spatial_join(inp: &dyn Inputs, p: &Json) -> Result<ToolResult, String> {
    let target = input(inp, p, "target")?;
    let join = input(inp, p, "join")?;

    if let (Some(tc), Some(jc)) = (target.crs, join.crs)
        && tc != jc
    {
        return Err("the target and join layers must have the same coordinate reference system".into());
    }

    let match_opt = match s(p, "match") {
        "Contains" => "Contains",
        "Within" => "Within",
        _ => "Intersects",
    };

    let op_many = s(p, "operation").eq_ignore_ascii_case("Join one to many");
    let keep_all = !s(p, "keepAll").eq_ignore_ascii_case("Keep only matching");

    let prefix_raw = s(p, "prefix");
    let prefix = if prefix_raw.is_empty() && p.get("prefix").is_none() { "join_" } else { prefix_raw };

    let target_rows = rows(inp, p, "target", target);
    let join_rows = rows(inp, p, "join", join);

    let mut join_geoms = Vec::with_capacity(join_rows.len());
    for &jr in &join_rows {
        if let Some(f) = join.features.get(jr)
            && let Some(g) = f.geometry.as_ref()
        {
            join_geoms.push((jr, g.extent(), to_geo(g)));
        }
    }

    let mut out = copy_schema(target, out_name(p, format!("{}_SpatialJoin", target.name)), target.geometry_type);
    out.source = None;
    out.crs = target.crs;

    let count_field_name = {
        let base = "join_count";
        let mut name = base.to_string();
        let mut k = 1usize;
        while out.field_index(&name).is_some() {
            k += 1;
            name = format!("{base}_{k}");
        }
        name
    };
    out.fields.push(Field::new(count_field_name, FieldType::Integer));

    let mut join_field_indices = Vec::with_capacity(join.fields.len());
    for (orig_idx, jf) in join.fields.iter().enumerate() {
        let raw = if prefix.is_empty() { jf.name.clone() } else { format!("{prefix}{}", jf.name) };
        let mut name = raw.clone();
        let mut k = 1usize;
        while out.field_index(&name).is_some() {
            k += 1;
            name = format!("{raw}_{k}");
        }
        out.fields.push(Field::new(name, jf.ty));
        join_field_indices.push(orig_idx);
    }

    let target_fields_len = target.fields.len();
    let join_fields_len = join.fields.len();

    // Spatial grid index over join geometries for efficient candidate pruning
    let mut total_ext = Extent::EMPTY;
    for (_, ext, _) in &join_geoms {
        total_ext = total_ext.union(ext);
    }
    let mut use_grid = join_geoms.len() > 16 && !total_ext.is_empty();
    let n_cells = (join_geoms.len() as f64).sqrt().clamp(4.0, 256.0);
    let cell_w = (total_ext.width() / n_cells).max(1e-9);
    let cell_h = (total_ext.height() / n_cells).max(1e-9);
    let key = |x: f64, y: f64| -> (i64, i64) {
        let kx = if x.is_finite() { ((x - total_ext.min_x) / cell_w).floor() } else { 0.0 };
        let ky = if y.is_finite() { ((y - total_ext.min_y) / cell_h).floor() } else { 0.0 };
        (kx.clamp(0.0, 512.0) as i64, ky.clamp(0.0, 512.0) as i64)
    };

    const MAX_GRID_REFERENCES: usize = 1_000_000;
    if use_grid {
        let mut projected_refs = 0usize;
        for (_, j_ext, _) in &join_geoms {
            let (gx0, gy0) = key(j_ext.min_x, j_ext.min_y);
            let (gx1, gy1) = key(j_ext.max_x, j_ext.max_y);
            let cells = ((gx1 - gx0 + 1) as usize).saturating_mul((gy1 - gy0 + 1) as usize);
            projected_refs = projected_refs.saturating_add(cells);
            if projected_refs > MAX_GRID_REFERENCES {
                use_grid = false;
                break;
            }
        }
    }

    let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> = std::collections::HashMap::new();
    if use_grid {
        for (idx, (_, j_ext, _)) in join_geoms.iter().enumerate() {
            let (gx0, gy0) = key(j_ext.min_x, j_ext.min_y);
            let (gx1, gy1) = key(j_ext.max_x, j_ext.max_y);
            for gx in gx0..=gx1 {
                for gy in gy0..=gy1 {
                    grid.entry((gx, gy)).or_default().push(idx);
                }
            }
        }
    }

    let hit = |t_ext: &Extent, t_geo: &geo::Geometry<f64>, j_ext: &Extent, j_geo: &geo::Geometry<f64>| -> bool {
        match match_opt {
            "Contains" => extent_contains(t_ext, j_ext) && Contains::contains(t_geo, j_geo),
            "Within" => extent_contains(j_ext, t_ext) && Contains::contains(j_geo, t_geo),
            _ => t_ext.intersects(j_ext) && t_geo.intersects(j_geo),
        }
    };

    let mut total_matches = 0usize;
    let mut written_features = 0usize;

    for tr in target_rows {
        let Some(tf) = target.features.get(tr) else { continue };
        let mut matches: Vec<usize> = Vec::new();

        if let Some(tg) = tf.geometry.as_ref() {
            let t_ext = tg.extent();
            let t_geo = to_geo(tg);

            if use_grid {
                if t_ext.intersects(&total_ext) {
                    let (tx0, ty0) = key(t_ext.min_x, t_ext.min_y);
                    let (tx1, ty1) = key(t_ext.max_x, t_ext.max_y);
                    let mut candidates = Vec::new();
                    for gx in tx0..=tx1 {
                        for gy in ty0..=ty1 {
                            if let Some(indices) = grid.get(&(gx, gy)) {
                                candidates.extend_from_slice(indices);
                            }
                        }
                    }
                    candidates.sort_unstable();
                    candidates.dedup();

                    for idx in candidates {
                        if let Some((jr, j_ext, j_geo)) = join_geoms.get(idx)
                            && hit(&t_ext, &t_geo, j_ext, j_geo)
                        {
                            matches.push(*jr);
                        }
                    }
                }
            } else {
                for (jr, j_ext, j_geo) in &join_geoms {
                    if hit(&t_ext, &t_geo, j_ext, j_geo) {
                        matches.push(*jr);
                    }
                }
            }
        }

        let m_count = matches.len();
        total_matches += m_count;

        let mut push_row = |count: i64, jf: Option<&rustgis_data::Feature>| {
            let mut attrs = Vec::with_capacity(out.fields.len());
            for i in 0..target_fields_len {
                attrs.push(tf.attrs.get(i).cloned().unwrap_or(Value::Null));
            }
            attrs.push(Value::Int(count));
            if let Some(f) = jf {
                for &ji in &join_field_indices {
                    attrs.push(f.attrs.get(ji).cloned().unwrap_or(Value::Null));
                }
            } else {
                attrs.resize(attrs.len() + join_fields_len, Value::Null);
            }
            out.push_next(tf.geometry.clone(), attrs);
            written_features += 1;
        };

        if m_count == 0 {
            if keep_all {
                push_row(0, None);
            }
        } else if op_many {
            for &jr in &matches {
                push_row(m_count as i64, join.features.get(jr));
            }
        } else {
            let jf = matches.first().and_then(|&jr| join.features.get(jr));
            push_row(m_count as i64, jf);
        }
    }

    result(out, format!("Spatial Join: {written_features} features joined ({total_matches} matches)"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Map(HashMap<String, Dataset>);
    impl Inputs for Map {
        fn dataset(&self, layer: &str) -> Option<&Dataset> {
            self.0.get(layer)
        }
        fn selection(&self, _: &str) -> &[usize] {
            &[]
        }
    }

    fn sq(x: f64, y: f64, s: f64) -> Geometry {
        Geometry::Polygon(vec![vec![vec![
            Coord::new(x, y),
            Coord::new(x + s, y),
            Coord::new(x + s, y + s),
            Coord::new(x, y + s),
            Coord::new(x, y),
        ]]])
    }

    fn inputs() -> Map {
        let mut polys = Dataset::new("polys", GeometryType::Polygon, vec![Field::new("k", FieldType::Text)]);
        polys.push(Some(sq(0.0, 0.0, 2.0)), vec![Value::Text("a".into())]);
        polys.push(Some(sq(1.0, 0.0, 2.0)), vec![Value::Text("a".into())]);
        polys.push(Some(sq(10.0, 10.0, 1.0)), vec![Value::Text("b".into())]);
        let mut pts = Dataset::new("pts", GeometryType::Point, vec![Field::new("v", FieldType::Double)]);
        for (x, y, v) in [(0.5, 0.5, 1.0), (1.5, 1.5, 2.0), (50.0, 50.0, 3.0)] {
            pts.push(Some(Geometry::Point(Coord::new(x, y))), vec![Value::Float(v)]);
        }
        Map(HashMap::from([("polys".to_string(), polys), ("pts".to_string(), pts)]))
    }

    fn layer(r: ToolResult) -> Dataset {
        match r.output {
            Output::Layer(d) | Output::Table(d) | Output::Update { dataset: d, .. } => d,
            Output::Selection { .. } | Output::Layers(_) => panic!("expected a dataset"),
        }
    }

    #[test]
    fn all_tools_run() {
        let m = inputs();
        let j = |v: serde_json::Value| v;
        let b = layer(crate::run("buffer", &m, &j(serde_json::json!({"input": "pts", "distance": 50}))).unwrap());
        assert_eq!(b.features.len(), 3);
        let a = b.features[0].geometry.as_ref().unwrap().extent();
        assert!((a.width() - 0.9).abs() < 0.1, "≈100 km wide at the equator: {}", a.width());
        let d = layer(crate::run("dissolve", &m, &j(serde_json::json!({"input": "polys", "field": "k"}))).unwrap());
        assert_eq!(d.features.len(), 2);
        let c = layer(crate::run("clip", &m, &j(serde_json::json!({"input": "pts", "clip": "polys"}))).unwrap());
        assert_eq!(c.features.len(), 2);
        let cnt = layer(crate::run("countPoints", &m, &j(serde_json::json!({"polygons": "polys", "points": "pts"}))).unwrap());
        assert_eq!(cnt.value(0, 1), &Value::Int(2));
        let st = layer(crate::run("summaryStatistics", &m, &j(serde_json::json!({"input": "pts", "field": "v"}))).unwrap());
        assert_eq!(st.value(0, 2), &Value::Float(2.0));
        match crate::run("selectByLocation", &m, &j(serde_json::json!({"input": "pts", "select": "polys"}))).unwrap().output {
            Output::Selection { rows, .. } => assert_eq!(rows, vec![0, 1]),
            _ => panic!(),
        }
        let sj = layer(crate::run("spatialJoin", &m, &j(serde_json::json!({"target": "pts", "join": "polys"}))).unwrap());
        assert_eq!(sj.features.len(), 3);
        assert_eq!(sj.value(0, sj.field_index("join_count").unwrap()), &Value::Int(1));
        assert_eq!(sj.value(0, sj.field_index("join_k").unwrap()), &Value::Text("a".into()));
        assert_eq!(sj.value(2, sj.field_index("join_count").unwrap()), &Value::Int(0));
        for id in ["convexHull", "featureToPoint", "simplify", "calculateGeometry", "exportFeatures", "erase"] {
            let r = crate::run(id, &m, &j(serde_json::json!({"input": "polys", "erase": "polys"})));
            assert!(r.is_ok(), "{id}: {r:?}");
        }
    }

    /// Calculate Geometry stores full-precision values (#513): rounding to 0.01 km² gave every
    /// polygon under half a hectare an area of 0, and lines under 5 m a length of 0.
    #[test]
    fn calculate_geometry_keeps_small_values() {
        // A square of about 50 m (0.00045°) at the equator, and one of its sides as a line.
        let s = 0.00045;
        let mut polys = Dataset::new("p", GeometryType::Polygon, vec![]);
        polys.push(Some(sq(0.0, 0.0, s)), vec![]);
        let mut lines = Dataset::new("l", GeometryType::Polyline, vec![]);
        lines.push(Some(Geometry::Line(vec![vec![Coord::new(0.0, 0.0), Coord::new(0.00002, 0.0)]])), vec![]);
        let m = Map(HashMap::from([("p".to_string(), polys), ("l".to_string(), lines)]));
        let p = layer(crate::run("calculateGeometry", &m, &serde_json::json!({"input": "p"})).unwrap());
        let area = p.value(0, p.field_index("area_km2").unwrap()).as_f64().unwrap();
        let per = p.value(0, p.field_index("perimeter_km").unwrap()).as_f64().unwrap();
        assert!((area - 0.0025).abs() < 0.0001, "≈ 50 m × 50 m: {area}");
        assert!((per - 0.2).abs() < 0.001 && per != 0.2, "≈ 200 m, unrounded: {per}");
        let l = layer(crate::run("calculateGeometry", &m, &serde_json::json!({"input": "l"})).unwrap());
        let len = l.value(0, l.field_index("length_km").unwrap()).as_f64().unwrap();
        assert!((len - 0.00222).abs() < 0.00001, "≈ 2.2 m: {len}");
    }

    /// A choice outside the declared ones is an error naming them, never a guess (#406): "meter"
    /// was a buffer in kilometres. A choice in another case is the declared one.
    #[test]
    fn choices_outside_the_list_are_errors() {
        let mut pts = Dataset::new("p", GeometryType::Point, vec![]);
        pts.push(Some(Geometry::Point(Coord::new(-105.9378, 35.687))), vec![]);
        let m = Map(HashMap::from([("p".to_string(), pts)]));
        let buffer =
            |units: serde_json::Value| crate::run("buffer", &m, &serde_json::json!({"input": "p", "distance": 500, "units": units}));
        for bad in ["meter", "Meter", "feet", "m", "km"] {
            let e = buffer(serde_json::json!(bad)).err().unwrap_or_default();
            assert!(e.contains("Kilometers, Meters, Miles") && e.contains(bad), "{bad}: {e}");
        }
        assert!(buffer(serde_json::json!(5)).is_err());
        // "meters" is "Meters": a 500 m radius (about 0.0045° of latitude), named as declared.
        let r = buffer(serde_json::json!("meters")).unwrap();
        assert_eq!(r.messages, vec!["Buffer: 1 features at 500 Meters".to_string()]);
        let e = layer(r).extent();
        assert!((e.max_y - e.min_y - 1000.0 / 111_000.0).abs() < 0.0005, "{e:?}");
        assert!(buffer(serde_json::Value::Null).is_ok(), "no units: the default");
        assert_eq!(layer(buffer(serde_json::json!("  ")).unwrap()).name, "p_Buffer", "blank units: the default");
        // An external tool's choices (GeoLibre's, a plugin's) aren't checked here: its runner does.
        crate::register_external(
            "choicetest",
            || {
                vec![ToolSpec {
                    id: "choicetest:list",
                    name: "Choice Test",
                    toolbox: "Test",
                    summary: "",
                    params: &[P {
                        name: "features",
                        label: "Features",
                        kind: K::Choice(&["contrast", "energy"]),
                        required: false,
                        default: "",
                        help: "",
                    }],
                    run: crate::external_placeholder,
                }]
            },
            |_, _, _| Err("not run".into()),
        );
        let external = crate::external_tools_of("choicetest").first().unwrap();
        let list = crate::resolve_params(external, &serde_json::json!({"features": "contrast,energy"})).unwrap();
        assert_eq!(list["features"], "contrast,energy");
        // A copy of a built-in spec is still checked.
        let b = crate::tool("buffer").unwrap();
        let copy = ToolSpec { id: b.id, name: b.name, toolbox: b.toolbox, summary: b.summary, params: b.params, run: b.run };
        assert!(crate::resolve_params(&copy, &serde_json::json!({"input": "p", "distance": 1, "units": "meter"})).is_err());
        assert!(crate::run("buffer", &m, &serde_json::json!({"input": "p", "distance": 1, "dissolve": "everything"})).is_err());
        // Every built-in choice defaults to one of its own choices.
        for t in TOOLS {
            for p in t.params {
                if let K::Choice(c) = p.kind
                    && let Ok(serde_json::Value::String(d)) = serde_json::from_str::<serde_json::Value>(p.default)
                {
                    assert!(c.contains(&d.as_str()), "{}: {} defaults to {d:?}", t.id, p.name);
                }
            }
        }
    }

    /// `touches_files` in rustgis-engine lets tasks, models and notebooks from files set a built-in
    /// tool's `output`, because it only names the new layer (`out_name`). Hold every built-in tool
    /// to that: an `output` parameter is plain text, a run names its layer after it, and no tool
    /// here writes a file at all (it hands back layers; the engine adds them), so `output` can't
    /// be made a place by a tool's body either.
    #[test]
    fn a_built_in_tools_output_only_names_its_layer() {
        for spec in TOOLS {
            for p in spec.params.iter().filter(|p| p.name.eq_ignore_ascii_case("output")) {
                assert!(matches!(p.kind, K::Text), "{}: output must be a layer name (Text), not {:?}", spec.id, p.kind);
            }
        }
        let m = inputs();
        let r = crate::run("buffer", &m, &serde_json::json!({"input": "pts", "distance": 1, "output": "Named"})).unwrap();
        assert_eq!(layer(r).name, "Named");
        // A best-effort tripwire, not a proof: the crate's code (every file under src/, up to its
        // test module) names nothing that reaches the file system, a path type included (a tool
        // here takes layers, not paths). It can't see a write through another crate's helper;
        // `no_built_in_tool_writes_its_output_as_a_file` runs the tools for that, and review
        // still has to keep tools to it.
        // (Read where the sources are: not in a wasm test runner.)
        let mut dirs = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut scanned = 0;
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
                    // The tests (this one reads files) are the `mod tests` closing each file: the
                    // first `#[cfg(test)]` must open it, so no test-only item hides the code after
                    // it. (Code after the test module would go unseen; the crate keeps none, and
                    // the runtime test below runs the tools whatever their source.)
                    let code = match text.find("#[cfg(test)]") {
                        None => text.as_str(),
                        Some(at) => {
                            let (code, tests) = text.split_at(at);
                            assert!(
                                tests.starts_with("#[cfg(test)]\nmod tests {"),
                                "{}: a `#[cfg(test)]` before its `mod tests`",
                                path.display()
                            );
                            code
                        }
                    };
                    // The standard library's files and paths, and rustgis-data's writers that take a path
                    // (its byte writers, such as GeoTIFF's, return bytes and are fine).
                    let reaches =
                        ["fs::", "std::fs", "File::", "io::Write", "OpenOptions", "std::path", "write_path", "write_atomic", "gpkg::write"];
                    // Code only, not comment lines. A path-like name counts only where a name starts
                    // (`fs::`, not `bufs::`); a writer's name counts anywhere (`do_write_atomic(`).
                    let code: String = code.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
                    let names = |reach: &str| {
                        let anywhere = reach.starts_with("write_") || reach.starts_with("gpkg::");
                        code.match_indices(reach)
                            .any(|(i, _)| anywhere || !code[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_'))
                    };
                    for reach in reaches {
                        assert!(!names(reach), "{}: names {reach}, so a tool's output could become a place", path.display());
                    }
                    scanned += 1;
                }
            }
        }
        assert!(cfg!(target_arch = "wasm32") || scanned > 5, "the sources were read");
    }

    /// The runtime side of the guard above: every built-in tool, run with `output` set to a path
    /// in an empty folder (other parameters from their defaults, layers from `inputs`), leaves
    /// that folder empty. A tool that can't run on these inputs fails before writing anything,
    /// so this catches a tool that writes its output wherever it runs.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn no_built_in_tool_writes_its_output_as_a_file() {
        let dir = std::env::temp_dir().join(format!("rustgis-tool-output-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let m = inputs();
        let mut ran = 0;
        for spec in TOOLS {
            let mut p = serde_json::Map::new();
            for param in spec.params {
                let value = match serde_json::from_str::<serde_json::Value>(param.default) {
                    Ok(v) => v,
                    Err(_) => match param.kind {
                        K::Number => serde_json::json!(1),
                        K::Bool => serde_json::json!(false),
                        K::Text => serde_json::json!("a"),
                        K::Choice(c) => serde_json::json!(c.first().copied().unwrap_or_default()),
                        K::Field | K::NumericField | K::CoordinateField => serde_json::json!("v"),
                        K::PolygonLayer => serde_json::json!("polys"),
                        K::Path => continue,
                        _ => serde_json::json!("pts"),
                    },
                };
                p.insert(param.name.into(), value);
            }
            let out = dir.join(format!("{}.geojson", spec.id));
            p.insert("output".into(), serde_json::json!(out.display().to_string()));
            ran += usize::from(crate::run(spec.id, &m, &serde_json::Value::Object(p)).is_ok());
            let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
            if !left.is_empty() {
                // (The folder goes before the test fails, not left behind.)
                let _ = std::fs::remove_dir_all(&dir);
                panic!("{} wrote {left:?} where its output was named", spec.id);
            }
        }
        let _ = std::fs::remove_dir(&dir);
        assert!(ran * 2 >= TOOLS.len(), "only {ran} of {} tools ran on these inputs", TOOLS.len());
    }

    #[test]
    fn dissolved_buffers_merge() {
        let mut pts = Dataset::new("p", GeometryType::Point, vec![]);
        pts.push(Some(Geometry::Point(Coord::new(0.0, 0.0))), vec![]);
        pts.push(Some(Geometry::Point(Coord::new(0.5, 0.0))), vec![]);
        let m = Map(HashMap::from([("p".to_string(), pts)]));
        let d = layer(crate::run("buffer", &m, &serde_json::json!({"input": "p", "distance": 50, "dissolve": "All"})).unwrap());
        match d.features[0].geometry.as_ref().unwrap() {
            Geometry::Polygon(polys) => assert_eq!(polys.len(), 1, "overlapping buffers merge into one polygon"),
            _ => panic!(),
        }
    }

    /// The buffer of one feature, run through the tool.
    fn buffered(g: Geometry, km: f64) -> Vec<Vec<Vec<Coord>>> {
        let mut ds = Dataset::new("f", GeometryType::Polygon, vec![]);
        ds.push(Some(g), vec![]);
        let m = Map(HashMap::from([("f".to_string(), ds)]));
        let b = layer(crate::run("buffer", &m, &serde_json::json!({"input": "f", "distance": km})).unwrap_or_else(|e| panic!("{e}")));
        match b.features.first().and_then(|f| f.geometry.clone()) {
            Some(Geometry::Polygon(polys)) => polys,
            other => panic!("expected a polygon: {other:?}"),
        }
    }

    /// The great-circle distance (km) from `c` to a line, sampled every 500 m along it.
    fn km_to(c: Coord, line: &[Coord]) -> f64 {
        line.windows(2)
            .flat_map(|w| {
                let n = (rustgis_geom::haversine_m(w[0], w[1]) / 500.0).ceil().max(1.0) as usize;
                (0..=n).map(move |i| rustgis_geom::geodesic_interpolate(w[0], w[1], i as f64 / n as f64))
            })
            .map(|p| rustgis_geom::haversine_m(c, p) / 1000.0)
            .fold(f64::INFINITY, f64::min)
    }

    /// Every vertex of every ring is `km` from `line`, within 0.2%. Where the arcs of two pieces
    /// cross, the vertex sits inside both circles by up to their chord's sag (0.5% with `geo`'s
    /// 0.2 rad steps), as anywhere between a planar buffer's vertices.
    fn all_at(polys: &[Vec<Vec<Coord>>], line: &[Coord], km: f64) {
        // A cap closed along a pole has corners on the pole, which aren't on the buffer's edge.
        for c in polys.iter().flatten().flatten().filter(|c| c.y.abs() < 90.0) {
            let d = km_to(*c, line);
            assert!(d < km * 1.002 && d > km * 0.994, "{c:?} is {d:.2} km away, not {km} km");
        }
    }

    /// Buffers are geodesic (#394): Web Mercator's scale changes with latitude, so one scale for
    /// the whole feature made a 1000 km circle at 60°N reach 872 km north and 1142 km south, and a
    /// 50 km buffer of a line from 5°S to 33°S 59 km wide at one end and 50 km at the other.
    #[test]
    fn buffers_keep_their_distance_on_the_earth() {
        let centre = Coord::new(10.0, 60.0);
        let circle = buffered(Geometry::Point(centre), 1000.0);
        all_at(&circle, &[centre, centre], 1000.0);
        let north = circle.iter().flatten().flatten().map(|c| c.y).fold(f64::MIN, f64::max);
        let south = circle.iter().flatten().flatten().map(|c| c.y).fold(f64::MAX, f64::min);
        assert!((north - 60.0 - (60.0 - south)).abs() < 0.01, "as far north as south: {north} {south}");

        let line = [Coord::new(-45.0, -5.0), Coord::new(-45.0, -33.0)];
        all_at(&buffered(Geometry::Line(vec![line.to_vec()]), 50.0), &line, 50.0);

        // A polygon too large for one projection, outward and inward, keeps the distance from
        // its outline; inward, the ring sits inside.
        let ring =
            vec![Coord::new(0.0, 40.0), Coord::new(20.0, 40.0), Coord::new(20.0, 60.0), Coord::new(0.0, 60.0), Coord::new(0.0, 40.0)];
        let grown = buffered(Geometry::Polygon(vec![vec![ring.clone()]]), 100.0);
        assert_eq!(grown.len(), 1, "one polygon, not the pieces it was drawn in");
        all_at(&grown, &ring, 100.0);
        let shrunk = buffered(Geometry::Polygon(vec![vec![ring.clone()]]), -100.0);
        all_at(&shrunk, &ring, 100.0);
        assert!(shrunk.iter().flatten().flatten().all(|c| c.x > 0.0 && c.x < 20.0 && c.y > 40.0 && c.y < 60.0));

        // A polygon split at the antimeridian keeps each half's buffer next to the half: buffered
        // in a projection centred near 180°, the western half came back at 185°, not −175°, and
        // missed the polygon it was joined to.
        let half = |w: f64, e: f64| {
            vec![vec![Coord::new(w, 60.0), Coord::new(e, 60.0), Coord::new(e, 70.0), Coord::new(w, 70.0), Coord::new(w, 60.0)]]
        };
        let split = buffered(Geometry::Polygon(vec![half(170.0, 180.0), half(-180.0, -170.0)]), 100.0);
        assert_eq!(split.len(), 2, "{split:?}");
        let within = |poly: &Vec<Vec<Coord>>, w: f64, e: f64| poly.iter().flatten().all(|c| (w..=e).contains(&c.x));
        assert!(split.iter().any(|p| within(p, 165.0, 185.0)) && split.iter().any(|p| within(p, -185.0, -165.0)), "{split:?}");
        // The error of a projection centred on the feature grows with a vertex's distance from
        // the centre, not with the buffer's distance: a small square keeps 3000 km.
        let small =
            vec![Coord::new(30.0, 50.0), Coord::new(31.0, 50.0), Coord::new(31.0, 51.0), Coord::new(30.0, 51.0), Coord::new(30.0, 50.0)];
        all_at(&buffered(Geometry::Polygon(vec![vec![small.clone()]]), 3000.0), &small, 3000.0);
        // A line round the world at the largest distance is a few hundred pieces, joined.
        let round = (0..=8).map(|i| Coord::new(-180.0 + 45.0 * i as f64, 10.0)).collect::<Vec<_>>();
        assert!(!buffered(Geometry::Line(vec![round]), 5000.0).is_empty());
        // A long line keeps a long distance: a stretch's vertices are up to 300 km from the
        // centre of its projection.
        all_at(&buffered(Geometry::Line(vec![line.to_vec()]), 1000.0), &line, 1000.0);
        // A circle round a pole is closed along it, so in lon/lat it holds the pole's
        // neighbourhood at every longitude; a circle centred on a pole is one too.
        for (centre, km) in [(Coord::new(10.0, 85.0), 1000.0), (Coord::new(0.0, 90.0), 500.0), (Coord::new(-60.0, -80.0), 1500.0)] {
            let cap = buffered(Geometry::Point(centre), km);
            all_at(&cap, &[centre, centre], km);
            let pole = centre.y.signum() * 89.0;
            let poly = cap.first().map(|rings| geo::Polygon::new(rings[0].iter().map(|c| geo::Coord { x: c.x, y: c.y }).collect(), vec![]));
            for lon in [-179.0, -90.0, 0.0, 45.0, 179.0] {
                assert!(
                    poly.as_ref().is_some_and(|p| geo::Contains::contains(p, &geo::Point::new(lon, pole))),
                    "{centre:?}: ({lon}, {pole})"
                );
            }
        }
        // Two antipodes still have their circles.
        let ends = buffered(Geometry::MultiPoint(vec![Coord::new(0.0, 0.0), Coord::new(180.0, 0.0)]), 100.0);
        assert_eq!(ends.len(), 2);
    }

    #[test]
    fn idw_interpolation_writes_a_raster() {
        let m = inputs();
        let r =
            crate::run("idwInterpolation", &m, &serde_json::json!({"input": "pts", "field": "v", "extent": "0,0,2,2", "pixelSize": 0.5}))
                .unwrap_or_else(|e| panic!("{e}"));
        let Output::Layers(out) = r.output else { panic!("expected layers") };
        let Some(OutputLayer::Raster { name, file_name, bytes }) = out.first() else { panic!("expected a raster: {out:?}") };
        assert_eq!(name, "pts_v_idw");
        let g = rustgis_data::raster::read_bytes(file_name, bytes).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((g.width, g.height), (4, 4));
        // Every value lies between the inputs', nearest the point it sits closest to.
        let d = &g.bands[0].data;
        assert!(d.iter().all(|v| (1.0..=3.0).contains(v)), "{d:?}");
        assert!(d[12] < d[3], "south-west, by the 1, is below north-east, by the 2: {d:?}");
        // A text field gives no values; a bad extent or power is refused.
        assert!(crate::run("idwInterpolation", &m, &serde_json::json!({"input": "polys", "field": "k"})).is_err(), "text field");
        for p in [
            serde_json::json!({"extent": "1,2,3"}),
            serde_json::json!({"extent": "0,bad,1,2,3"}),
            serde_json::json!({"extent": "0,0,1,NaN"}),
            serde_json::json!({"power": -1}),
            serde_json::json!({"power": "x"}),
            serde_json::json!({"pixelSize": 0}),
            serde_json::json!({"pixelSize": "-1"}),
            serde_json::json!({"pixelSize": "x"}),
        ] {
            let mut p = p;
            p["input"] = "pts".into();
            p["field"] = "v".into();
            assert!(crate::run("idwInterpolation", &m, &p).is_err(), "{p}");
        }
    }

    #[test]
    fn idw_auto_cell_stays_within_the_work_limit() {
        let e = Extent::new(0.0, 0.0, 3.0, 3.0);
        assert!((idw_auto_cell(&e, 10) - 0.01).abs() < 1e-12, "few points keep 300 cells across");
        for n in [5_000, 50_000, 1_000_000] {
            let c = idw_auto_cell(&e, n);
            let cells = ((3.0 / c).ceil() * (3.0 / c).ceil()) as usize;
            assert!(cells * n <= crate::rasterops::MAX_IDW_WORK, "{n} points: {cells} cells of {c}");
        }
    }

    #[test]
    fn kriging_interpolation_writes_prediction_and_variance_rasters() {
        let m = inputs();
        let r = crate::run(
            "kriging",
            &m,
            &serde_json::json!({
                "input": "pts",
                "field": "v",
                "model": "Spherical",
                "extent": "0,0,2,2",
                "pixelSize": 0.5
            }),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let Output::Layers(out) = r.output else { panic!("expected layers") };
        assert_eq!(out.len(), 2, "Expected prediction and variance rasters");
        let (pred_layer, var_layer) = match (&out[0], &out[1]) {
            (
                OutputLayer::Raster { name: p_name, file_name: p_file, bytes: p_bytes },
                OutputLayer::Raster { name: v_name, file_name: v_file, bytes: v_bytes },
            ) => {
                assert_eq!(p_name, "pts_v_kriging");
                assert_eq!(v_name, "pts_v_kriging_variance");
                (
                    rustgis_data::raster::read_bytes(p_file, p_bytes).unwrap_or_else(|e| panic!("{e}")),
                    rustgis_data::raster::read_bytes(v_file, v_bytes).unwrap_or_else(|e| panic!("{e}")),
                )
            }
            _ => panic!("expected two raster output layers: {out:?}"),
        };
        assert_eq!((pred_layer.width, pred_layer.height), (4, 4));
        assert_eq!((var_layer.width, var_layer.height), (4, 4));
        // Values in prediction band
        let d = &pred_layer.bands[0].data;
        assert!(d.iter().all(|v| (0.5..=3.5).contains(v)), "{d:?}");
        // Variance in variance raster is non-negative
        let var = &var_layer.bands[0].data;
        assert!(var.iter().all(|v| *v >= 0.0), "{var:?}");

        // Invalid models or parameters return errors cleanly
        assert!(crate::run("kriging", &m, &serde_json::json!({"input": "pts", "field": "v", "model": "UnknownModel"})).is_err());
        assert!(crate::run("kriging", &m, &serde_json::json!({"input": "pts", "field": "v", "neighbours": 1})).is_err());
        assert!(crate::run("kriging", &m, &serde_json::json!({"input": "pts", "field": "v", "nugget": -1})).is_err());
        assert!(crate::run("kriging", &m, &serde_json::json!({"input": "pts", "field": "v", "nugget": "abc"})).is_err());
        assert!(crate::run("kriging", &m, &serde_json::json!({"input": "pts", "field": "v", "sill": 0})).is_err());
        assert!(crate::run("kriging", &m, &serde_json::json!({"input": "pts", "field": "v", "sill": "xyz"})).is_err());
        assert!(crate::run("kriging", &m, &serde_json::json!({"input": "pts", "field": "v", "range": -5})).is_err());
        assert!(crate::run("kriging", &m, &serde_json::json!({"input": "pts", "field": "v", "range": "none"})).is_err());

        // Projected layer with non-degrees CRS returns error
        let Some(pts_layer) = m.0.get("pts") else { panic!("missing pts") };
        let mut proj_pts = pts_layer.clone();
        proj_pts.crs = Some(rustgis_data::raster::Crs::WebMercator);
        let m_proj = Map(HashMap::from([("proj_pts".to_string(), proj_pts)]));
        let err = crate::run("kriging", &m_proj, &serde_json::json!({"input": "proj_pts", "field": "v"})).unwrap_err();
        assert!(err.contains("degrees"));

        // Datasets with fewer than two points are cleanly rejected
        let mut single_pt = Dataset::new("single", GeometryType::Point, vec![Field::new("v", FieldType::Double)]);
        single_pt.push(Some(Geometry::Point(Coord::new(0.5, 0.5))), vec![Value::Float(1.0)]);
        let m_single = Map(HashMap::from([("single".to_string(), single_pt)]));
        assert_eq!(
            crate::run("kriging", &m_single, &serde_json::json!({"input": "single", "field": "v"})).unwrap_err(),
            "kriging needs at least two points with values"
        );

        // Deduplication averages coincident points with true arithmetic mean
        let mut dup_pts = Dataset::new("dup", GeometryType::Point, vec![Field::new("v", FieldType::Double)]);
        dup_pts.push(Some(Geometry::Point(Coord::new(0.5, 0.5))), vec![Value::Float(10.0)]);
        dup_pts.push(Some(Geometry::Point(Coord::new(0.5, 0.5))), vec![Value::Float(20.0)]);
        dup_pts.push(Some(Geometry::Point(Coord::new(0.5, 0.5))), vec![Value::Float(30.0)]);
        dup_pts.push(Some(Geometry::Point(Coord::new(1.5, 1.5))), vec![Value::Float(20.0)]);
        let m_dup = Map(HashMap::from([("dup".to_string(), dup_pts)]));
        let r_dup = crate::run(
            "kriging",
            &m_dup,
            &serde_json::json!({
                "input": "dup",
                "field": "v",
                "extent": "0,0,2,2",
                "pixelSize": 0.5
            }),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let Output::Layers(dup_layers) = r_dup.output else { panic!("expected layers") };
        let pred = match &dup_layers[0] {
            OutputLayer::Raster { file_name, bytes, .. } => rustgis_data::raster::read_bytes(file_name, bytes).unwrap(),
            _ => panic!("expected raster"),
        };
        assert!(pred.bands[0].data.iter().all(|v| (19.99..=20.01).contains(v)), "{:?}", pred.bands[0].data);

        // Deduplication merges near-duplicate samples across x-windows even when separated by different y
        let mut inter_pts = Dataset::new("inter", GeometryType::Point, vec![Field::new("v", FieldType::Double)]);
        inter_pts.push(Some(Geometry::Point(Coord::new(0.5, 0.5))), vec![Value::Float(10.0)]);
        inter_pts.push(Some(Geometry::Point(Coord::new(0.5 + 5e-13, 10.0))), vec![Value::Float(100.0)]);
        inter_pts.push(Some(Geometry::Point(Coord::new(0.5 + 9e-13, 0.5))), vec![Value::Float(30.0)]);
        inter_pts.push(Some(Geometry::Point(Coord::new(1.5, 1.5))), vec![Value::Float(20.0)]);
        let m_inter = Map(HashMap::from([("inter".to_string(), inter_pts)]));
        let r_inter = crate::run(
            "kriging",
            &m_inter,
            &serde_json::json!({
                "input": "inter",
                "field": "v",
                "extent": "0.25,0.25,0.75,0.75",
                "pixelSize": 0.5
            }),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let Output::Layers(inter_layers) = r_inter.output else { panic!("expected layers") };
        let pred_inter = match &inter_layers[0] {
            OutputLayer::Raster { file_name, bytes, .. } => rustgis_data::raster::read_bytes(file_name, bytes).unwrap(),
            _ => panic!("expected raster"),
        };
        assert!((pred_inter.bands[0].data[0] - 20.0).abs() < 1e-4, "expected 20.0 at (0.5, 0.5), got {}", pred_inter.bands[0].data[0]);
    }

    #[test]
    fn xy_table_to_point_places_rows_by_their_coordinates() {
        let fields = vec![Field::new("name", FieldType::Text), Field::new("lon", FieldType::Text), Field::new("lat", FieldType::Double)];
        let mut stops = Dataset::new("stops", GeometryType::None, fields);
        for (name, lon, lat) in [
            // A decimal comma, a decimal point with spaces around, and an integer in text.
            ("comma", Value::Text("-74,08".into()), Value::Float(4.6)),
            ("point", Value::Text(" -75.5 ".into()), Value::Float(6.25)),
            ("whole", Value::Text("-76".into()), Value::Int(3)),
            // No number, two commas, nothing at all, and off the globe: rows without a point.
            ("words", Value::Text("n/a".into()), Value::Float(1.0)),
            ("commas", Value::Text("1,234,5".into()), Value::Float(1.0)),
            ("empty", Value::Null, Value::Null),
            ("far", Value::Text("200".into()), Value::Float(0.0)),
        ] {
            stops.push(None, vec![Value::Text(name.into()), lon, lat]);
        }
        // The fields offered for coordinates: those holding numbers, as text or not.
        assert_eq!([0, 1, 2, 3].map(|i| holds_coordinates(&stops, i)), [false, true, true, false]);
        // A single comma is the decimal separator, never one for thousands.
        assert_eq!(["1,234", "1234,567", "1,234.5"].map(|t| coordinate(&Value::Text(t.into()))), [Some(1.234), Some(1234.567), None]);
        // Web Mercator metres: 10° east on the equator.
        let fields = vec![Field::new("e", FieldType::Double), Field::new("n", FieldType::Double)];
        let mut metres = Dataset::new("metres", GeometryType::None, fields);
        metres.push(None, vec![Value::Float(1_113_194.9), Value::Float(0.0)]);
        let m = Map(HashMap::from([("stops".to_string(), stops), ("metres".to_string(), metres)]));
        let run = |p: serde_json::Value| crate::run("xyTableToPoint", &m, &p);

        let r = run(serde_json::json!({"input": "stops", "x": "lon", "y": "lat"})).unwrap();
        assert_eq!(r.messages.len(), 2, "{:?}", r.messages);
        let out = layer(r);
        assert_eq!((out.name.as_str(), out.geometry_type), ("stops_Points", GeometryType::Point));
        assert_eq!((out.fields.len(), out.features.len()), (3, 7));
        let points: Vec<Option<(f64, f64)>> = out
            .features
            .iter()
            .map(|f| match &f.geometry {
                Some(Geometry::Point(c)) => Some((c.x, c.y)),
                _ => None,
            })
            .collect();
        assert_eq!(points, [Some((-74.08, 4.6)), Some((-75.5, 6.25)), Some((-76.0, 3.0)), None, None, None, None]);
        assert_eq!(out.value(3, 0), &Value::Text("words".into()), "rows without a point keep their attributes");

        // Another coordinate system, and the output's name.
        let out = layer(run(serde_json::json!({"input": "metres", "x": "e", "y": "n", "crs": "EPSG:3857", "output": "East"})).unwrap());
        assert_eq!(out.name, "East");
        match out.features.first().and_then(|f| f.geometry.as_ref()) {
            Some(Geometry::Point(c)) => assert!((c.x - 10.0).abs() < 1e-6 && c.y.abs() < 1e-6, "{c:?}"),
            g => panic!("{g:?}"),
        }

        // Metres read as longitude/latitude, fields without numbers, and junk parameters.
        let e = run(serde_json::json!({"input": "metres", "x": "e", "y": "n"})).unwrap_err();
        assert!(e.contains("coordinate system"), "{e}");
        let e = run(serde_json::json!({"input": "stops", "x": "name", "y": "lat"})).unwrap_err();
        assert!(e.contains("no row has numbers"), "{e}");
        for p in [
            serde_json::json!({"input": "stops", "x": "nope", "y": "lat"}),
            serde_json::json!({"input": "stops", "x": "lon"}),
            serde_json::json!({"input": "stops", "x": "lon", "y": "lat", "crs": "EPSG:99999999"}),
            serde_json::json!({"input": "stops", "x": "lon", "y": "lat", "crs": "somewhere"}),
            serde_json::json!({"input": "nope", "x": "lon", "y": "lat"}),
        ] {
            assert!(run(p.clone()).is_err(), "{p}");
        }
    }

    #[test]
    fn hostile_params_never_panic() {
        let m = inputs();
        let junk = [
            serde_json::json!({"input": "pts", "x": "v", "y": "v", "crs": "((", "output": 7}),
            serde_json::json!(null),
            serde_json::json!({}),
            serde_json::json!({"input": 5, "distance": "x"}),
            serde_json::json!({"input": "polys", "distance": 1e308, "tolerance": -1, "field": "nope", "where": "(((", "clip": "pts"}),
            serde_json::json!({"input": "pts", "polygons": "pts", "points": "polys", "select": "pts", "distance": -5}),
        ];
        for t in TOOLS {
            for p in &junk {
                let _ = crate::run(t.id, &m, p);
            }
        }
    }

    #[test]
    fn select_by_location_invert_excludes_features_without_geometry() {
        let mut polys = Dataset::new("polys", GeometryType::Polygon, vec![]);
        polys.push(Some(sq(0.0, 0.0, 2.0)), vec![]);
        let mut pts = Dataset::new("pts", GeometryType::Point, vec![]);
        pts.push(Some(Geometry::Point(Coord::new(1.0, 1.0))), vec![]); // inside
        pts.push(Some(Geometry::Point(Coord::new(10.0, 10.0))), vec![]); // outside
        pts.push(None, vec![]); // no geometry (unplaced record)
        let m = Map(HashMap::from([("polys".to_string(), polys), ("pts".to_string(), pts)]));

        let res_normal = crate::run("selectByLocation", &m, &serde_json::json!({"input": "pts", "select": "polys"})).unwrap();
        match res_normal.output {
            Output::Selection { rows, .. } => assert_eq!(rows, vec![0], "normal select should only select intersecting feature"),
            _ => panic!("expected selection"),
        }

        let res_inverted =
            crate::run("selectByLocation", &m, &serde_json::json!({"input": "pts", "select": "polys", "invert": "Yes"})).unwrap();
        match res_inverted.output {
            Output::Selection { rows, .. } => {
                assert_eq!(rows, vec![1], "inverted select should exclude unplaced records with null geometry")
            }
            _ => panic!("expected selection"),
        }
    }

    #[test]
    fn spatial_join_cases() {
        let mut polys = Dataset::new("polys", GeometryType::Polygon, vec![Field::new("k", FieldType::Text)]);
        polys.push(Some(sq(0.0, 0.0, 2.0)), vec![Value::Text("a".into())]);
        polys.push(Some(sq(1.0, 0.0, 2.0)), vec![Value::Text("b".into())]);

        let mut pts = Dataset::new("pts", GeometryType::Point, vec![Field::new("name", FieldType::Text), Field::new("k", FieldType::Text)]);
        pts.push(Some(Geometry::Point(Coord::new(1.5, 1.0))), vec![Value::Text("both".into()), Value::Text("orig".into())]);
        pts.push(Some(Geometry::Point(Coord::new(0.5, 0.5))), vec![Value::Text("first".into()), Value::Text("orig".into())]);
        pts.push(Some(Geometry::Point(Coord::new(50.0, 50.0))), vec![Value::Text("outside".into()), Value::Text("orig".into())]);

        let m = Map(HashMap::from([("polys".to_string(), polys), ("pts".to_string(), pts)]));

        // 1. One to one join (Intersects)
        let r1 = crate::run("spatialJoin", &m, &serde_json::json!({"target": "pts", "join": "polys"})).unwrap();
        let ds1 = layer(r1);
        assert_eq!(ds1.features.len(), 3);
        let jc = ds1.field_index("join_count").unwrap();
        let jk = ds1.field_index("join_k").unwrap();
        assert_eq!(ds1.value(0, jc), &Value::Int(2));
        assert!(matches!(ds1.value(0, jk), Value::Text(_)));
        assert_eq!(ds1.value(1, jc), &Value::Int(1));
        assert_eq!(ds1.value(1, jk), &Value::Text("a".into()));
        assert_eq!(ds1.value(2, jc), &Value::Int(0));
        assert_eq!(ds1.value(2, jk), &Value::Null);

        // 2. One to many join
        let r2 =
            crate::run("spatialJoin", &m, &serde_json::json!({"target": "pts", "join": "polys", "operation": "Join one to many"})).unwrap();
        let ds2 = layer(r2);
        assert_eq!(ds2.features.len(), 4);

        // 3. Inner join (Keep only matching)
        let r3 =
            crate::run("spatialJoin", &m, &serde_json::json!({"target": "pts", "join": "polys", "keepAll": "Keep only matching"})).unwrap();
        let ds3 = layer(r3);
        assert_eq!(ds3.features.len(), 2);

        // 4. Match option Contains (target polys, join pts)
        let r4 = crate::run("spatialJoin", &m, &serde_json::json!({"target": "polys", "join": "pts", "match": "Contains"})).unwrap();
        let ds4 = layer(r4);
        assert_eq!(ds4.features.len(), 2);
        let jc4 = ds4.field_index("join_count").unwrap();
        assert_eq!(ds4.value(0, jc4), &Value::Int(2));
        assert_eq!(ds4.value(1, jc4), &Value::Int(1));

        // 5. Match option Within (target pts, join polys)
        let r5 = crate::run("spatialJoin", &m, &serde_json::json!({"target": "pts", "join": "polys", "match": "Within"})).unwrap();
        let ds5 = layer(r5);
        assert_eq!(ds5.features.len(), 3);
        let jc5 = ds5.field_index("join_count").unwrap();
        assert_eq!(ds5.value(0, jc5), &Value::Int(2));
        assert_eq!(ds5.value(1, jc5), &Value::Int(1));
        assert_eq!(ds5.value(2, jc5), &Value::Int(0));

        // 6. Custom/empty prefix and field name collision deduplication
        let r6 = crate::run("spatialJoin", &m, &serde_json::json!({"target": "pts", "join": "polys", "prefix": ""})).unwrap();
        let ds6 = layer(r6);
        assert!(ds6.field_index("k").is_some());
        assert!(ds6.field_index("k_2").is_some(), "colliding field k should be deduplicated to k_2");
        assert_eq!(ds6.value(1, ds6.field_index("k").unwrap()), &Value::Text("orig".into()));
        assert_eq!(ds6.value(1, ds6.field_index("k_2").unwrap()), &Value::Text("a".into()));

        // 7. Indexed path (> 16 join geometries) for Intersects, Contains, and Within, including outside extent
        let mut grid_polys = Dataset::new("grid_polys", GeometryType::Polygon, vec![Field::new("id", FieldType::Integer)]);
        for y in 0..5 {
            for x in 0..5 {
                grid_polys.push(Some(sq(x as f64, y as f64, 1.0)), vec![Value::Int(y * 5 + x)]);
            }
        }
        assert_eq!(grid_polys.features.len(), 25);

        let mut test_pts = Dataset::new("test_pts", GeometryType::Point, vec![Field::new("tag", FieldType::Text)]);
        test_pts.push(Some(Geometry::Point(Coord::new(0.5, 0.5))), vec![Value::Text("in_0".into())]);
        test_pts.push(Some(Geometry::Point(Coord::new(4.5, 4.5))), vec![Value::Text("in_24".into())]);
        test_pts.push(Some(Geometry::Point(Coord::new(100.0, 100.0))), vec![Value::Text("far".into())]);

        let m2 = Map(HashMap::from([("grid_polys".to_string(), grid_polys), ("test_pts".to_string(), test_pts)]));

        let r_int = crate::run("spatialJoin", &m2, &serde_json::json!({"target": "test_pts", "join": "grid_polys", "match": "Intersects"}))
            .unwrap();
        let ds_int = layer(r_int);
        assert_eq!(ds_int.features.len(), 3);
        let jc_int = ds_int.field_index("join_count").unwrap();
        let jid_int = ds_int.field_index("join_id").unwrap();
        assert_eq!(ds_int.value(0, jc_int), &Value::Int(1));
        assert_eq!(ds_int.value(0, jid_int), &Value::Int(0));
        assert_eq!(ds_int.value(1, jc_int), &Value::Int(1));
        assert_eq!(ds_int.value(1, jid_int), &Value::Int(24));
        assert_eq!(ds_int.value(2, jc_int), &Value::Int(0));
        assert_eq!(ds_int.value(2, jid_int), &Value::Null);

        let r_within =
            crate::run("spatialJoin", &m2, &serde_json::json!({"target": "test_pts", "join": "grid_polys", "match": "Within"})).unwrap();
        let ds_within = layer(r_within);
        assert_eq!(ds_within.value(0, jc_int), &Value::Int(1));
        assert_eq!(ds_within.value(1, jc_int), &Value::Int(1));
        assert_eq!(ds_within.value(2, jc_int), &Value::Int(0));

        let r_contains =
            crate::run("spatialJoin", &m2, &serde_json::json!({"target": "grid_polys", "join": "test_pts", "match": "Contains"})).unwrap();
        let ds_contains = layer(r_contains);
        assert_eq!(ds_contains.features.len(), 25);
        let jc_poly = ds_contains.field_index("join_count").unwrap();
        assert_eq!(ds_contains.value(0, jc_poly), &Value::Int(1));
        assert_eq!(ds_contains.value(24, jc_poly), &Value::Int(1));
        assert_eq!(ds_contains.value(12, jc_poly), &Value::Int(0));

        // 8. Mismatched coordinate reference systems error
        let Some(pts_layer) = m.0.get("pts") else { panic!("missing pts") };
        let Some(polys_layer) = m.0.get("polys") else { panic!("missing polys") };
        let mut pts_crs = pts_layer.clone();
        pts_crs.crs = Some(rustgis_data::raster::Crs::LonLat);
        let mut polys_crs = polys_layer.clone();
        polys_crs.crs = Some(rustgis_data::raster::Crs::WebMercator);
        let m_crs = Map(HashMap::from([("polys".to_string(), polys_crs), ("pts".to_string(), pts_crs)]));
        let err = crate::run("spatialJoin", &m_crs, &serde_json::json!({"target": "pts", "join": "polys"})).unwrap_err();
        assert!(err.contains("coordinate reference system"));
    }
}
