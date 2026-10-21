# Acknowledgements

RustGIS stands on the work of many projects and their authors. Thank you to all of them. If your
work is missing here, please [open an issue](https://github.com/opengeos/RustGIS/issues).

!!! note "Disclaimer"

    RustGIS is primarily built with [Claude Opus 5.5](https://www.anthropic.com/claude), an AI
    model by Anthropic, working under human direction and review. Its results are tested, but check
    them for your own use, and please [report](https://github.com/opengeos/RustGIS/issues) anything
    that looks wrong.

## The GIS that came before

[QGIS](https://qgis.org) and [ArcGIS Pro](https://www.esri.com/en-us/arcgis/products/arcgis-pro/overview)
set the expectations of a desktop GIS that RustGIS is measured against, and [GDAL](https://gdal.org)
those of reading geospatial formats. [MapLibre](https://maplibre.org) defines the style
specification RustGIS draws vector tiles with, and [PROJ](https://proj.org) the coordinate
operations it follows.

## Geoprocessing

The optional GeoLibre tool suite ([geolibre-rust](https://github.com/opengeos/geolibre-rust),
built on [whitebox-wasm](https://github.com/opengeos/whitebox-wasm)) brings more than 1,000 raster,
vector, LiDAR and hydrology tools, after John Lindsay's
[WhiteboxTools](https://www.whiteboxgeo.com).

## Data

- [Natural Earth](https://www.naturalearthdata.com) (public domain) for the sample data.
- The [EPSG Geodetic Parameter Dataset](https://epsg.org) of IOGP, PROJ's database and Esri's
  [projection-engine-db-doc](https://github.com/Esri/projection-engine-db-doc) for coordinate
  systems.
- [OpenFreeMap](https://openfreemap.org), whose free vector tiles and styles (such as Liberty)
  draw RustGIS's vector basemaps.
- [OpenStreetMap](https://www.openstreetmap.org) and its contributors,
  [Overture Maps](https://overturemaps.org) and [Protomaps](https://protomaps.com) for basemaps
  and open map data.
- The scientific colour ramps of matplotlib (Viridis and family, Cividis), Google (Turbo), Fabio
  Crameri's [Scientific colour maps](https://www.fabiocrameri.ch/colourmaps/) and
  [cmocean](https://matplotlib.org/cmocean/).

[ATTRIBUTION.md](https://github.com/opengeos/RustGIS/blob/main/ATTRIBUTION.md) lists every asset
with its source and licence.

## Rust crates

RustGIS is pure Rust, built on these crates, among others:

| Crate | For |
|---|---|
| [egui and eframe](https://github.com/emilk/egui), [wgpu](https://wgpu.rs) | The interface, natively and in the browser, and the GPU map |
| [tiny-skia](https://github.com/linebender/tiny-skia), [lyon](https://github.com/nical/lyon), [ab_glyph](https://crates.io/crates/ab_glyph), [image](https://github.com/image-rs/image) | Drawing, tessellation, text and images |
| [geo](https://github.com/georust/geo), [geozero](https://github.com/georust/geozero), [shapefile](https://crates.io/crates/shapefile), [a5](https://crates.io/crates/a5) | Geometry, formats and the A5 grid |
| [proj4rs](https://crates.io/crates/proj4rs), [crs-definitions](https://crates.io/crates/crs-definitions) | Coordinate systems |
| [tiff](https://github.com/image-rs/image-tiff), [laz](https://crates.io/crates/laz) | GeoTIFF and LAZ |
| [Apache Arrow and Parquet](https://github.com/apache/arrow-rs), [flatgeobuf](https://crates.io/crates/flatgeobuf), [rusqlite](https://github.com/rusqlite/rusqlite) ([SQLite](https://sqlite.org)), [calamine](https://crates.io/crates/calamine), [csv](https://crates.io/crates/csv), [roxmltree](https://crates.io/crates/roxmltree) | GeoParquet, FlatGeobuf, GeoPackage, spreadsheets, CSV and XML |
| [postgres](https://crates.io/crates/postgres), [mysql](https://crates.io/crates/mysql), [tiberius](https://crates.io/crates/tiberius), [rustls](https://github.com/rustls/rustls) | Databases |
| [miniz_oxide](https://crates.io/crates/miniz_oxide), [brotli](https://crates.io/crates/brotli), [ruzstd](https://crates.io/crates/ruzstd), [lz4_flex](https://crates.io/crates/lz4_flex), [zip](https://crates.io/crates/zip) | Compression |
| [wasmi](https://github.com/wasmi-labs/wasmi) | Sandboxed WebAssembly plugins |
| [ureq](https://crates.io/crates/ureq), [serde](https://serde.rs) | HTTP and JSON |
| [PyO3](https://pyo3.rs), [wasm-bindgen](https://github.com/rustwasm/wasm-bindgen), [rfd](https://crates.io/crates/rfd) | The Python package, the web app and file dialogs |

The full list of dependencies is in `Cargo.toml` and `Cargo.lock`.
