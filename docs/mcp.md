# MCP Server

`rustgis-cli mcp` runs RustGIS as a [Model Context Protocol](https://modelcontextprotocol.io)
server, so an AI agent such as Claude Code, Claude Desktop, Cursor, VS Code or Codex can make
maps with it. The agent opens data, runs any RustGIS command or geoprocessing tool, reads
attribute tables, looks at the map as an image and looks things up in this documentation.

```sh
rustgis-cli mcp --sample                # the World Atlas sample
rustgis-cli mcp --data parcels.geojson  # your own data (--data more than once, or --project p.rgis)
rustgis-cli mcp --yes                   # also allow file and network access
rustgis-cli mcp --attach [--yes]        # the desktop app that is already open (its MCP Relay)
```

The server talks over standard input and output, so you don't start it yourself: you tell your
agent how to start it (below), and the agent runs it. Each agent gets its own project, held in
memory until the agent saves it.

## Adding it to an agent

Claude Code:

```sh
claude mcp add rustgis -- rustgis-cli mcp --sample
```

Codex:

```sh
codex mcp add rustgis -- rustgis-cli mcp --sample
```

Claude Desktop (`claude_desktop_config.json`), Cursor (`~/.cursor/mcp.json`) and other clients
that use an `mcpServers` list:

```json
{
  "mcpServers": {
    "rustgis": {
      "command": "rustgis-cli",
      "args": ["mcp", "--sample"]
    }
  }
}
```

VS Code (`.vscode/mcp.json`):

```json
{
  "servers": {
    "rustgis": {
      "type": "stdio",
      "command": "rustgis-cli",
      "args": ["mcp", "--sample"]
    }
  }
}
```

If the agent can't find `rustgis-cli`, give its full path (see [Installation](installation.md)).
Then ask, for example:

- "Which 5 countries have the largest GDP? Show them on the map."
- "Buffer the cities by 200 km, colour the result by population and show me the map."
- "Load ~/data/parcels.geojson, label the parcels with their owner and export the map to
  parcels.png." (this reads and writes files, so it needs `--yes`)

## Managing the open desktop app

By default each agent gets its own project in memory. To drive the project that is open in the
desktop app instead, tick **Let your Agents Control RustGIS** in the AI Assistant's settings. It
starts the relay and shows what an agent needs to connect: the MCP server configuration, and the
`claude mcp add` / `codex mcp add` commands. While it's ticked, attached agents may also read and
write files and use the network, without `--yes`. RustGIS remembers the choice and starts the
relay with the app. The **MCP Relay** command (Help menu, or Command Search with Ctrl+K) starts the
relay too, without that: attached agents then have the access their `--yes` asks for.
The relay is off until the box is ticked or that command runs. While it runs, the status bar shows `MCP relay · n attached`,
where n is the number of authenticated clients. For ten seconds after an attached agent runs a
tool, the label also names it (`MCP relay · 1 attached · layer.addSample`, in red when it failed);
hovering shows `Stop MCP relay` and the last call with its parameters, and clicking it stops the
relay. Agents attached through the relay are told, when they connect, that this is the project you
have open and that you watch the map change as they work.

Under it, **Full access** (off unless you tick it, for this session only) lifts every refusal for
agents attached with the relay's own token. They may then also run scripts, tasks and models, and
install, remove and run plugins: what you could do yourself in RustGIS. It isn't remembered: it's
off again at the next launch, and when agents are turned off. While it's on, the status bar reads
`MCP relay · full access · n attached` in red. Agents the app starts itself keep their own grants'
rules.

```sh
rustgis-cli mcp --attach                 # the open app on 127.0.0.1:8767
rustgis-cli mcp --attach 9000 --yes      # another port, with file and network access
rustgis-cli mcp --attach --timeout 300   # wait up to 5 minutes for each call (5 to 600 seconds)
```

The port is 8767 unless the `RUSTGIS_MCP_PORT` environment variable names another, for the app
and for `--attach` alike (a second copy of RustGIS, or a port another program holds). When the
port is taken, the relay's error says so and names that variable; a value that isn't a port number
is an error too, not a quiet fall back to 8767. While the relay runs, the app
publishes `endpoint.json` in the relay's private folder: `schema`, `host`, `port`, `url`, `pid`,
`started_at`, the app `version`, the `token_file` path (not the token) and the `attach` command.
Setup scripts can read it instead of hard-coding the port. It's removed when the relay stops. With
two copies of RustGIS on different ports it describes the one started last, and a crash can leave
it behind: check that its `pid` is still running before trusting it.

```json
{
  "mcpServers": {
    "rustgis-app": {
      "command": "rustgis-cli",
      "args": ["mcp", "--attach", "--yes"]
    }
  }
}
```

Attached calls run on the open app's session through the same tools and refusal policy as the
private server. One more tool works only here: `snapshot` with `mode: "screen"` captures the map as
the window shows it. The relay holds the request until the app has taken the picture, and runs
nothing else meanwhile. File and network commands need `--yes`, unless **Let your Agents Control RustGIS**
is ticked. The session is the project you have open, so an attached agent can't replace it:
`project.new`, `project.open`, `project.openText`, `project.sample` and `project.fromTemplate`
are refused, with or without `--yes`. Each agent command is a step in your own undo history,
named for the agent (`agent 1`, `agent 2`…, in the order they first call, with the relay run, so a
relay started again doesn't take an earlier run's agent for its own). An agent's `edit.undo`
and `edit.redo` work only on its own steps: when the step on top is yours (or another agent's),
they're refused, and they run on their own (`run_command`), not inside a `run` script. You can
still undo anything. Each agent also has its own active layer: when an
agent selects a layer (`layer.select`, or a command that makes a layer active), your selection in
the Layers pane stays as it was. Once the agent has chosen a layer (selected one, or run a command
that made one active), its commands that name no layer act on that layer, whatever you select
meanwhile; until then they act on the one you have selected.

Every tool call an attached agent makes is written to an audit log, one JSON line per call in a
file per day (`2026-10-18.jsonl`) in the `mcp-audit` folder of the app-data folder
(`~/.local/share/rustgis/mcp-audit` on Linux, `~/Library/Application Support/RustGIS/mcp-audit` on
macOS, `%LOCALAPPDATA%\RustGIS\mcp-audit` on Windows). Each line holds `ts`, `source` (`mcp`),
`tool`, `command` or `script` when there is one, `args`, `success`, `duration_s` and `error`.
Passwords, keys and tokens in the arguments, a `run` script's lines included, are replaced before
they are written. On Linux and macOS the folder and files are readable by you only; on Windows they're protected by
your user profile's permissions. A batch's calls share the batch's
`duration_s` and are marked `"batch": true`. An error whose text mentions a credential (a token, a
password, a bearer…) is logged as withheld rather than risk keeping the credential. Days older than 30 are removed.

An agent the app starts itself gets a run-scoped token instead of the relay's own:
`rustgis-cli mcp --attach PORT --token-file FILE`. Each such grant carries its own file and network
permission, and the app revokes it when it's done with the agent; that closes its connections and
removes its file.

### The file bridge

For an agent that can't use MCP, or when the relay's port is taken, the app also runs scripts
written to a file. It's on while **Let your Agents Control RustGIS** is ticked, even if the relay
couldn't start: agents then stay on, and the status bar says only the file bridge is running.

1. Write a script to `bridge-command.txt` in the relay's private folder
   (`$XDG_RUNTIME_DIR/rustgis` on Linux, a `rustgis-mcp-<user>` folder in the temporary folder
   elsewhere). The settings section shows the full path. The script is in the `script.run` format:
   one `command-id {json}` per line, `#` for comments.
2. Once the file has stopped changing for half a second, RustGIS runs it on the open project. (To be
   sure a half-written script never runs, write it to another file in the folder and rename it to
   `bridge-command.txt`.) It runs as one undo step, with
   the same rules as the `run` tool: a refused or failing line stops it, and nothing it did to the
   project is kept.
3. The outcome is written to `bridge-result.json` in the same folder: `{success, result, lines,
   layersAdded, notices, error, duration_s, timestamp}`. The status bar says whether it worked.

Only one RustGIS at a time runs the bridge: a second copy's is refused while the first's runs
(it holds `bridge.lock`). The bridge runs a script each time the file changes. Whatever the file held when the bridge started
isn't run. While the bridge is on, `bridge-ready.json` there holds its `pid`, the two files' paths,
`started_at`, the app `version` and a `schema` number. The folder is readable by this user only,
like the relay's token.

### Relay requirements

- **Off until asked.** Only `ui.mcpRelay` starts a listener: run directly, or by ticking **Let your
  Agents Control RustGIS** (also at launch, when it was left ticked). `ui.mcpRelay` with no parameters
  or with the same port reports the address it already listens on. A different explicit `port`
  is refused until the relay is stopped. `ui.mcpRelay {"stop": true}` stops it and is
  idempotent. The default port is 8767.
- **Loopback only.** `Relay::bind` refuses any non-loopback address before a socket is opened; the
  app's `ui.mcpRelay` binds 127.0.0.1.
- **Private token.** Each run draws 16 bytes from the operating system's random source,
  written as 32 hex characters. The token file is `{port}.token` in `$XDG_RUNTIME_DIR/rustgis`
  when that is an absolute folder, otherwise in a per-user folder under the system temporary
  folder. The folder must be owned by the current user and not accessible to others. The file
  must be a regular file owned by the current user, without group or other access. A file
  that is a symlink is refused. `rustgis-cli mcp --attach` reads it with `read_token(port)`.
  The token never appears on a command line. The file is removed when the relay stops.
- **App-granted access.** `Relay::set_allow_access(true)` lets connections attached with the relay's
  own token use files and the network whether or not they asked; the desktop app sets it while
  **Let your Agents Control RustGIS** is ticked. This includes any process running as the same
  user that can read that token, not just agents launched by RustGIS. It is checked for every call.
  Turning the setting off stops the relay and disconnects its clients, including a relay started
  manually. A failed launch clears the saved setting and reports the error in the app.
- **Grants.** `Relay::grant(access)` writes a run-scoped token to `{port}-{run}-{n}.token` in the
  same private folder (`run` is random per relay run). A grant asked for after the relay stopped is
  refused. A connection that attaches with it gets the grant's permission. Dropping the grant
  removes the file, forgets the token and closes its connections. Stopping the relay removes every
  grant's file.
- **Attach first.** A connection's first non-empty line must be a `rustgis/attach` message
  carrying the token. It must arrive within 5 seconds, measured from connection, and be at most
  4096 bytes including its newline. The token is compared without early exit. A connection that fails
  this is closed. The `access` flag sets what that connection may do; the per-call refusal
  policy still applies. A repeat attach may change `access`.
- **Handshake and idle limits.** Attached clients that send nothing for 300 seconds are closed.
  A reply write that takes longer than 10 seconds closes the connection.
- **Connection caps.** At most 16 open connections, and at most 4 that have not attached. A
  connection beyond either cap is dropped without a reply.
- **Queue cap.** At most 64 messages wait for the app's frame. A request beyond that gets error
  `-32001` (`the app is busy; try again`) if it expects a reply.
- **Bounded request waits.** A request waits up to 60 seconds for the app to run it, or as long
  as the client's attach asks (`timeout_s`, 5 to 600 seconds; `--timeout` on the command line).
  If it is still pending then, it is cancelled and never runs; the client gets error `-32002`. If
  it is already running, the client waits one more interval of the same length. If the reply
  still has not come, the client gets error `-32003` (`its outcome is unknown`), and the command
  may still complete later.
- **Busy app.** When the app has drawn no frame for 10 seconds since a request was queued (a file
  dialog is open, a long tool is running, or the window is minimized or hidden and the system has
  stopped drawing it), the request is cancelled at once and the client gets
  error `-32004` saying RustGIS is busy (apart from the full queue's `-32001`). It never runs, and the connection stays open. An app that
  was only idle before the request isn't busy: the request wakes it. This comes before a long
  `timeout_s`: a stalled app answers after 10 seconds whatever the client's wait, and a client
  waiting less than 10 seconds gets `-32002` instead. A `timeout_s` outside 5 to 600 is clamped to
  that range, and one that isn't a number is ignored (the 60-second default).
- **Stop.** Stopping the relay, or dropping its last owner, closes the listener and every
  client, drops queued messages unrun, and removes the token file. A waiting request ends at
  once, checked every 25 milliseconds.
- **Wake.** A queued message or a change in the attached-client count wakes the UI thread so
  it draws a frame. A relay started before the first frame installs its wake on the first
  logic call after that frame exists.
- **Shared framing.** Each message is one JSON-RPC message per line. After attach, messages and
  replies are read with `read_message_max` at 64 MiB (`MAX_LINE`), the limit counting the newline.
  The attach line is read at most 4096 bytes including its newline (see Attach first). After an
  oversized line on a socket the relay sends a -32700 error and closes that connection. On standard
  input the reader skips the rest of the line and continues. IO errors name neither standard input
  nor a socket.

### Tests

These behavioral tests in `crates/assistant/src/relay.rs` check the requirements above:

- Loopback bind and command run: `relay_runs_a_command_on_the_target_session`
- Token required: `relay_drops_connections_without_the_token`
- Parse error after attach: `relay_returns_parse_error_after_attach_and_stays_usable`
- File commands need `--yes`: `relay_refuses_file_commands_without_yes`
- Separate active layers: `relay_keeps_the_agents_active_layer_apart_from_the_users`
- Agents undo only their own steps: `relay_agents_undo_only_their_own_steps`
- Agents of an earlier relay run aren't this run's: `relay_agents_of_another_run_cant_undo_earlier_steps`
- Busy app: `relay_says_the_app_is_busy_early`
- Client-chosen waits: `relay_waits_as_long_as_the_client_asks`
- Endpoint file and port variable: `relay_publishes_its_endpoint_and_names_the_port_variable`
- Full access, for the relay's own token only: `relay_full_access_lifts_the_refusals_for_its_own_token_only`
- Screen snapshots go to the app: `relay_hands_screen_snapshots_to_the_app`
- Snapshots without the screen: `relay_draws_snapshots_offscreen_without_the_screen`
- Saved snapshots need access: `relay_refuses_a_saved_snapshot_without_access`
- The agent's last call, for the status bar: `relay_records_the_agents_last_call`
- Audit log: `relay_writes_each_call_to_the_audit_log`
- The open project stays the user's: `relay_keeps_project_commands_for_the_user`
- Timed-out command never runs: `relay_cancels_a_timed_out_command_so_it_never_runs`
- Stop cleanup: `relay_stop_closes_clients_and_removes_the_token`
- Last owner drop: `relay_last_owner_drop_closes_the_listener`
- Unauthenticated cap: `relay_caps_unauthenticated_connections`
- Oversized pre-attach line: `relay_refuses_an_oversized_unauthenticated_line`
- Token random and private: `relay_token_is_random_and_private`
- Client count and wake: `relay_counts_authenticated_clients_and_wakes_on_change`
- Stop ends a reply wait: `relay_stop_ends_a_reply_wait_and_the_command_never_runs`
- Running command reported as unknown: `relay_reports_a_running_command_past_its_wait_as_unknown`
- Handshake deadline: `relay_closes_a_trickling_handshake_at_its_deadline`
- App-granted access: `relay_app_lets_agents_use_files_without_yes`
- Grants are scoped and revocable: `relay_grants_are_scoped_and_revocable`


## Tools

| Tool | What it does |
| --- | --- |
| `list_layers` | The whole project in one call: path and unsaved changes, the view (centre, longitude/latitude extent, scale, projection), the active layer, layers in table-of-contents order (group, type, source, extent, feature count, selection, definition query, renderer, labels, fields, visibility), web tile layers, basemap, tables and layouts with their map frames. `name_filter` keeps the layers and tables whose name holds it; `include_fields: false` leaves fields out |
| `view_map` | The map as a PNG image, at the current view |
| `snapshot` | The map as a PNG with JSON metadata: `mode`, `width`, `height`, `extent` (longitude/latitude), `center`, `scale`, `projection` and `render_settled`. `mode: "screen"` is exactly what you see in the RustGIS window (through the relay only), taken once the map has finished drawing or after `wait_s` seconds (default 5, at most 30); `overlays: false` leaves out the map's controls, scale bar, north arrow and sketches. `mode: "render"` draws the same layers, view and styling offscreen at `max_width` (the height follows the view's shape), which works while the window is hidden. `mode: "auto"` (the default) takes the screen when the map is on it. `save_path` also writes the PNG to a file, which needs `--yes` |
| `search_commands` | Finds RustGIS commands by words: their ids, ribbon locations and parameters |
| `run_command` | Runs a command by id with JSON parameters; each run is one undo step. The result names the layers it added (`layersAdded`) and what the command said beside it (`notices`), including any parameters it ignored (`layer.moveDown ignores 'steps' (it reads layer)`) |
| `run` | Runs several commands in one call, as one undo step: a script in the `script.run` format (one `command-id {json}` per line, `#` comments). A failing line stops it, nothing it did to the project is kept, and the error names the line; files an earlier line wrote (an export) stay written. Answers `{success, result, lines, layersAdded, notices, error, duration_s}`, where `result` is the last command's. Each line follows `run_command`'s rules, checked before any line runs |
| `search_tools` | Finds geoprocessing tools by words |
| `describe_tool` | A geoprocessing tool's parameters |
| `run_tool` | Runs a geoprocessing tool; its output is added to the map as a new layer (with `layersAdded` and `notices` as for `run_command`) |
| `read_features` | A layer's attribute rows, optionally only those matching a where clause, sorted |
| `search_docs` | Searches this documentation and returns the best matching sections |
| `read_doc` | Reads a documentation page or section as Markdown; `offset` (1-based line) and `limit` (default 2000 lines) page through long results using `next_offset` |

These are the [AI Assistant](user-guide/assistant.md)'s tools, plus `view_map`. Every page of this documentation is also a resource
(`rustgis://docs/cli.md`, `rustgis://docs/user-guide/maps.md`…) that a client can attach to a
conversation.

## What the agent may do

Your agent asks you before each tool call, and the server adds its own limits:

- Commands that read or write files, connect to an address on the internet or run a plugin's
  command run only when the server was started with `--yes`, as does opening a project from
  text (it could bring in web layers). Without it, the agent can still work with the data you
  gave the server and with the built-in sample, basemaps and geoprocessing. `view_map` draws
  the web tile layers the project you opened already has, fetching their tiles.
- Plugin management, scripts, tasks, models and the GeoLibre runner are never run from here,
  unless you turned on **Full access** for agents in the desktop app. That applies only to agents
  attached to it with the relay's own token.
- In private-server mode, opening, replacing and undoing in the project are allowed: the project
  is the agent's own. With `--attach`, these operations affect the project open in the desktop app.

Installed [plugins](plugins.md) load first, as with the other subcommands (`--no-plugins` skips
them). Notes such as "MCP server ready" go to standard error; standard output carries only the
protocol.


### Local-agent approvals

Local agents launched by the assistant use `Relay::grant_reviewed(auto)`, not an unrestricted
run grant. With automatic approval off, gated tool calls wait in a RustGIS-side queue scoped to
that grant and exact message, including all tool calls in a batch. `--yes` and the general relay
checkbox do not bypass it. An approval cannot revive an expired request; revoking the grant or
stopping the relay cancels pending work. Explicit automatic approval bypasses this extra queue
for that run. Ordinary MCP clients retain the access policy described above.

Stopping the relay through its command or status-bar control also turns off its remembered
agent-control choice. Copyable setup commands are labelled for POSIX shells or PowerShell and
quote the executable path literally; use the matching shell.
