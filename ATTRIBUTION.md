# Attribution

Every asset file in this repository, with its source and licence.

| File | Author / source | Licence |
|---|---|---|
| Sample data (`countries`, `countries_chn`, `cities`, `rivers`, `lakes` GeoJSON), embedded at build time from [rustgis-assets](https://github.com/opengeos/rustgis-assets/tree/main/data) | Natural Earth 1:110m, attributes trimmed; `countries_chn` is Natural Earth's China point of view (1:10m, simplified) | Public domain |
| QGIS screenshot (`qgis-world-atlas.webp` in [rustgis-assets](https://github.com/opengeos/rustgis-assets/tree/main/images)), on the [RustGIS and QGIS](docs/comparison/qgis.md) page | Taken by the RustGIS project: [QGIS](https://qgis.org) 4.2 showing the Natural Earth sample data above | QGIS is GPL-2.0-or-later; the data is public domain |

Raster coordinate systems are looked up by EPSG code in the PROJ definitions of the
[crs-definitions](https://crates.io/crates/crs-definitions) crate (CC0-1.0), compiled into the
app. The codes it lacks (EPSG 9020–20003, `crates/data/src/raster/epsg_gap.rs`) are from the
[EPSG Geodetic Parameter Dataset](https://epsg.org) v12.029 (© IOGP, under its
[terms of use](https://epsg.org/terms-of-use.html)), written as PROJ definitions by PROJ 9.8.

The GeoLibre tool suite ([geolibre-rust](https://github.com/opengeos/geolibre-rust), MIT, built
on [whitebox-wasm](https://github.com/opengeos/whitebox-wasm), MIT) is not part of RustGIS: the
desktop app and command line run its separately downloaded `geolibre` runner, and the web app
loads its WebAssembly runner (the `geolibre-wasm` npm package, from jsDelivr) when asked. Its
tool names, descriptions and categories come from the runner's manifests.

The scientific colour ramps (`crates/map/src/color/scientific.rs`) are sampled from published
colour maps: Viridis, Plasma, Inferno and Magma by Stéfan van der Walt and Nathaniel Smith (CC0),
and Cividis by Jamie R. Nuñez, Christopher R. Anderton and Ryan S. Renslow, as
[matplotlib](https://matplotlib.org) ships them (matplotlib licence); Turbo by Anton Mikhailov,
Copyright 2019 Google LLC (Apache-2.0); Batlow, Vik, Roma and Oleron from Fabio Crameri's
[Scientific colour maps](https://www.fabiocrameri.ch/colourmaps/), Copyright (c) 2020 Fabio
Crameri (MIT, through [cmcrameri](https://github.com/callumrollo/cmcrameri)); and Balance,
Haline, Deep and Topo from [cmocean](https://matplotlib.org/cmocean/), Copyright (c) 2015
Kristen M. Thyng (MIT). The MIT licence's terms: permission is granted, free of charge, to use,
copy, modify, merge, publish, distribute, sublicense and sell copies, provided the copyright
notice and permission notice are included; the software is provided "as is", without warranty
of any kind.

Icons are original vector drawings in code (`crates/ui-egui/src/icons.rs`). Fonts are not
bundled: the interface uses an installed system sans-serif when one is found, otherwise the
Ubuntu Light face that ships inside `epaint_default_fonts` (Ubuntu Font Licence 1.0).
