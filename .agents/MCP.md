# MCP server

The editor hosts a Model Context Protocol server. An external AI client (Claude Code, an IDE)
connects to the running editor and calls its tools: build and run the game, read the
engine's log, list tasks, threads, problems and plugins, and drive the open level through the
same tools the editor's chat panel uses.

Code: `crates/core/pulsar_mcp` (the mechanism, no tool in it), `crates/editor/ui_mcp` (the editor's
host: lifecycle, the existing AI tools, tools that describe the editor), and the tools themselves in
the crates that own the systems they touch (`ui_build`, `ui_log_viewer`, `ui_core`).

## Protocol and transport

- **Streamable HTTP**, request/response half: a client `POST`s one JSON-RPC message (or a batch) to
  `http://127.0.0.1:<port>/mcp` and reads the JSON answer (`202` for a notification). `GET` and
  `DELETE` on `/mcp` answer `405`: the server has no stream to offer and no session to end. It is
  stateless (no `Mcp-Session-Id`), speaks protocol revisions 2025-06-18, 2025-03-26 and 2024-11-05, and
  implements `initialize`, `ping`, `tools/list` and `tools/call` (`tools/list` is not paginated and
  `listChanged` is false: ask again).
- No stdio transport. A client that only speaks stdio can use an HTTP bridge (`mcp-remote`).
- Why not push notifications: watching is a **cursor-based tool**, `logs_wait` (below), which works with
  every client.
- Tool results carry text, `structuredContent` (the JSON), and images (`type: "image"`, base64).
  A tool's own failure is `isError: true` with what to do instead; a malformed call is a JSON-RPC
  error. A missing required argument is reported by name before the tool runs.

## Security

- Binds `127.0.0.1` only (never all interfaces).
- **Bearer token**, 256 random bits, generated per server start and written with the URL to
  `<app data>/mcp/endpoint.json` (Windows: `%APPDATA%\Pulsar\Pulsar_Engine\data\mcp\endpoint.json`),
  not into the project (projects are under version control). The file also holds a ready-made
  `.mcp.json` entry. It is deleted when the server stops. Compared in constant time; `401` otherwise.
- **DNS-rebinding defence**: the `Host` header must be `127.0.0.1`, `localhost` or `[::1]` with the
  server's own port, and an `Origin`, when sent, must be a loopback origin (`403` otherwise).
- Tools carry **annotations** (`readOnlyHint`, `destructiveHint`, `idempotentHint`; `openWorldHint` is
  always false). `build_stop`, deleting objects, saving and the Blueprint `remove_*` tools are
  destructive. The AI tools that were written without annotations get them from the verb in the name
  (`ui_mcp/src/ai_tools.rs`: list/get/query/describe read, delete/remove/save write destructively,
  anything else writes).
- **Off by default.** Editor Settings > MCP Server: `Enable MCP Server` and `Port` (default 47832),
  watched every two seconds (`editor / mcp`, `pulsar_settings::editor::mcp`). `PULSAR_MCP=1` turns it on
  for one editor session and `PULSAR_MCP_PORT` picks the port (headless runs, CI).
- Runs as editor services with heartbeats (`MCP server` and four `MCP worker N` in the Tasks window
  and Threads view): no raw threads.

## Connecting a client

Start the editor with the server on, then read the endpoint file:

```json
{ "url": "http://127.0.0.1:47832/mcp", "token": "<64 hex>", "pid": 1234,
  "client_config": { "mcpServers": { "pulsar": { "type": "http", "url": "http://127.0.0.1:47832/mcp",
                     "headers": { "Authorization": "Bearer <64 hex>" } } } } }
```

Copy `client_config` into the project's (or your user's) `.mcp.json`. The token changes with each
server start, so refresh it then. Claude Code: `claude mcp add --transport http pulsar <url> --header
"Authorization: Bearer <token>"`. Without a client, any HTTP tool works:

