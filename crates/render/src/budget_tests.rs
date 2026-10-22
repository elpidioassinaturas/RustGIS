//! A small interaction budget must not change a full-resolution map export.
use super::*;
use rustgis_data::{Dataset, Field, FieldType, Geometry, GeometryType, Value};

#[test]
fn export_after_budgeted_interaction_matches_the_original_full_source() {
    let mut data = Dataset::new("budget export", GeometryType::Point, vec![Field::new("label", FieldType::Text)]);
    for i in 0..80 {
        data.push(Some(Geometry::Point(Coord::new((i % 10) as f64 * 0.01, (i / 10) as f64 * 0.01))), vec![Value::Text(format!("p{i}"))]);
    }
    let (bytes, _) = rustgis_data::io::write_bytes(&data, "fgb").unwrap();
    let original = rustgis_data::source::open_bytes("export.fgb", bytes, None).unwrap();
    let mut source = rustgis_map::source::SourceLayer::from_path("memory.fgb", None, original);
    source.byte_budget = 1024;
    let mut streamed = Map::new("budget export");
    let id = streamed.add_streamed("budget export", source);
    let extent = Extent::new(-1.0, -1.0, 1.0, 1.0);
    assert!(!streamed.layer_mut(id).unwrap().sync_source(&extent, true));
    assert!(streamed.layer(id).unwrap().dataset.features.len() < 80);
    let mut expected = streamed.clone();
    expected.layer_mut(id).unwrap().source = None;
    expected.layer_mut(id).unwrap().dataset = streamed.layer(id).unwrap().load_all().unwrap();
    for map in [&mut streamed, &mut expected] {
        map.camera = Camera::new(rustgis_geom::lonlat_to_merc(Coord::new(0.045, 0.035)), 55.0);
        let layer = map.layer_mut(id).unwrap();
        layer.labels.enabled = true;
        layer.labels.default.field = "label".into();
        layer.transparency = 0.25;
        layer.definition_query = Some("label <> 'p3'".into());
    }
    let view = View { width: 320.0, height: 240.0, pixels_per_point: 1.0, camera: streamed.camera, sky: false };
    let reference = render_complete(&mut expected, &view).unwrap();
    assert!(!reference.labels.is_empty(), "the reference must exercise real label placement");
    let actual = render_complete(&mut streamed, &view).unwrap();
    assert_eq!(streamed.layer(id).unwrap().dataset.features.len(), 80);
    assert!(reference.pixmap.data() == actual.pixmap.data(), "interaction sampling changed export pixels");
    assert_eq!(reference.labels.iter().map(|l| &l.text).collect::<Vec<_>>(), actual.labels.iter().map(|l| &l.text).collect::<Vec<_>>());
}
