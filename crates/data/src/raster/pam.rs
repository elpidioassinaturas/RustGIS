//! GDAL's auxiliary metadata (PAM): the `.aux.xml` file GDAL writes next to a raster to hold what
//! the file itself doesn't, such as band descriptions, no-data values, class names and computed
//! statistics.
//!
//! ```xml
//! <PAMDataset>
//!   <PAMRasterBand band="1">
//!     <Description>Blue</Description>
//!     <NoDataValue>0</NoDataValue>
//!     <CategoryNames>
//!       <Category>no data</Category>
//!       <Category>water</Category>
//!     </CategoryNames>
//!     <Metadata>
//!       <MDI key="STATISTICS_MINIMUM">12</MDI>
//!       <MDI key="STATISTICS_MAXIMUM">2400</MDI>
//!     </Metadata>
//!   </PAMRasterBand>
//! </PAMDataset>
//! ```

use std::path::{Path, PathBuf};

use roxmltree::ParsingOptions;

/// Largest sidecar read (16 MiB): real ones hold a few histograms at most.
const MAX_BYTES: u64 = 16 << 20;
/// Most bands described.
const MAX_BANDS: usize = 65_536;
/// Most class names read for a band: one for each value of a 16-bit band.
const MAX_CATEGORIES: usize = 65_536;

/// Statistics GDAL stored for a band (`STATISTICS_*`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub min: f64,
    pub max: f64,
    pub mean: Option<f64>,
    pub std_dev: Option<f64>,
}

/// What a sidecar says about one band.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PamBand {
    /// The band's number, from 1.
    pub band: usize,
    pub description: Option<String>,
    pub nodata: Option<f64>,
    pub stats: Option<Stats>,
    /// The names of the band's classes, by value (GDAL's category names): `categories[v]` names
    /// value `v`, and is empty when that value has none. Empty when the band names no class.
    pub categories: Vec<String>,
}

/// What a sidecar says about a raster's bands.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pam {
    pub bands: Vec<PamBand>,
}

impl Pam {
    /// The band numbered `band` (from 1).
    pub fn band(&self, band: usize) -> Option<&PamBand> {
        self.bands.iter().find(|b| b.band == band)
    }

    /// The no-data value the bands that give one agree on. A raster holds one no-data value for
    /// all its bands, so bands giving different ones give none.
    pub fn nodata(&self) -> Option<f64> {
        let mut values = self.bands.iter().filter_map(|b| b.nodata);
        let first = values.next()?;
        values.all(|v| v == first).then_some(first)
    }
}

/// The sidecars GDAL would read for `path`: `x.tif.aux.xml`, then `x.aux.xml`.
pub fn sidecar_paths(path: &Path) -> Vec<PathBuf> {
    let mut long = path.as_os_str().to_owned();
    long.push(".aux.xml");
    vec![PathBuf::from(long), path.with_extension("aux.xml")]
}

/// The sidecar next to `path`, if there's one that parses.
pub fn read(path: &Path) -> Option<Pam> {
    sidecar_paths(path).into_iter().find_map(|p| {
        let meta = std::fs::metadata(&p).ok()?;
        if !meta.is_file() || meta.len() > MAX_BYTES {
            return None;
        }
        parse(&std::fs::read_to_string(&p).ok()?)
    })
}

