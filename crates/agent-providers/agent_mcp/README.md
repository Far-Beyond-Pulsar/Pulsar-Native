# agent_mcp

MCP both ways for the agent framework.

## Consume external servers (client)

Create `.pulsar/mcp.json` (project) and/or `~/.pulsar/mcp.json` (global; project entries win):

```json
{ "mcpServers": { "files": {
  "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "."],
  "env": {}, "enabled": true, "timeout_secs": 30,
  "read_only_tools": ["read_file"]
} } }
```

Tools appear as `mcp__<server>__<tool>`. Safety: every MCP tool is write-class (asks once the run is
tainted) unless listed in `read_only_tools`; every MCP result is untrusted and taints the run. In Lite
mode the tools live in the `mcp` category behind `more_tools`. The chat panel's "MCP: n/m" button lists
servers, status and per-tool on/off (stored in `.pulsar/mcp_state.json`). Only stdio is supported;
a `url` entry is reported as failed. A crashing or hanging server only fails its own calls.

## Expose engine tools (server)

```
cargo run -p agent_mcp --bin pulsar_mcp_server -- --workspace . [--allow-write] [--allow-tool <name>]... [--list]
```

Register it in an MCP client, e.g. `{"mcpServers":{"pulsar":{"command":"pulsar_mcp_server","args":["--workspace","."]}}}`.
Only read-class tools are exposed by default (same `ToolClassifier` as the runtime); `--allow-write` exposes
the rest. This is a headless host: tools needing the live editor report it unavailable.
