//! A small QGIS project and style, written by hand in the shapes QGIS writes them.

use super::*;
use crate::symbology::{LabelPlacement, LineCap, Marker};

const PROJECT: &str = r#"<!DOCTYPE qgis PUBLIC 'http://mrcc.com/qgis.dtd' 'SYSTEM'>
<qgis projectname="Towns" version="3.42.0-Münster">
  <title>Towns</title>
  <projectCrs><spatialrefsys><authid>EPSG:2056</authid></spatialrefsys></projectCrs>
  <layer-tree-group>
    <layer-tree-layer id="towns_1" checked="Qt::Checked" name="Towns"/>
    <layer-tree-group name="Base" checked="Qt::Unchecked">
      <layer-tree-layer id="roads_1" checked="Qt::Checked" name="Roads"/>
    </layer-tree-group>
    <layer-tree-layer id="land_1" checked="Qt::Checked" name="Land"/>
  </layer-tree-group>
  <mapcanvas name="theMapCanvas">
    <extent><xmin>2690000</xmin><ymin>1250000</ymin><xmax>2700000</xmax><ymax>1260000</ymax></extent>
  </mapcanvas>
  <projectlayers>
    <maplayer type="vector" hasScaleBasedVisibilityFlag="1" minScale="100000" maxScale="1000" labelsEnabled="1">
      <id>land_1</id>
      <datasource>./data/base.gpkg|layername=land|subset="kind" &lt;&gt; 'sea'</datasource>
      <layername>Land</layername>
      <srs><spatialrefsys><authid>EPSG:2056</authid></spatialrefsys></srs>
      <provider encoding="UTF-8">ogr</provider>
      <renderer-v2 type="graduatedSymbol" attr="pop" referencescale="25000">
        <ranges>
          <range lower="0" upper="100" symbol="0" label="few" render="true"/>
          <range lower="100" upper="1000" symbol="1" label="many" render="true"/>
        </ranges>
        <symbols>
          <symbol type="fill" name="0" alpha="1"><layer class="SimpleFill" enabled="1" pass="0">
            <Option type="Map"><Option name="color" type="QString" value="200,220,180,255,rgb:0.78,0.86,0.7,1"/><Option name="style" type="QString" value="solid"/>
            <Option name="outline_color" type="QString" value="0,0,0,255"/><Option name="outline_width" type="QString" value="0.5"/><Option name="outline_width_unit" type="QString" value="MM"/></Option>
          </layer></symbol>
          <symbol type="fill" name="1" alpha="0.5"><layer class="LinePatternFill" enabled="1">
            <Option type="Map"><Option name="distance" value="2"/></Option>
            <symbol type="line" name="@1@0" alpha="1"><layer class="SimpleLine" enabled="1"><prop k="line_color" v="0,0,255,255"/><prop k="line_width" v="0.2"/></layer></symbol>
          </layer></symbol>
        </symbols>
      </renderer-v2>
      <layerOpacity>0.75</layerOpacity>
      <blendMode>13</blendMode>
      <labeling type="simple">
        <settings><text-style fieldName="name" fontSize="12" fontSizeUnit="Point" textColor="20,40,60,255" namedStyle="Bold Italic" fontFamily="Liberation Sans Narrow">
          <text-buffer bufferDraw="1"/></text-style><placement placement="1" quadOffset="2"/></settings>
      </labeling>
    </maplayer>
    <maplayer type="vector" labelsEnabled="1">
      <id>roads_1</id>
      <datasource>/abs/roads.shp</datasource>
      <layername>Roads</layername>
      <provider>ogr</provider>
      <renderer-v2 type="categorizedSymbol" attr="&quot;class&quot;" symbollevels="1">
        <categories>
          <category value="motorway" symbol="0" label="Motorway" render="true"/>
          <category value="track" symbol="1" label="track" render="false"/>
          <category value="" symbol="2" label="other" render="true"/>
        </categories>
        <symbols>
          <symbol type="line" name="0" alpha="1">
            <layer class="SimpleLine" enabled="1" pass="0"><prop k="line_color" v="90,40,20,255"/><prop k="line_width" v="1.2"/><prop k="capstyle" v="flat"/></layer>
            <layer class="SimpleLine" enabled="1" pass="3"><prop k="line_color" v="240,140,40,255"/><prop k="line_width" v="0.8"/><prop k="use_custom_dash" v="1"/></layer>
          </symbol>
          <symbol type="line" name="1" alpha="1"><layer class="SimpleLine" enabled="1"><prop k="line_color" v="0,0,0,255"/></layer></symbol>
          <symbol type="line" name="2" alpha="1"><layer class="MarkerLine" enabled="1"/></symbol>
        </symbols>
      </renderer-v2>
      <labeling type="rule-based">
        <rules key="root">
          <rule key="a" filter="&quot;class&quot; = 'motorway'" description="Motorways">
            <settings><text-style fieldName="ref" fontSize="9"/><placement placement="3"/></settings>
          </rule>
          <rule key="p" filter="&quot;lanes&quot; &gt; 1">
            <rule key="b" filter="ELSE" description="Rest" scalemaxdenom="50000">
              <settings><text-style fieldName="name" fontSize="7"/><placement placement="2"/></settings>
            </rule>
            <rule key="c" filter="&quot;kind&quot; = 'x'"><settings><text-style fieldName="name" fontSize="7"/></settings></rule>
            <rule key="off" filter="upper(&quot;kind&quot;) = 'X'" description="Switched off" active="0"><settings><text-style fieldName="name"/></settings></rule>
          </rule>
          <rule key="d" filter="length(&quot;name&quot;) &gt; 3"><settings><text-style fieldName="name"/></settings></rule>
        </rules>
      </labeling>
    </maplayer>
    <maplayer type="vector">
      <id>towns_1</id>
      <datasource>type=xyz&amp;url=https://tile.example.org/%7Bz%7D/%7Bx%7D/%7By%7D.png&amp;zmax=19</datasource>
      <layername>Towns</layername>
      <provider>wms</provider>
      <renderer-v2 type="RuleRenderer">
        <rules key="r">
          <rule key="1" filter="&quot;rank&quot; IN (1, 2)" symbol="0"/>
          <rule key="2" filter="ELSE" symbol="1"/>
        </rules>
        <symbols>
          <symbol type="marker" name="0" alpha="1"><layer class="SimpleMarker" enabled="1"><prop k="name" v="star"/><prop k="color" v="200,0,0,255"/><prop k="size" v="4"/></layer></symbol>
          <symbol type="marker" name="1" alpha="1"><layer class="FontMarker" enabled="1"><prop k="color" v="0,0,0,255"/><prop k="chr" v="A"/></layer></symbol>
        </symbols>
      </renderer-v2>
    </maplayer>
  </projectlayers>