/// Parses a sidecar's text (`None` when it isn't a PAM document).
pub fn parse(text: &str) -> Option<Pam> {
    let opts = ParsingOptions { allow_dtd: false, nodes_limit: crate::io::XML_NODES, ..ParsingOptions::default() };
    let doc = crate::xml::parse(text, opts).ok()?;
    let root = doc.root_element();
    if root.tag_name().name() != "PAMDataset" {
        return None;
    }
    let mut pam = Pam::default();
    for b in root.children().filter(|n| n.has_tag_name("PAMRasterBand")) {
        let Some(band) = b.attribute("band").and_then(|v| v.trim().parse::<usize>().ok()).filter(|n| (1..=MAX_BANDS).contains(n)) else {
            continue;
        };
        let text_of = |tag: &str| b.children().find(|n| n.has_tag_name(tag)).and_then(|n| n.text()).map(str::trim);
        let description = text_of("Description").filter(|d| !d.is_empty()).map(|d| d.chars().take(256).collect());
        let nodata = text_of("NoDataValue").and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite());
        // Only the default (unnamed) metadata domain holds the statistics.
        let mdi = |key: &str| {
            b.children()
                .filter(|n| n.has_tag_name("Metadata") && n.attribute("domain").is_none_or(str::is_empty))
                .flat_map(|m| m.children())
                .find(|n| n.has_tag_name("MDI") && n.attribute("key") == Some(key))
                .and_then(|n| n.text())
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite())
        };
        let stats = match (mdi("STATISTICS_MINIMUM"), mdi("STATISTICS_MAXIMUM")) {
            (Some(min), Some(max)) if min <= max => {
                Some(Stats { min, max, mean: mdi("STATISTICS_MEAN"), std_dev: mdi("STATISTICS_STDDEV").filter(|s| *s >= 0.0) })
            }
            _ => None,
        };
        let mut categories: Vec<String> = b
            .children()
            .find(|n| n.has_tag_name("CategoryNames"))
            .into_iter()
            .flat_map(|c| c.children().filter(|n| n.has_tag_name("Category")))
            .take(MAX_CATEGORIES)
            .map(|n| n.text().map(str::trim).unwrap_or_default().chars().take(256).collect())
            .collect();
        // Values past the last named one have no name either.
        while categories.last().is_some_and(String::is_empty) {
            categories.pop();
        }
        pam.bands.push(PamBand { band, description, nodata, stats, categories });
    }
    Some(pam)
}

