//! The engine's MCP (Model Context Protocol) server.
//!
//! An external AI client (Claude Code, an IDE) connects to the running editor
//! and calls its tools. This crate is the whole mechanism and knows no tool:
//!
//! - [`McpToolRegistration`]: how the engine's crates and plugins contribute a
//!   tool (category `mcp.tools`), crossing plugin libraries like any other
//!   registration;
//! - [`ToolProvider`]: how the host adds tools that are not registrations (the
//!   editor's existing AI tools, found through the plugin manager);
//! - [`protocol::handle`]: the JSON-RPC methods;
//! - [`McpServer`]: the Streamable HTTP transport on loopback, bearer-token
//!   authenticated, origin-checked, running as editor services.
//!
//! The editor starts the server from its Editor Settings (`editor / mcp`); see
//! `.agents/MCP.md` for the tool catalogue and how to connect a client.

pub mod catalog;
mod http;
pub mod protocol;
mod server;
mod tool;

pub use catalog::{ToolProvider, add_provider, remove_provider};
pub use server::{McpConfig, McpServer, default_endpoint_file, new_token};
pub use tool::{Annotations, Content, McpToolRegistration, ToolInfo, ToolResult};