```powershell
$e = Get-Content "$env:APPDATA\Pulsar\Pulsar_Engine\data\mcp\endpoint.json" | ConvertFrom-Json
Invoke-RestMethod $e.url -Method Post -Headers @{ Authorization = "Bearer $($e.token)" } `
  -ContentType application/json `
  -Body '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"build_status","arguments":{}}}'
```

## How a tool is registered

One registration, `pulsar_mcp::McpToolRegistration` (category `mcp.tools`), carries `origin`, `name`,
`title`, `description`, `input_schema` (JSON text), `annotations` and `call: fn(&str) -> ToolResult`
(arguments as JSON text in, a `ToolResult` of strings out). Everything crossing the boundary is plain std
data so a plugin library built with other `serde_json` features cannot disagree about a layout.

- **The engine's crates** submit it with `pulsar_plugins::inventory::submit!` and
  `origin: pulsar_plugins::engine_origin!()`.
- **A plugin** adds it from its root crate's `build`: `p.add(McpToolRegistration { origin:
  pulsar_plugins::origin!(), .. })`. It then crosses the library boundary like every other contribution
  (`pulsar_plugins` attach, layout fingerprint), is gated by the plugin being enabled and appears in
  `tools/list` with no code in the server. Name it with its kind as a prefix (`voxel_bake`).
- **Tools that exist only while something runs** (the editor's existing AI tools) come from a
  `pulsar_mcp::ToolProvider`: `ui_mcp` adds one that lists `PluginManager::build_tool_bridge()` (every
  loaded plugin's `ai_tools()` and every built-in provider's, so the level editor's `level_editor_*`,
  the Blueprint editor's `blueprint_*`, the code editor's `code_editor_*`) and runs a call through the
  bridge, so the chat panel and an external client share one set of tools. Each gets an optional
  `file_path` argument (which open editor the call is for; omitted, the open editor).
- A tool runs on a server worker, never the UI thread. One that needs the UI thread (start a build
  with its window and notifications, open an editor tab) hops there with
  `ui_common::ui_call::call(timeout, |cx: &mut App| ..)`.
- The layer gate holds: `pulsar_mcp` and `ui_mcp` are kernel crates and name no plugin or kind; the log
  tools sit in `ui_log_viewer` (a tool-layer crate) because the log store is there.

## Tool catalogue

| Tool | Source | What |
|---|---|---|
| `build_configurations`, `build_set_mode`, `build_start`, `build_stop`, `build_status`, `build_output` | `ui_build/src/mcp.rs` | The toolbar's Build menu. Select a configuration or set its profile (debug, release, shipping); start a build (`run` true: build and launch the game, false: build only); cancel it and stop the game; status (outcome, step, percent, task id, Problems-panel failures); the build task's or the game's output with a cursor |
| `logs_query`, `logs_wait` | `ui_log_viewer/src/mcp.rs` | The live log (last 20000 lines, the Mission Control buffer). Filters: min level, target, text, exclude text, editor task id or title, `since`/`until`, `since_seconds_ago`; `tail`; `after_cursor` and `next_cursor`. `logs_wait` returns when a matching line arrives after the cursor or at `timeout_seconds` |
| `tasks_list`, `threads_list`, `problems_list`, `plugins_list` | `ui_mcp/src/inspect.rs` | The Tasks window's tasks and threads, the Problems panel's findings, and every installed plugin: whether its library loaded, whether the project enables it, what it contributes, the plugin directory in use, and for each library file what loading did (loaded, or failed with the exact reason, and which needed its imports renamed for a shared library of another build) |
| `profile_status`, `profile_start`, `profile_stop`, `profile_list_recordings`, `profile_query_sql`, `profile_top_scopes`, `profile_frame_times`, `profile_frame_breakdown`, `profile_threads`, `profile_gpu`, `profile_latency`, `profile_gaps`, `profile_compare`, `profile_export` | `ui_flamegraph/src/mcp.rs` | The profiler: record the running game or this editor with the Flamegraph window's recorder, read the recording (a SQLite file) through canned read-only queries. See "Profiling" below |
| `editor_open_file` | `ui_core/src/app/mcp_tools.rs` | Open a file or class folder in its editor tab (what double-clicking in the Content browser does) |
| `viewport_screenshot` | `ui_level_editor/src/ai/screenshot_tool.rs` | A still of the level viewport as a .png or .jpg file, no dialog: the High res screenshot's tiled, scene-time-frozen frame at `width` x `height` (default 2560 x 1440, clamped to 16..16384), the grid, the gizmo and the editor's overlay lines (spline curves, volume bounds) hidden unless `hide_grid`, `hide_gizmos`, `hide_overlays` say false. Returns path, size, format, bytes, tiles and milliseconds. See "Screenshots" below |
| `level_editor_*`, `blueprint_*`, `code_editor_*` | existing AI tools, through the provider | Objects, components, hierarchy, selection, transforms, spawn and delete, undo and redo, save, play control, Blueprint graph editing, code editing |

Reading the build while it runs: `build_start`, then `build_status` and `logs_wait` with
`task_title: "Build"` (and `"Game"` for the launched game: its stdout and stderr are the `Game: Run`
task's log).

## Profiling

The tools in `ui_flamegraph/src/mcp.rs` are a thin layer over the engine's profiler
(`crates/external/pulsar-profiling`, crate `profiling`), not a second one.

**Recording.** Every instrumented scope (`profiling::profile_scope!`) costs one atomic load while
nothing records. A process is recordable when it publishes itself (`profiling::remote`): the editor
once a project is open, a game launched by Build & Run (the editor sets `PULSAR_PROFILE=1`; a game
started by hand needs the variable or `--pulsar-profile`). Publishing maps a file
`<temp>/.../<pid>.pprof` holding a 32 MiB event ring; the viewer claims it, the process starts
streaming, and the viewer drains it. `ui_flamegraph::recorder` is that viewer for both the Flamegraph
window's Record button and `profile_start`/`profile_stop` (one recording at a time per editor, whoever
started it): an `InstrumentationCollector` (this editor's own queue, or another process through the ring)
and, at stop, every event written to
`<project>/.pulsar/profiling/flamegraph/flamegraph_<YYYY-MM-DD_HH-MM-SS>.db` (`profiling::database`).
A remote recording keeps at most 1,000,000 events (`DEFAULT_RETAINED_EVENT_CAPACITY`); the target's ring
drops (and counts) events the editor does not drain in time. The Flamegraph window opens any `.db`.

**Schema** (`profiling::database::create_database`; `profile_query_sql` without `sql` returns it):

```sql
profile_events(id INTEGER PRIMARY KEY, scope_id INTEGER, parent_scope_id INTEGER, name TEXT,
  thread_id INTEGER, thread_name TEXT, process_id INTEGER, parent_name TEXT,
  start_ns INTEGER,      -- wall clock, ns since the Unix epoch: comparable across threads and processes
  duration_ns INTEGER, depth INTEGER,  -- depth: nesting on its thread, 0 = outermost
  location TEXT, metadata TEXT, track_name TEXT)
