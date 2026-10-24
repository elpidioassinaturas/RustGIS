# Command Line

`rustgis-cli` runs RustGIS engine commands headlessly, using the same ids as the app.
Window and pane commands (`ui.*`) belong to the app and are not available in a private CLI session.

```sh
rustgis-cli commands                  # every command id with its parameters
rustgis-cli tools                     # geoprocessing tools
rustgis-cli info data.geojson         # project and layer summary as JSON
rustgis-cli render data.geojson map.png --basemap ocean --width 1600 --height 900
rustgis-cli render data.geojson map.png --tiles 'https://tile.openstreetmap.org/{z}/{x}/{y}.png'
rustgis-cli logo icon.png --size 256  # the app icon as a PNG
rustgis-cli plugins                   # installed plugins (also: browse, get, install, uninstall…)
rustgis-cli skills                    # the AI Assistant's skills (also: add, remove, enable, disable)
```

`commands` lists the engine commands and their JSON parameters; `tools` lists geoprocessing
tools and their parameters. See [Installation](installation.md) for the CLI packages.

## Rendering

`render <data|project> <out.png>` opens a data file or saved project and exports its map.
It defaults to 1600 × 900 pixels at 96 DPI; `--width`, `--height` and `--dpi` change those.
A data file is zoomed to its full extent; a project keeps its saved view.
`--tiles` accepts an XYZ template, vector tiles or a MapLibre-style address.

`--worldview auto|default|china` chooses the disputed borders in the built-in countries and
world outline, for `render`, `run` and the other subcommands. `auto` follows
`RUSTGIS_WORLDVIEW`, else the system time zone and locale.

## Chaining commands

`run` starts with an empty project. `--sample` loads the World Atlas sample, `--project <path>`
opens a project, and repeatable `--data <path>` adds files, in that order. Repeat `--cmd` to run
commands in order; `--save <path>` saves the resulting project:

```sh
rustgis-cli run --sample \
  --cmd 'gp.run={"tool":"countPoints","params":{"polygons":"Countries","points":"Cities"}}' \
  --cmd 'layer.symbology={"layer":"Countries_Summarize","type":"graduated","field":"point_count"}' \
  --cmd 'bookmark.go={"name":"Europe"}' \
  --cmd 'share.exportMap={"path":"europe.png","dpi":192}' \
  --save atlas.rgis
```

Each non-null command result goes to standard output as JSON. `--cmd 'id'` runs a command with
no parameters; otherwise use `--cmd 'id={json params}'`. What a command reports beside its result,
which the app shows in its status bar, goes to standard error.

Nothing waits on `run` the way a window does, so a long run takes the bounds the app's panes give
it on a thread: a trend over a data cube (`cube.trend`) may compare up to 20 billion pairs, ten
times what the Script pane allows, and a trend's or a summary's STAC search up to 10,000 items,
where the app takes 2,000. A trend or a summary over time (`cube.reduce`) prints its progress to
standard error (`cube.trend: 42%`; a line every tenth when standard error isn't a terminal).
`ask` and `mcp` keep the Script pane's bounds, as an assistant or an MCP client waits on them, and
so does Python. See [Trend analysis](user-guide/analysis.md).

Installed [plugins](plugins.md) load first (`--no-plugins` skips them), and `--plugin <file or
folder>` (repeatable) loads one more for this run only. Their commands and tools have ids like
`plugin:vector-toolkit/hexagon-grid`:

```sh
rustgis-cli run --sample --plugin vector-toolkit-0.1.0.rgplugin \
  --cmd 'gp.run={"tool":"plugin:vector-toolkit/hexagon-grid","params":{"input":"Countries","size":300}}' \
  --cmd 'share.exportMap={"path":"hexagons.png"}'
```

`plugins install` and `plugins get` also prepare a Python plugin's environment;
`plugins prepare <id>` prepares it on demand, reusing it if it is already ready.
`--registry <URL>` points `plugins browse` and `plugins get` at another registry.

Rasters work the same way:

```sh
rustgis-cli render dem.tif dem.png
rustgis-cli run --data dem.tif \
  --cmd 'raster.symbology={"type":"shadedRelief","ramp":"Terrain","zFactor":1.5}' \
  --cmd 'raster.info={}' \
  --cmd 'share.exportMap={"path":"relief.png","dpi":192}'
```

<figure markdown="span">
  ![A map of Europe exported by rustgis-cli](https://assets.rustgis.app/images/cli-export.webp)
  <figcaption>A map exported by <code>rustgis-cli</code>, with no window open.</figcaption>
</figure>

## Asking the AI assistant

`ask` gives the [AI Assistant](user-guide/assistant.md) one request. It loads data like `run`,
then the model answers and acts on the project through commands; `--save` keeps the result:

```sh
export ANTHROPIC_API_KEY=…            # or OPENAI_API_KEY, GEMINI_API_KEY, OPENROUTER_API_KEY, …
rustgis-cli ask --sample "Which 5 countries have the largest GDP?"
rustgis-cli ask --sample "Buffer the cities by 200 km and label them" --save buffered.rgis
rustgis-cli ask --data parcels.geojson --provider ollama --model qwen3:8b "Summarise the fields"
rustgis-cli ask --models --provider openai          # the provider's models
rustgis-cli ask --login                             # sign in with ChatGPT to use your ChatGPT plan
rustgis-cli ask --sample --provider chatgpt "Zoom to Tokyo"
```

The provider is `--provider` (`anthropic`, `openai`, `chatgpt`, `gemini`, `openrouter`, `ollama`, `custom`),
else `RUSTGIS_ASSISTANT_PROVIDER`, else the first with a key in the environment, else Ollama (a
ChatGPT sign-in is used only when chosen).
`--model` (or `RUSTGIS_ASSISTANT_MODEL`) and `--base-url` override the defaults. The answer goes to
standard output and each command the assistant ran to standard error. Commands that read or write
files or use the network are declined unless you pass `--yes`; plugin commands, scripts, tasks,
models and replacing the project are always declined here. `--trace log.txt` appends each request
and response body (never the headers, which carry the key) to a file; the bodies can contain your
data and prompts. `--login` prints a browser sign-in URL so `--provider chatgpt` uses your ChatGPT
plan. `--another-account` starts a new registration, `--accounts` lists saved registrations,
`--account <client-id>` selects one, and `--logout` signs out; see
[Signing in with ChatGPT](user-guide/assistant.md#signing-in-with-chatgpt).

## Serving AI agents

`mcp` runs RustGIS as a Model Context Protocol server for agents such as Claude Code, Cursor and
VS Code, which then run commands, look at the map and search the documentation themselves:

```sh
claude mcp add rustgis -- rustgis-cli mcp --sample
```

To use the already-open desktop project instead, enable its MCP Relay and register
`rustgis-cli mcp --attach`. Do not combine `--attach` with `--sample`, `--project` or `--data`.
File and network commands require `--yes` unless the app's agent-control setting grants access.

`mcp-register --client claude` registers the open desktop app's MCP over HTTP with Claude Code,
for when agents are turned on in the app. Without `--client`, it prints the configuration;
`--name` (default `rustgis-live`) and `--port` (default `RUSTGIS_MCP_HTTP_PORT`, else 8768)
change the entry:

```sh
rustgis-cli mcp-register --client claude
```

See [MCP Server](mcp.md).