</qgis>"#;

#[test]
fn a_project_reads_into_layers_in_tree_order_with_their_styles() {
    let mut report = Report::default();
    let p = read_project(PROJECT, "/maps", &mut report).unwrap();
    assert_eq!(p.title, "Towns");
    assert_eq!(p.crs.as_deref(), Some("EPSG:2056"));
    assert_eq!(p.extent, Some([2_690_000.0, 1_250_000.0, 2_700_000.0, 1_260_000.0]));
    let names: Vec<&str> = p.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Towns", "Roads", "Land"], "top first");
    let visible: Vec<bool> = p.layers.iter().map(|l| l.visible).collect();
    assert_eq!(visible, [true, false, true], "Roads is in an unchecked group");

    // Land: a GeoPackage table, filtered; graduated; opacity, blending, scales, reference scale.
    let land = &p.layers[2];
    assert_eq!(
        land.source,
        Source::File {
            path: "/maps/data/base.gpkg".into(),
            layer: Some("land".into()),
            subset: Some("\"kind\" <> 'sea'".into()),
            ignored: Vec::new()
        }
    );
    assert_eq!(land.definition_query.as_deref(), Some("\"kind\" <> 'sea'"));
    assert_eq!(land.crs.as_deref(), Some("EPSG:2056"));
    let s = &land.style;
    assert_eq!((s.transparency, s.blend), (Some(25.0), Some(Blend::Multiply)));
    let (min, max) = (s.min_scale.flatten().unwrap(), s.max_scale.flatten().unwrap());
    assert!((min - 100_000.0 * QGIS_SCALE).abs() < 1e-6 && (max - 1000.0 * QGIS_SCALE).abs() < 1e-6);
    assert!((s.reference_scale.unwrap() - 25_000.0 * QGIS_SCALE).abs() < 1e-6);
    let Some(Renderer::Graduated { field, breaks, symbols, min, class_labels, .. }) = &s.renderer else { panic!("{:?}", s.renderer) };
    assert_eq!((field.as_str(), breaks.as_slice(), *min), ("pop", &[100.0, 1000.0][..], 0.0));
    assert_eq!(symbols[0].fill, Rgba::rgb(200, 220, 180));
    assert!((symbols[0].stroke_width - 0.5 * 96.0 / 25.4).abs() < 1e-4, "0.5 mm in points");
    // The pattern fill: a tint of its line colour, at the symbol's alpha too.
    assert_eq!((symbols[1].fill.b(), symbols[1].fill.a()), (255, 45));
    assert_eq!(class_labels.get(&1).map(String::as_str), Some("many"));
    let labels = s.labels.as_ref().unwrap();
    let d = &labels.default;
    assert!(labels.enabled && labels.classes.is_empty());
    assert_eq!((d.field.as_str(), d.size, d.bold, d.italic, d.halo), ("name", 12.0, true, true, true));
    assert_eq!((d.color, d.placement), (Rgba::rgb(20, 40, 60), LabelPlacement::AboveRight));
    assert_eq!(d.font.as_deref(), Some("Liberation Sans Narrow"), "QGIS's font family");

    // Roads: categories with stacked strokes in levels, a hidden one and the rest.
    let roads = &p.layers[1];
    assert_eq!(roads.source, Source::File { path: "/abs/roads.shp".into(), layer: None, subset: None, ignored: Vec::new() });
    let s = &roads.style;
    assert_eq!((s.stroke_levels, s.line_strokes.as_deref()), (Some(true), Some(&[][..])));
    let Some(Renderer::Unique { field, classes, class_strokes, class_labels, other, .. }) = &s.renderer else { panic!() };
    assert_eq!(field, "class");
    assert_eq!(classes.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["motorway", "track"]);
    assert_eq!(classes[1].1.stroke.a(), 0, "not rendered: hidden");
    assert_eq!(other.stroke.a(), 0, "a marker line only: nothing RustGIS draws");
    let motorway = &class_strokes["motorway"];
    assert_eq!(motorway.len(), 2);
    assert_eq!((motorway[0].cap, motorway[0].level, motorway[1].level), (LineCap::Butt, Some(0), Some(3)));
    assert_eq!(class_labels.get("motorway").map(String::as_str), Some("Motorway"));
    // Labels: a class per rule; the else rule takes what its sibling doesn't, under its parent.
    let l = s.labels.as_ref().unwrap();
    assert!(!l.default.visible);
    let by = |n: &str| l.classes.iter().find(|c| c.name == n).unwrap();
    assert_eq!((by("Motorways").where_.as_deref(), by("Motorways").placement), (Some("\"class\" = 'motorway'"), LabelPlacement::Curved));
    let rest = by("Rest");
    assert_eq!(rest.where_.as_deref(), Some("(\"lanes\" > 1) AND (NOT ((\"kind\" = 'x')))"));
    assert_eq!(rest.placement, LabelPlacement::Parallel);
    assert!((rest.min_scale.unwrap() - 50_000.0 * QGIS_SCALE).abs() < 1e-6);
    assert_eq!(l.classes.len(), 3, "the function filter's rule is left out");

    // Towns: XYZ tiles as the source; rules on one field read as categories.
    let towns = &p.layers[0];
    assert_eq!(
        towns.source,
        Source::Xyz { url: "https://tile.example.org/{z}/{x}/{y}.png".into(), zmin: None, zmax: Some(19), vector: false }
    );
    let Some(Renderer::Unique { field, classes, other, .. }) = &towns.style.renderer else { panic!() };
    assert_eq!((field.as_str(), classes.len()), ("rank", 2));
    assert_eq!((classes[0].1.marker, classes[0].1.fill), (Marker::Star, Rgba::rgb(200, 0, 0)));
    assert_eq!((other.marker, other.fill), (Marker::Circle, Rgba::rgb(0, 0, 0)));

    // What didn't carry over is said, once for each thing.
    let said =
        |layer: &str, kind: NoteKind, part: &str| report.notes.iter().any(|n| n.layer == layer && n.kind == kind && n.text.contains(part));
    assert!(said("Land", NoteKind::Approximated, "line pattern"));
    assert!(said("Land", NoteKind::Approximated, "above the last"), "values outside the ranges are noted");
    // A QGIS font family is the label class's font, drawn in it where it's installed.
    assert!(!said("Land", NoteKind::Approximated, "Liberation Sans Narrow"), "no longer approximated");
    assert!(said("Roads", NoteKind::Approximated, "dashed"));
    assert!(said("Roads", NoteKind::Skipped, "markers and hash marks"));
    assert!(said("Roads", NoteKind::Skipped, "length("));
    assert!(said("Towns", NoteKind::Approximated, "font markers"));
    // A switched-off label rule is never drawn, so its filter isn't noted.
    assert!(!report.notes.iter().any(|n| n.text.contains("Switched off")), "{:?}", report.notes);
}

