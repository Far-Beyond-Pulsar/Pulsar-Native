//! The editor's MCP server host.
//!
//! `pulsar_mcp` is the mechanism (registration, protocol, transport) and knows no
//! tool. This crate is the editor's side of it:
//!
//! - [`init`]: called once by the shell. It installs the UI-thread bridge
//!   (`ui_common::ui_call`), offers the editor's existing AI tools (plugin and
//!   level-editor tools, found through the plugin manager) as a
//!   [`pulsar_mcp::ToolProvider`], and keeps the server in step with the Editor
//!   Settings (`editor / mcp`: enabled, port): turning it on starts it, off stops
//!   it, a new port restarts it.
//! - `inspect`: the tools that describe the editor itself (tasks, threads,
//!   problems, plugins and why one did not load).
//!
//! Tools of the build, the log and the level editor are registered by the crates
//! that own those systems.

mod ai_tools;
mod inspect;
mod lifecycle;

pub use lifecycle::{current_endpoint, Endpoint};

/// Start the host: the UI bridge, the AI-tool provider and the settings watcher.
pub fn init(cx: &mut gpui::App) {
    ui_common::ui_call::install(cx);
    ai_tools::install();
    lifecycle::install(cx);
}
