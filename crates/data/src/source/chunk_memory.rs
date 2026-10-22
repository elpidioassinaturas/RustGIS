//! Allocation accounting for complete render chunks.
use crate::{Feature, Geometry, Value};
use std::mem::size_of;

/// Allocated vector storage owned by a geometry, excluding the enum in its containing array.
pub fn geometry_bytes(g: &Geometry) -> usize {
    let ring = |r: &Vec<crate::Coord>| r.capacity().saturating_mul(size_of::<crate::Coord>());
    let parts = |p: &Vec<Vec<crate::Coord>>| {
        p.iter().fold(p.capacity().saturating_mul(size_of::<Vec<crate::Coord>>()), |n, r| n.saturating_add(ring(r)))
    };
    match g {
        Geometry::Point(_) => 0,
        Geometry::MultiPoint(p) => ring(p),
        Geometry::Line(p) => parts(p),
        Geometry::Polygon(p) => {
            p.iter().fold(p.capacity().saturating_mul(size_of::<Vec<Vec<crate::Coord>>>()), |n, r| n.saturating_add(parts(r)))
        }
    }
}

/// Heap storage owned by a row, including unused capacity and attribute strings.
pub fn feature_bytes(f: &Feature) -> usize {
    let attrs = f.attrs.iter().fold(f.attrs.capacity().saturating_mul(size_of::<Value>()), |n, v| {
        n.saturating_add(match v {
            Value::Text(s) => s.capacity(),
            _ => 0,
        })
    });
    attrs.saturating_add(f.geometry.as_ref().map_or(0, geometry_bytes))
}

/// Actual allocated chunk storage, including spare capacity.
pub fn features_bytes(features: &Vec<Feature>) -> usize {
    features.iter().fold(features.capacity().saturating_mul(size_of::<Feature>()), |n, f| n.saturating_add(feature_bytes(f)))
}