session_metadata(key TEXT PRIMARY KEY, value TEXT)   -- timestamp, version
-- indexes: (thread_id, start_ns), (start_ns)
```

- One row per finished scope. `parent_scope_id` is the scope it ran inside (identity, so it also links
  work continued on another thread); own ("self") time = `duration_ns` less the durations of the rows whose
  `parent_scope_id` is its `scope_id`.
- `name` starting `__` is a marker: `__FRAME_MARKER__` has `start_ns` = when the frame **ended** and
  `duration_ns` = the frame time.
- **Frames**: a row of the frame scope: `Game::Frame` (the packaged game's render frame, in
  `windowed_app`), else `helio_frame` (the editor viewport), else the marker; `TickLoop::tick` is the
  simulation tick. The frame's *cost* is its duration, its *period* the distance to the next frame's start.
- **GPU**: rows with `thread_id = 0`, `thread_name`/`track_name` `GPU`: a root `helio_frame (estimated GPU
  placement)` (depth 0) and one child per pass, from Helio's timestamp queries (read back a frame or two late, never
  waited for); `metadata` carries `lag_frames`, `readback_drops`, `query_overflows`. The editor viewport and the
  packaged game (`SceneRenderer::render`) publish them while a recording runs.
- **Input latency**: the game publishes each followed input as scopes on the `Input latency` track (root
  `Latency::input_to_present`, children `Latency::<stage>`; stages and stamps in `pulsar_game::latency`). The chain's first
  stamp is the OS (the raw-input thread), and the time raw input waited for the event loop is its own root,
  `Latency::os_to_handler`. The mouse look the frame applies ahead of the tick (the late look) is its own root,
  `Latency::look_to_present`: the OS to the `present` of the first frame that showed the motion (no tick stages).
- Names containing wait, lock, fence, present, acquire, sleep, idle, park, block, recv or vsync are
  counted as waits by `profile_threads`.

**Tools** (every reading tool takes `recording`: `latest` by default, a file stem, a file name or an
absolute path, and opens it with `SQLITE_OPEN_READ_ONLY` and `PRAGMA query_only`):

| Tool | Answers |
|---|---|
| `profile_status` | recording running (and from where), recordable processes, the recordings folder |
| `profile_start` `{target: game|editor|auto|pid, duration_seconds, wait, uncap_frame_rate}` | start the recorder; it stops itself after the duration (a registered timer thread) |
| `profile_stop` | stop whoever's recording, save, return the overview and top scopes |
| `profile_list_recordings` | the project's recordings with events, span, frame scope and count |
| `profile_query_sql` `{sql, limit}` | one read-only statement, at most 5000 rows; no `sql`: the schema, column meanings, examples |
| `profile_top_scopes` | self/total time, calls and per-frame figures, per-call p50/p95/p99/max, worst frames and what ran in them; by scope or by thread and scope |
| `profile_frame_times` | period and cost percentiles, fps and 1% low, histogram, series, hitches with what ran, the other frame streams (tick next to render) |
| `profile_frame_breakdown` | one frame's scope tree, critical path (waits flagged) and what the other threads did |
| `profile_threads` | busy, idle, utilisation and wait time per thread; waits ranked |
| `profile_gpu` | per-pass GPU time, or why there is none |
| `profile_latency` | the input-to-present chain per stage: mean, p50, p95, p99, worst, share |
| `profile_gaps` | scopes that are mostly their own time: where to add inner scopes |
| `profile_compare` `{a, b}` | per-scope own time per frame before and after, ranked by change |
| `profile_export` | a Chrome trace of the recording under `<recordings>/exports/` |

Per-scope detail: `HELIO_FINISH_BREAKDOWN=1` in the editor's environment makes the render graph finish its command
buffers in line and name each one (`encoder.finish: <passes>` scopes): it shows which pass the CPU spends in wgpu's
validation (it serialises the frame, so use it to attribute, not to time).

Typical loop: `build_start {run: true}`, `profile_start {target: "game", duration_seconds: 20}`, drive the
game (walk, turn, jump), `profile_top_scopes`, `profile_frame_times`, `profile_threads`,
`profile_latency`; change code; record again; `profile_compare`.

Tests: `ui_flamegraph/src/profile_db.rs` (every query over a recording written with
`profiling::database`, including the read-only guarantees) and `mcp.rs` (the tools over a file).

## Screenshots

`viewport_screenshot` (`ui_level_editor/src/ai/screenshot_tool.rs`) is the Tools > High res screenshot
without its save dialog. The call runs on an MCP worker: it validates the arguments, leaves a
`McpScreenshotRequest` (`state::build`) in the open level editor's build state under one short write lock
and waits on a channel with a timeout; it never holds a scene lock and never reads the scene (see the
nested `SceneDomain::world()` deadlock fixed in 77379692b). The level viewport takes the request on its next
frame (`start_pending_screenshot`, `mcp_screenshot.rs`) and renders it on an editor task, "Viewport
screenshot (MCP)" in the Tasks window: the same `capture_viewport_png_tiled` as the window, then a copy to
the destination or a JPEG encode. The grid, gizmo and overlay lines are left out by
`GpuRenderer::set_capture_view` (`CaptureView`) for the capture's frames only and put back before the
renderer is released; Helio's `DebugDrawState` carries the grid and overlay switches
(`Renderer::set_editor_overlay_visibility`).

| Argument | Meaning |
|---|---|
| `path` | File to write, `.png`, `.jpg` or `.jpeg` (picks the format); absolute, or relative to the project. Parent folders are created. Default `<project>/.pulsar/screenshots/shot-<ms>-<n>.png`. The server has no allowed-root policy, so none is applied |
| `width`, `height` | Pixels, default 2560 x 1440, clamped to 16..16384 per side and to the device's 2D texture limit; the result says what was used (`clamped`). A very wide frame can still fail inside a Helio pass (a post-process texture over 2048 at 16384 x 100 on the test GPU): that comes back as a tile error, not a hang |
| `viewport` | Which open level (its `.level` path); omitted, the only open one |
| `hide_grid`, `hide_gizmos`, `hide_overlays` | Default true |
| `jpeg_quality` | 1..100, default 90; a JPEG over 64 megapixels is refused |
| `timeout_seconds` | Default 120, at most 1800 |

Errors name the cause: no level open or several open, the viewport has not drawn yet, a screenshot already
running, the viewport not drawing (minimised) within the timeout, the device lost or a tile failing, the
encode or the write failing. A request needs a drawing viewport: it is served on a frame. Tests: the
argument rules and the hand-over (a thread playing the viewport) in `screenshot_tool.rs`.

Blog capture notes: the call is `viewport_screenshot {"path": "D:/shots/a.png", "width": 2560, "height": 1440,
"viewport": "<.level path when several are open>"}` over `tools/call`; a 2560 x 1440 frame of a sample level
took about 0.8 s and 1920 x 1080 about 0.6 s with the editor window minimised. The grid, axis lines and gizmo
are off by default (`hide_grid: false` brings them back, which makes a before/after pair); the light
billboard icons are scene icons, not overlays, and stay. Verified end to end on a private copy of the voxel
reference project (Ocean.level) with `WGPU_BACKEND=dx12`, `PULSAR_MCP=1`, `PULSAR_MCP_PORT`, and plugin
libraries from `cargo run -p plugin_builder -- --release --out <dir>` with `PULSAR_INSTALL_DIR=<dir>`.

## Not built yet

- Viewport camera get/set and focus an object: the camera lives in the viewport's panel entity, not in
  the shared state the tools reach. Tracked in `.agents/GAMEPLAY.md` (issues).
- Input injection into the game; use OS-level input for now.
- `tools/list` pagination, resources and prompts, server-initiated notifications.

## Tests

- `pulsar_mcp`: protocol unit tests, and `tests/http.rs` over a real socket: initialize, list a
  registered tool, call it, a missing argument, no/wrong/mis-schemed token (401), foreign Host or Origin
  (403), `GET` (405), and the endpoint file.
- `ui_log_viewer`: log filters, cursors, `logs_wait` (returns on arrival, times out empty).
- `editor_task_queue`: a multi-line message is one log row per line.
- Plugin-registered tool: see `plugins/installed/tests` (the fixture plugin).

## Notes added with the editor-open work

- `tasks_list` rows carry `details` (key and value pairs a task publishes about itself): a render runtime's
  size, state, window (focused, in the background, minimized), the kind of its last frame and its frame
  count. Two reads a few seconds apart give a viewport's published frames a second
  (`title: "Helio runtime"`).
- A recording keeps at most `DEFAULT_RETAINED_EVENT_CAPACITY` (1,000,000) events, the latest: a 60 s editor
  recording of a viewport at 146 fps no longer holds the first seconds (where an open's long scopes are).
  Record the open in a window of about 20 s, or read the log lines `helio::pipeline: graph ... node(s) built`
  and `node ... built N pass(es) in X ms: ...` (the build's own account of where its time went).