#[test]
fn a_style_file_reads_in_either_settings_format() {
    // QGIS before 3.26 writes a symbol layer's settings as <prop k v>, after as <Option>.
    let qml = |settings: &str| {
        format!(
            r#"<!DOCTYPE qgis><qgis version="3.10"><renderer-v2 type="singleSymbol"><symbols><symbol type="line" name="0" alpha="1"><layer class="SimpleLine" enabled="1">{settings}</layer></symbol></symbols></renderer-v2></qgis>"#
        )
    };
    for settings in [
        r#"<prop k="line_color" v="10,20,30,255"/><prop k="line_width" v="2"/><prop k="line_width_unit" v="Pixel"/>"#,
        r#"<Option type="Map"><Option name="line_color" value="10,20,30,255"/><Option name="line_width" value="2"/><Option name="line_width_unit" value="Pixel"/></Option>"#,
    ] {
        let mut report = Report::default();
        let s = read_style(&qml(settings), "Rivers", &mut report).unwrap();
        let Some(Renderer::Single(sym)) = s.renderer else { panic!() };
        assert_eq!((sym.stroke, sym.stroke_width), (Rgba::rgb(10, 20, 30), 2.0));
        assert_eq!(s.line_strokes.as_deref(), Some(&[][..]), "one plain line: no stack");
        assert!(report.notes.is_empty(), "{:?}", report.notes);
    }
    // Applied to a layer: its parts only.
    let mut layer = crate::Layer::new(1, rustgis_data::Dataset::new("r", rustgis_data::GeometryType::Polyline, vec![]));
    layer.transparency = 40.0;
    let s = read_style(&qml(r#"<prop k="line_color" v="1,2,3,255"/>"#), "r", &mut Report::default()).unwrap();
    s.apply(&mut layer);
    assert_eq!((layer.transparency, matches!(layer.renderer, Renderer::Single(_))), (40.0, true));
}

#[test]
fn hostile_files_are_refused_or_bounded() {
    let mut r = Report::default();
    assert!(read_project("not xml", "", &mut r).is_err());
    assert!(read_project("<other/>", "", &mut r).is_err());
    assert!(read_style("<qgis><renderer-v2 type=\"x\"", "", &mut r).is_err());
    // An entity expansion bomb: no DTD is read.
    let bomb = r#"<!DOCTYPE qgis [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;">]><qgis><title>&b;</title></qgis>"#;
    assert!(read_project(bomb, "", &mut r).is_err());
    // Rules nested past any sensible depth, and nonsense values: read without a crash.
    let deep = format!(
        r#"<qgis><renderer-v2 type="RuleRenderer" referencescale="-5"><rules>{}{}</rules><symbols/></renderer-v2><layerOpacity>NaN</layerOpacity></qgis>"#,
        "<rule filter=\"a = 1\">".repeat(200),
        "</rule>".repeat(200)
    );
    let s = read_style(&deep, "x", &mut r).unwrap();
    assert_eq!((s.reference_scale, s.transparency), (None, None));
    // The same note many times is kept once, counted.
    let mut r = Report::default();
    for _ in 0..10_000 {
        r.skipped("a", "the same");
    }
    assert_eq!((r.notes.len(), r.notes[0].count), (1, 10_000));
    // Many different notes stop at the cap; the ones kept still count their repeats, each in its
    // own place (layer, kind and text all tell notes apart).
    for i in 0..(MAX_NOTES + 500) {
        r.approximated("a", format!("note {i}"));
    }
    assert_eq!(r.notes.len(), MAX_NOTES);
    r.approximated("a", "note 7");
    r.approximated("b", "note 7");
    r.skipped("a", "note 7");
    let at = r.notes.iter().position(|n| n.text == "note 7").unwrap();
    assert_eq!((r.notes[at].count, r.notes[at].kind), (2, NoteKind::Approximated));
    assert!(!r.notes.iter().any(|n| n.layer == "b"), "past the cap, a new note isn't kept");
}

#[test]
fn paths_on_a_drive_are_told_apart_from_absolute_ones() {
    for p in ["C:roads.gpkg", "d:data\\x.shp", "C:"] {
        assert!(is_drive_relative(p) && !is_relative(p), "{p}");
    }
    for p in ["C:/gis/roads.gpkg", "C:\\gis\\roads.gpkg", "/srv/x.shp", "https://x.org/a.geojson"] {
        assert!(!is_drive_relative(p) && !is_relative(p), "{p}");
    }
    assert!(is_relative("./data/x.shp") && is_relative("data/x.shp") && !is_drive_relative("data/x.shp"));
}

#[test]
fn merged_categories_and_dashes_carry_over() {
    // QGIS 4 writes a category of several values as <val>s; an empty value is all the others.
    let qml = r#"<qgis version="4.2.3"><renderer-v2 type="categorizedSymbol" attr="kind">
      <categories>
        <category value="a" symbol="0" render="true"/>
        <category label="c or d" symbol="1" render="true"><val type="string" value="c"/><val type="string" value="d"/></category>
        <category value="" symbol="2" label="other" render="true"/>
      </categories>
      <symbols>
        <symbol type="line" name="0" alpha="1"><layer class="SimpleLine" enabled="1"><Option type="Map">
          <Option name="line_color" value="10,0,0,255"/><Option name="line_width" value="1"/><Option name="capstyle" value="flat"/>
          <Option name="use_custom_dash" value="1"/><Option name="customdash" value="2.5;0.5;0"/><Option name="customdash_unit" value="MM"/></Option></layer></symbol>
        <symbol type="line" name="1" alpha="1"><layer class="SimpleLine" enabled="1"><Option type="Map">
          <Option name="line_color" value="20,0,0,255"/><Option name="line_width" value="3"/><Option name="line_width_unit" value="Pixel"/><Option name="line_style" value="dot"/></Option></layer></symbol>
        <symbol type="line" name="2" alpha="1"><layer class="SimpleLine" enabled="1"><Option type="Map"><Option name="line_color" value="30,0,0,255"/></Option></layer></symbol>
      </symbols></renderer-v2></qgis>"#;
    let mut report = Report::default();
    let s = read_style(qml, "Paths", &mut report).unwrap();
    let Some(Renderer::Unique { classes, class_strokes, class_labels, other, .. }) = &s.renderer else { panic!("{:?}", s.renderer) };
    let values: Vec<&str> = classes.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(values, ["a", "c", "d"], "both values of the merged category, not \"all others\"");
    assert_eq!((classes[1].1.stroke, classes[2].1.stroke), (Rgba::rgb(20, 0, 0), Rgba::rgb(20, 0, 0)));
    assert_eq!(other.stroke, Rgba::rgb(30, 0, 0));
    assert_eq!(class_labels.get("c").map(String::as_str), Some("c or d"));
    // A custom dash in millimetres (its odd trailing step of nothing dropped); Qt's dots in widths.
    let mm = 96.0 / 25.4;
    let dash = class_strokes["a"][0].dash.unwrap();
    assert_eq!(dash.steps().len(), 2);
    assert!((dash.steps()[0] - 2.5 * mm).abs() < 1e-3 && (dash.steps()[1] - 0.5 * mm).abs() < 1e-3, "{:?}", dash.steps());
    assert_eq!(class_strokes["c"][0].dash.unwrap().steps(), &[3.0, 6.0]);
    assert!(!report.notes.iter().any(|n| n.text.contains("dash")), "{:?}", report.notes);
}

#[test]
fn files_of_qgis_1_and_2_read_by_their_old_names() {
    // Before QGIS 3: no layer tree but a legend, the canvas's coordinate system, scale limits named
    // the other way round, opacity as transparency, and QGIS 2's names for line and fill settings.
    let qgs = r#"<qgis projectname="" version="2.14.6-Essen"><title>Old</title>
      <mapcanvas><units>meters</units><extent><xmin>0</xmin><ymin>0</ymin><xmax>10</xmax><ymax>10</ymax></extent>
        <destinationsrs><spatialrefsys><authid>EPSG:21781</authid></spatialrefsys></destinationsrs></mapcanvas>
      <legend updateDrawingOrder="true">
        <legendlayer checked="Qt::Checked" name="roads"><filegroup><legendlayerfile layerid="roads1" visible="1"/></filegroup></legendlayer>
        <legendgroup checked="Qt::Unchecked" name="base">
          <legendlayer checked="Qt::Checked" name="land"><filegroup><legendlayerfile layerid="land1" visible="1"/></filegroup></legendlayer>
        </legendgroup>
      </legend>
      <projectlayers>
        <maplayer type="vector" minimumScale="1000" maximumScale="50000" hasScaleBasedVisibilityFlag="1">
          <id>land1</id><datasource>/d/land.shp</datasource><layername>land</layername><provider>ogr</provider>
          <renderer-v2 type="singleSymbol"><symbols><symbol type="fill" name="0" alpha="1" outputUnit="MM"><layer class="SimpleFill" pass="0" locked="0">
            <prop k="color" v="200,200,150,255"/><prop k="color_border" v="10,20,30,255"/><prop k="style" v="solid"/><prop k="style_border" v="dash"/><prop k="width_border" v="0.5"/>
          </layer></symbol></symbols></renderer-v2>
          <layerTransparency>40</layerTransparency>
          <customproperties><property key="labeling/enabled" value="true"/><property key="labeling/fieldName" value="name"/></customproperties>
        </maplayer>
        <maplayer type="vector">
          <id>roads1</id><datasource>/d/roads.shp</datasource><layername>roads</layername><provider>ogr</provider>
          <renderer-v2 type="singleSymbol"><symbols><symbol type="line" name="0" alpha="1" outputUnit="Pixel"><layer class="SimpleLine" pass="0" locked="0">
            <prop k="color" v="40,40,40,255"/><prop k="width" v="2"/><prop k="penstyle" v="dash"/><prop k="capstyle" v="flat"/>
          </layer></symbol></symbols></renderer-v2>
          <transparencyLevelInt>255</transparencyLevelInt>
        </maplayer>
      </projectlayers></qgis>"#;
    let mut report = Report::default();
    let p = read_project(qgs, "", &mut report).unwrap();
    assert_eq!(p.crs.as_deref(), Some("EPSG:21781"), "the canvas's");
    let names: Vec<(&str, bool)> = p.layers.iter().map(|l| (l.name.as_str(), l.visible)).collect();
    assert_eq!(names, [("roads", true), ("land", false)], "the legend's order; land's group is unchecked");
    let (roads, land) = (&p.layers[0].style, &p.layers[1].style);
    // maximumScale was the out-beyond limit, minimumScale the in-beyond.
    let (min, max) = (land.min_scale.flatten().unwrap(), land.max_scale.flatten().unwrap());
    assert!((min - 50_000.0 * QGIS_SCALE).abs() < 1e-6 && (max - 1000.0 * QGIS_SCALE).abs() < 1e-6);
    assert_eq!((land.transparency, roads.transparency), (Some(40.0), Some(0.0)));
    let Some(Renderer::Single(fill)) = &land.renderer else { panic!() };
    assert_eq!((fill.fill, fill.stroke), (Rgba::rgb(200, 200, 150), Rgba::rgb(10, 20, 30)));
    assert!((fill.stroke_width - 0.5 * 96.0 / 25.4).abs() < 1e-4);
    // The line, in the symbol's unit (pixels), dashed as Qt dashes: 4 and 2 widths.
    let Some(Renderer::Single(line)) = &roads.renderer else { panic!() };
    assert_eq!((line.stroke, line.stroke_width), (Rgba::rgb(40, 40, 40), 2.0));
    let strokes = roads.line_strokes.as_deref().unwrap();
    assert_eq!((strokes.len(), strokes[0].dash.map(|d| d.steps().to_vec())), (1, Some(vec![8.0, 4.0])));
    let said = |layer: &str, part: &str| report.notes.iter().any(|n| n.layer == layer && n.text.contains(part));
    assert!(said("land", "dashed polygon outlines"));
    assert!(said("land", "format of QGIS 2"));
}

#[test]
fn what_rustgis_has_no_place_for_is_noted() {
    let effect = r#"<effect enabled="1" type="effectStack"><effect type="drawSource"><Option type="Map"><Option name="enabled" value="1"/></Option></effect>
        <effect type="dropShadow"><Option type="Map"><Option name="enabled" value="1"/></Option></effect></effect>"#;
    let plain = r#"<effect enabled="1" type="effectStack"><effect type="drawSource"><Option type="Map"><Option name="enabled" value="1"/></Option></effect>
        <effect type="outerGlow"><Option type="Map"><Option name="enabled" value="0"/></Option></effect></effect>"#;
    let qgs = format!(
        r#"<qgis version="3.42.0">
      <projectlayers>
        <maplayer type="vector" labelsEnabled="1"><id>a</id><datasource>/d/a.shp</datasource><layername>A</layername><provider>ogr</provider>
          <temporal enabled="1" mode="1" startField="day"/>
          <map-layer-style-manager current="default"><map-layer-style name="default"/><map-layer-style name="night"/></map-layer-style-manager>
          <vectorjoins><join joinLayerId="b"/></vectorjoins>
          <expressionfields><field name="twice" expression="&quot;v&quot; * 2"/></expressionfields>
          <SingleCategoryDiagramRenderer diagramType="Pie"><DiagramCategory enabled="1"/></SingleCategoryDiagramRenderer>
          <renderer-v2 type="singleSymbol" enableorderby="1">{effect}<symbols><symbol type="marker" name="0" alpha="1">
            <layer class="SimpleMarker" enabled="1"><Option type="Map"><Option name="color" value="1,2,3,255"/></Option>{plain}</layer></symbol></symbols></renderer-v2>
          <labeling type="simple"><settings calloutType="simple">
            <text-style fieldName="name"><text-buffer bufferDraw="1" bufferSize="1" bufferColor="255,255,255,255"/></text-style>
            <text-format wrapChar="" autoWrapLength="12"/>
            <placement placement="1" dist="2" xOffset="1" yOffset="0" repeatDistance="0" priority="8"/>
            <dd_properties><Option type="Map"><Option name="properties" type="Map"><Option name="Size" type="Map"><Option name="active" value="true"/><Option name="field" value="size"/></Option></Option></Option></dd_properties>
            <callout type="simple"><Option type="Map"><Option name="enabled" value="1"/></Option></callout>
          </settings></labeling>
        </maplayer>
      </projectlayers>
      <main-annotation-layer><items><item type="pointtext"/><item type="linetext"/></items></main-annotation-layer>
      <Layouts><Layout name="One"/></Layouts>
      <Bookmarks><Bookmark name="Here"/></Bookmarks>
      <visibility-presets><visibility-preset name="Only points"/></visibility-presets>
      <relations><relation id="r"/></relations>
    </qgis>"#
    );
    let mut report = Report::default();
    read_project(&qgs, "", &mut report).unwrap();
    let said = |layer: &str, part: &str| report.notes.iter().any(|n| n.layer == layer && n.text.contains(part));
    for part in ["print layouts (1)", "annotations (2", "bookmarks (1)", "map themes (1)", "relations between layers (1)"] {
        assert!(said("", part), "{part}: {:?}", report.notes);
    }
    for part in [
        "time settings",
        "other styles (night)",
        "joins",
        "virtual fields",
        "diagrams",
        "order-by",
        "draw effects",
        "label halos",
        "wrapped labels",
        "label distances",
        "label offsets",
        "label priorities",
        "callouts",
        "data-defined label settings",
    ] {
        assert!(said("A", part), "{part}: {:?}", report.notes);
    }
    // The renderer's effect is noted once; the symbol layer's, only plain drawing, isn't an effect.
    assert_eq!(report.notes.iter().find(|n| n.text.contains("draw effects")).map(|n| n.count), Some(1));
    assert!(!said("A", "repeated"), "a repeat distance of 0 is none");
}

/// A raster style file with `renderer` as its renderer and `pipe` after it in the pipe.
fn raster_qml(renderer: &str, pipe: &str) -> String {
    format!(
        r#"<!DOCTYPE qgis PUBLIC 'http://mrcc.com/qgis.dtd' 'SYSTEM'><qgis version="4.2.3" layerType="Raster"><pipe>{renderer}{pipe}</pipe><blendMode>0</blendMode></qgis>"#
    )
}

#[test]
fn raster_renderers_read_as_rustgis_raster_styles() {
    use crate::raster::{RasterRenderer, Resampling, Stretch};
    let read = |renderer: &str, pipe: &str| {
        let mut report = Report::default();
        let s = read_style(&raster_qml(renderer, pipe), "R", &mut report).unwrap();
        (s, report)
    };
    // Pseudocolour, interpolated (SMV25's rock): a ramp from transparent white to grey over 0 to 1.
    let (s, report) = read(
        r##"<rasterrenderer type="singlebandpseudocolor" band="1" opacity="0.5" alphaBand="-1"><rastershader>
          <colorrampshader colorRampType="INTERPOLATED" clip="0" minimumValue="0" maximumValue="1">
            <item value="0" color="#ffffff" alpha="0" label="0"/><item value="1" color="#3f3f3f" alpha="255" label="1"/>
          </colorrampshader></rastershader></rasterrenderer>"##,
        r#"<brightnesscontrast brightness="51" contrast="-10" gamma="2"/><huesaturation saturation="0" grayscaleMode="0" colorizeOn="0" invertColors="0"/>
           <provider><resampling enabled="true" zoomedInResamplingMethod="bilinear" zoomedOutResamplingMethod="nearestNeighbour"/></provider>"#,
    );
    let r = s.raster.unwrap();
    let RasterRenderer::Stretch { band: 0, ramp, invert: false } = &r.renderer else { panic!("{:?}", r.renderer) };
    assert_eq!((ramp.at(0.0).a(), ramp.at(1.0)), (0, Rgba::rgb(0x3f, 0x3f, 0x3f)));
    assert_eq!(r.stretch, Stretch::custom(0.0, 1.0));
    assert_eq!((r.brightness, r.contrast, r.gamma, r.resampling), (20.0, -10.0, 2.0, Resampling::Bilinear));
    assert_eq!(s.transparency, Some(50.0));
    assert!(report.notes.is_empty(), "{:?}", report.notes);

    // Discrete: classes up to each entry's value, the last open-ended; values outside clipped.
    let (s, _) = read(
        r##"<rasterrenderer type="singlebandpseudocolor" band="2"><rastershader><colorrampshader colorRampType="DISCRETE" clip="1" minimumValue="400" maximumValue="600">
            <item value="450" color="#ff0000"/><item value="500" color="#00ff00"/><item value="inf" color="#0000ff"/></colorrampshader></rastershader></rasterrenderer>"##,
        "",
    );
    let r = s.raster.unwrap();
    let RasterRenderer::Classify { band: 1, ramp, breaks, .. } = &r.renderer else { panic!("{:?}", r.renderer) };
    assert_eq!((breaks.len(), breaks[0], breaks[1]), (3, 450.0, 500.0));
    assert!(breaks[2].is_finite() && breaks[2] > 1e30, "inf kept as a number a project can hold");
    assert_eq!((ramp.pick(0, 3), ramp.pick(2, 3)), (Rgba::rgb(255, 0, 0), Rgba::rgb(0, 0, 255)));
    assert_eq!((r.mask.min, r.mask.max), (Some(400.0), Some(600.0)));

    // Exact and paletted: a colour per value, sorted.
    for renderer in [
        r##"<rasterrenderer type="singlebandpseudocolor" band="1"><rastershader><colorrampshader colorRampType="EXACT">
            <item value="7" color="#00ff00"/><item value="3" color="#ff0000"/></colorrampshader></rastershader></rasterrenderer>"##,
        r##"<rasterrenderer type="paletted" band="1"><colorPalette><paletteEntry value="7" color="#00ff00" alpha="255"/><paletteEntry value="3" color="#ff0000" alpha="255"/></colorPalette></rasterrenderer>"##,
    ] {
        let (s, _) = read(renderer, "");
        let r = s.raster.unwrap();
        let RasterRenderer::UniqueValues { band: 0, values, .. } = &r.renderer else { panic!("{:?}", r.renderer) };
        assert_eq!(values, &[(3.0, Rgba::rgb(255, 0, 0)), (7.0, Rgba::rgb(0, 255, 0))]);
    }

    // Grey: black to white, stretched to its enhancement's bounds; a value made transparent and
    // a user no-data value hide those values.
    let gray = r#"<rasterrenderer type="singlebandgray" grayBand="1" gradient="BlackToWhite"><rasterTransparency><singleValuePixelList>
          <pixelListEntry min="-9999" max="-9999" percentTransparent="100"/><pixelListEntry min="1" max="5" percentTransparent="50"/></singleValuePixelList></rasterTransparency>
          <contrastEnhancement><minValue>10</minValue><maxValue>200</maxValue><algorithm>StretchToMinimumMaximum</algorithm></contrastEnhancement></rasterrenderer>"#;
    let qml = raster_qml(gray, "").replace(
        "</qgis>",
        r#"<noData><noDataList bandNo="1" useSrcNoData="1"><noDataRange min="0" max="0"/></noDataList></noData></qgis>"#,
    );
    let mut report = Report::default();
    let r = read_style(&qml, "R", &mut report).unwrap().raster.unwrap();
    let RasterRenderer::Stretch { ramp, .. } = &r.renderer else { panic!() };
    assert_eq!((ramp.at(0.0), ramp.at(1.0)), (Rgba::rgb(0, 0, 0), Rgba::rgb(255, 255, 255)));
    assert_eq!(r.stretch, Stretch::custom(10.0, 200.0));
    assert_eq!(r.mask.values, [-9999.0, 0.0]);
    assert!(report.notes.iter().any(|n| n.text.contains("in part")), "{:?}", report.notes);

    // Multiband: RGB from 1-based bands, a stretch per band; a canvas-wide cut follows the view.
    let (s, _) = read(
        r#"<rasterrenderer type="multibandcolor" redBand="3" greenBand="2" blueBand="1">
          <redContrastEnhancement><minValue>1</minValue><maxValue>2</maxValue><algorithm>StretchToMinimumMaximum</algorithm></redContrastEnhancement>
          <greenContrastEnhancement><minValue>3</minValue><maxValue>4</maxValue><algorithm>StretchToMinimumMaximum</algorithm></greenContrastEnhancement>
          <blueContrastEnhancement><minValue>5</minValue><maxValue>6</maxValue><algorithm>StretchToMinimumMaximum</algorithm></blueContrastEnhancement></rasterrenderer>"#,
        "",
    );
    let r = s.raster.unwrap();
    assert_eq!(r.renderer, RasterRenderer::Rgb { red: 2, green: 1, blue: 0 });
    assert_eq!(r.stretch, Stretch::custom_bands([[1.0, 2.0], [3.0, 4.0], [5.0, 6.0]]));
    let (s, _) = read(
        r#"<rasterrenderer type="singlebandgray" grayBand="1"><minMaxOrigin><limits>CumulativeCut</limits><extent>UpdatedCanvas</extent><cumulativeCutLower>0.02</cumulativeCutLower><cumulativeCutUpper>0.98</cumulativeCutUpper></minMaxOrigin>
          <contrastEnhancement><minValue>10</minValue><maxValue>200</maxValue><algorithm>StretchToMinimumMaximum</algorithm></contrastEnhancement></rasterrenderer>"#,
        "",
    );
    let r = s.raster.unwrap();
    assert!(
        r.dra && matches!(r.stretch, Stretch::PercentClip { low, high } if (low - 2.0).abs() < 1e-9 && (high - 2.0).abs() < 1e-9),
        "{r:?}"
    );

    // Hillshade as shaded relief; hue and saturation and contours are noted.
    let (s, report) = read(
        r#"<rasterrenderer type="hillshade" band="1" azimuth="300" angle="35" zfactor="2" multidirection="0"/>"#,
        r#"<huesaturation saturation="-30" grayscaleMode="0" colorizeOn="0" invertColors="0"/>"#,
    );
    assert_eq!(
        s.raster.unwrap().renderer,
        RasterRenderer::ShadedRelief { band: 0, ramp: None, azimuth: 300.0, altitude: 35.0, z_factor: 2.0 }
    );
    assert!(report.notes.iter().any(|n| n.text.contains("saturation")));
    let (s, report) = read(r#"<rasterrenderer type="contour" band="1"/>"#, "");
    assert!(s.raster.is_none() && report.notes.iter().any(|n| n.text.contains("contour")));
    // Nonsense bands and values: nothing, without a crash, and noted.
    for bad in [r#"<rasterrenderer type="singlebandgray" grayBand="0"/>"#, r#"<rasterrenderer type="paletted" band="99999999999"/>"#] {
        let (s, report) = read(bad, "");
        assert!(s.raster.is_none() && report.notes.iter().any(|n| n.text.contains("couldn't read")), "{bad}: {report:?}");
    }
}

/// Every generated style and project in `testdata/qgis` reads, each project with its layers.
#[test]
fn every_corpus_file_reads() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/qgis");
    let files = |dir: &str, extensions: &[&str]| {
        let mut paths: Vec<_> = std::fs::read_dir(root.join(dir)).unwrap().map(|e| e.unwrap().path()).collect();
        paths.retain(|path| path.is_file() && path.extension().and_then(|e| e.to_str()).is_some_and(|e| extensions.contains(&e)));
        paths.sort();
        paths
    };
    // Pin the corpus sizes so a regeneration cannot silently drop fixtures.
    let styles = files("generated/qml", &["qml"]);
    assert_eq!(styles.len(), 90, "style corpus changed: {styles:?}");
    for path in &styles {
        let text = std::fs::read_to_string(path).unwrap();
        let mut report = Report::default();
        read_style(&text, "layer", &mut report).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
    let projects = files("generated/qgs", &["qgs", "qgz"]);
    assert_eq!(projects.len(), 21, "project corpus changed: {projects:?}");
    for path in &projects {
        let bytes = std::fs::read(path).unwrap();
        let text = if path.extension().is_some_and(|e| e == "qgz") {
            let entries = rustgis_data::zip::read_matching_within(&bytes, |n| n.ends_with(".qgs"), 64 << 20).unwrap();
            String::from_utf8(entries.into_iter().next().unwrap().1).unwrap()
        } else {
            String::from_utf8(bytes).unwrap()
        };
        let dir = path.parent().unwrap().to_string_lossy().to_string();
        let mut report = Report::default();
        let project = read_project(&text, &dir, &mut report).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(!project.layers.is_empty(), "{}: no layers", path.display());
        // The PostGIS and SQL Server projects' made-up password reaches neither the layers nor the notes.
        assert!(!format!("{project:?} {report:?}").contains("secret"), "{}: the password came through", path.display());
    }
}
