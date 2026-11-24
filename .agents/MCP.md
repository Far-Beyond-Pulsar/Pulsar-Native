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
| `editor_open_file` | `ui_core/src/app/mcp_tools.rs` | Open a file or class folder in its editor tab (what double-clicking in the Content browser does) |
| `level_editor_*`, `blueprint_*`, `code_editor_*` | existing AI tools, through the provider | Objects, components, hierarchy, selection, transforms, spawn and delete, undo and redo, save, play control, Blueprint graph editing, code editing |

Reading the build while it runs: `build_start`, then `build_status` and `logs_wait` with
`task_title: "Build"` (and `"Game"` for the launched game: its stdout and stderr are the `Game: Run`
task's log).

## Not built yet

- Viewport camera get/set, focus an object and a viewport screenshot as an image: the screenshot
  facility exists (`state.build.pending_screenshot_size`, which ends in a save dialog) but has no
  headless destination, and the camera lives in the viewport's panel entity, not in the shared state the
  tools reach. Tracked in `.agents/GAMEPLAY.md` (issues).
- Input injection into the game; use OS-level input for now.
- `tools/list` pagination, resources and prompts, server-initiated notifications.

## Tests

- `pulsar_mcp`: protocol unit tests, and `tests/http.rs` over a real socket: initialize, list a
  registered tool, call it, a missing argument, no/wrong/mis-schemed token (401), foreign Host or Origin
  (403), `GET` (405), and the endpoint file.
- `ui_log_viewer`: log filters, cursors, `logs_wait` (returns on arrival, times out empty).
- `editor_task_queue`: a multi-line message is one log row per line.
- Plugin-registered tool: see `plugins/installed/tests` (the fixture plugin).
