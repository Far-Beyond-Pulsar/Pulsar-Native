//! MCP (Model Context Protocol) both ways for the agent framework.
//!
//! * Client: [`McpManager`] reads `.pulsar/mcp.json` (+ `~/.pulsar/mcp.json`),
//!   launches stdio servers and registers their tools as `mcp__<server>__<tool>`.
//! * Server: [`server::serve`] exposes the engine's agent tools over stdio;
//!   the `pulsar_mcp_server` binary is the entrypoint.
//!
//! See `docs`-style notes in [`manager`] for classification and taint rules.

pub mod client;
pub mod config;
pub mod manager;
pub mod server;

pub use client::{McpClient, McpError, RemoteTool, PROTOCOL_VERSION};
pub use config::{McpConfig, ServerConfig, CONFIG_FILE};
pub use manager::{
    full_tool_name, sanitize, McpManager, ServerSnapshot, ServerStatus, ToolSnapshot, MCP_CATEGORY,
};
pub use server::{exposed_tools, serve, ServerOptions};
