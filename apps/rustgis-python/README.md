# rustgis

[![PyPI](https://img.shields.io/pypi/v/rustgis.svg)](https://pypi.org/project/rustgis/)
[![Open in Colab](https://colab.research.google.com/assets/colab-badge.svg)](https://colab.research.google.com/github/opengeos/RustGIS/blob/main/apps/rustgis-python/examples/getting-started.ipynb)

Python bindings to [RustGIS](https://rustgis.app), a fast, free and open-source GIS in pure Rust.
The RustGIS engine runs inside Python, headless: open data and projects, run any of RustGIS's
commands and over 1,000 geoprocessing tools, style layers, render maps and save projects the
desktop app opens as they are.

```sh
pip install rustgis            # add [geopandas] for GeoDataFrame in and out
```

```python
import rustgis

s = rustgis.Session(sample=True)                  # the World Atlas sample
buffers = s.run_tool("buffer", input=s["Cities"], distance=100)
s["Countries"].style("graduated", field="pop_est")
s.zoom_to(buffers)
s.to_png("map.png", width=1600, height=900)      # or just `s` in Jupyter
s.save("atlas.rgis")                              # open it in the RustGIS app

cities = s["Cities"]                              # a Layer: count, fields, bounds…
cities.select("pop_max > 10000000")
gdf = cities.to_geopandas(selected=True)          # a GeoDataFrame in EPSG:4326
big = s.add_geopandas(gdf, name="Big cities")     # add_* return the new Layer
big.to_file("big-cities.gpkg")
```

In Jupyter, `rustgis.Map` shows the RustGIS app itself in the cell, in step with its session both
ways (`pip install "rustgis[jupyter]"`):

```python
m = rustgis.Map(sample=True)
m.add_geopandas(gdf, name="Big cities")           # appears in the app
m.add_pmtiles("https://example.org/atlas.pmtiles", name="Atlas")
m                                                 # pan, edit, style there...
m.info()["center"]                                # ...and read it back here
```

Every action in RustGIS is a command with an id, the same ids the ribbon, `rustgis-cli` and the
AI Assistant use: `Session.commands()` lists them with their parameters, `Session.describe(id)`
shows one, and `Session.run` runs any of them. See the [Python guide](https://rustgis.app/python/) and the
[getting-started notebook](https://github.com/opengeos/RustGIS/blob/main/apps/rustgis-python/examples/getting-started.ipynb).

Licensed under Apache-2.0.
