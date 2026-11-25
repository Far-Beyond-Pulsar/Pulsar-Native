//! The MCP server's settings (Editor Settings > MCP Server).
//!
//! The server lets an external AI client call the editor's tools over the
//! Model Context Protocol (`crates/core/pulsar_mcp`, `.agents/MCP.md`). It binds
//! loopback only and requires a bearer token it writes to an endpoint file in the
//! app data folder, but it can build, delete and save: it is **off** until a person
//! turns it on. `PULSAR_MCP=1` in the environment turns it on for one editor
//! session (and `PULSAR_MCP_PORT` picks its port), which is what a headless test
//! or a CI run uses.

use pulsar_config::{ConfigManager, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "mcp";
pub const ENABLED: &str = "enabled";
pub const PORT: &str = "port";
/// The port when none is set.
pub const DEFAULT_PORT: i64 = 47_832;

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new(
        "MCP Server",
        "Let an AI client (Claude Code, an IDE) call the editor's tools over the Model Context Protocol",
    )
    .setting(
        ENABLED,
        SchemaEntry::new(
            "Run the MCP server. It listens on 127.0.0.1 only and needs the bearer token in the endpoint file (see the MCP guide). Tools can build, edit, delete and save: turn it on only for a client you trust.",
            false,
        )
        .label("Enable MCP Server")
        .page("MCP Server")
        .field_type(FieldType::Checkbox),
    )
    .setting(
        PORT,
        SchemaEntry::new("The loopback TCP port the server listens on", DEFAULT_PORT)
            .label("Port")
            .page("MCP Server")
            .field_type(FieldType::NumberInput {
                min: Some(1024.0),
                max: Some(65535.0),
                step: Some(1.0),
            })
            .validator(Validator::int_range(1024, 65535)),
    );
    let _ = cfg.register(NS, OWNER, schema);
}