/// The band descriptions a GeoTIFF holds itself, in GDAL's metadata tag (`GDAL_METADATA`, 42112),
/// where GDAL and rasterio write them: `None` when it isn't such a document or describes no band.
/// Its `sample` counts from 0.
///
/// ```xml
/// <GDALMetadata>
///   <Item name="DESCRIPTION" sample="0" role="description">NDVI 2024-06-01</Item>
/// </GDALMetadata>
/// ```
pub fn parse_gdal_metadata(text: &str) -> Option<Pam> {
    let opts = ParsingOptions { allow_dtd: false, nodes_limit: crate::io::XML_NODES, ..ParsingOptions::default() };
    let doc = crate::xml::parse(text, opts).ok()?;
    let root = doc.root_element();
    if root.tag_name().name() != "GDALMetadata" {
        return None;
    }
    let mut pam = Pam::default();
    // The bands described so far, so a long document isn't searched again for each item.
    let mut seen = std::collections::HashSet::new();
    for item in root.children().filter(|n| n.has_tag_name("Item") && n.attribute("role") == Some("description")) {
        let band = item
            .attribute("sample")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .and_then(|s| s.checked_add(1))
            .filter(|n| *n <= MAX_BANDS);
        let description = item.text().map(str::trim).filter(|d| !d.is_empty()).map(|d| d.chars().take(256).collect::<String>());
        // A band's first description, if it is given twice.
        if let (Some(band), Some(description)) = (band, description)
            && seen.insert(band)
        {
            pam.bands.push(PamBand { band, description: Some(description), ..PamBand::default() });
        }
    }
    (!pam.bands.is_empty()).then_some(pam)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_band_descriptions_nodata_and_statistics() {
        let xml = r#"<PAMDataset>
  <SRS>GEOGCS["WGS 84"]</SRS>
  <PAMRasterBand band="2"><Description>Green</Description></PAMRasterBand>
  <PAMRasterBand band="1">
    <Description> Blue </Description>
    <NoDataValue>-9999</NoDataValue>
    <Metadata domain="IMAGE_STRUCTURE"><MDI key="STATISTICS_MINIMUM">999</MDI></Metadata>
    <Metadata>
      <MDI key="STATISTICS_MINIMUM">1</MDI>
      <MDI key="STATISTICS_MAXIMUM">250.5</MDI>
      <MDI key="STATISTICS_MEAN">80</MDI>
      <MDI key="STATISTICS_STDDEV">nan</MDI>
    </Metadata>
  </PAMRasterBand>
  <PAMRasterBand band="0"><Description>none</Description></PAMRasterBand>
  <PAMRasterBand band="3"><NoDataValue>nan</NoDataValue><Description></Description></PAMRasterBand>
</PAMDataset>"#;
        let pam = parse(xml).unwrap();
        assert_eq!(pam.bands.len(), 3, "band 0 isn't a band");
        let b1 = pam.band(1).unwrap();
        assert_eq!(b1.description.as_deref(), Some("Blue"));
        assert_eq!(b1.nodata, Some(-9999.0));
        assert_eq!(b1.stats, Some(Stats { min: 1.0, max: 250.5, mean: Some(80.0), std_dev: None }));
        assert_eq!(pam.band(2).unwrap().description.as_deref(), Some("Green"));
        assert_eq!((pam.band(3).unwrap().description.as_ref(), pam.band(3).unwrap().nodata), (None, None));
        assert_eq!(pam.nodata(), Some(-9999.0));
        let mut differ = pam.clone();
        differ.bands.push(PamBand { band: 4, nodata: Some(0.0), ..Default::default() });
        assert_eq!(differ.nodata(), None, "bands disagree");
    }

    #[test]
    fn reads_class_names() {
        // As GDAL writes them (SetCategoryNames): one per value from 0, empty for a value with no
        // name.
        let xml = r#"<PAMDataset>
  <PAMRasterBand band="1">
    <CategoryNames>
      <Category>no data</Category>
      <Category> 0-5 % flat </Category>
      <Category></Category>
      <Category>over 100 % (over 45°)</Category>
      <Category/>
    </CategoryNames>
  </PAMRasterBand>
  <PAMRasterBand band="2"><CategoryNames><Category></Category></CategoryNames></PAMRasterBand>
  <PAMRasterBand band="3"><Description>none</Description></PAMRasterBand>
</PAMDataset>"#;
        let pam = parse(xml).unwrap();
        assert_eq!(pam.band(1).unwrap().categories, ["no data", "0-5 % flat", "", "over 100 % (over 45°)"]);
        assert!(pam.band(2).unwrap().categories.is_empty(), "no value is named");
        assert!(pam.band(3).unwrap().categories.is_empty());
        let long = format!(
            "<PAMDataset><PAMRasterBand band=\"1\"><CategoryNames><Category>{}</Category></CategoryNames></PAMRasterBand></PAMDataset>",
            "x".repeat(1000)
        );
        assert_eq!(parse(&long).unwrap().band(1).unwrap().categories[0].len(), 256);
    }

    #[test]
    fn junk_is_not_a_sidecar() {
        for text in ["", "not xml", "<GDALMetadata/>", "<PAMDataset><PAMRasterBand band=\"x\"/>", "<!DOCTYPE x><PAMDataset/>"] {
            assert!(parse(text).is_none_or(|p| p.bands.is_empty()), "{text}");
        }
        let deep = format!("<PAMDataset>{}{}</PAMDataset>", "<a>".repeat(100_000), "</a>".repeat(100_000));
        assert!(parse(&deep).is_none());
    }

    #[test]
    fn reads_band_descriptions_from_gdal_metadata() {
        let xml = r#"<GDALMetadata>
  <Item name="AREA_OR_POINT">Area</Item>
  <Item name="DESCRIPTION" sample="1" role="description"> NDVI 2024-07-01 </Item>
  <Item name="DESCRIPTION" sample="0" role="description">NDVI 2024-06-01</Item>
  <Item name="DESCRIPTION" sample="0" role="description">again</Item>
  <Item name="SCALE" sample="0" role="scale">0.0001</Item>
  <Item name="DESCRIPTION" sample="x" role="description">no band</Item>
  <Item name="DESCRIPTION" sample="18446744073709551615" role="description">past the last band</Item>
  <Item name="DESCRIPTION" sample="65536" role="description">past the last band</Item>
  <Item name="DESCRIPTION" sample="2" role="description"></Item>
</GDALMetadata>"#;
        let pam = parse_gdal_metadata(xml).unwrap();
        assert_eq!(pam.bands.len(), 2, "{pam:?}");
        assert_eq!(pam.band(1).unwrap().description.as_deref(), Some("NDVI 2024-06-01"));
        assert_eq!(pam.band(2).unwrap().description.as_deref(), Some("NDVI 2024-07-01"));
        assert_eq!(pam.nodata(), None);
        for text in ["", "not xml", "<GDALMetadata/>", "<PAMDataset/>", "<GDALMetadata><Item name=\"A\">1</Item></GDALMetadata>"] {
            assert!(parse_gdal_metadata(text).is_none(), "{text}");
        }
    }

    #[test]
    fn sidecar_names() {
        let p = sidecar_paths(Path::new("/d/scene.tif"));
        assert_eq!(p, vec![PathBuf::from("/d/scene.tif.aux.xml"), PathBuf::from("/d/scene.aux.xml")]);
    }
}
