//! Panel side of MCP: starts the servers named in `.pulsar/mcp.json`, merges
//! their tools into each run's registry, and shows a small settings list
//! (servers, status, tools with on/off toggles).
//!
//! All protocol work lives in the `agent_mcp` crate.

use super::*;
use agent_mcp::{McpManager, ServerStatus};
use smol::Timer;
use std::time::Duration;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Sizable as _,
};

impl AgentChatPanel {
    /// Read the MCP config and start every enabled server in the background.
    /// `None` when no project is open or nothing is configured.
    pub(super) fn start_mcp() -> Option<Arc<McpManager>> {
        let root = engine_state::get_project_path().map(PathBuf::from)?;
        // Servers from the project's mcp.json wait for approval (see `trust.rs`).
        let manager = McpManager::load(&root, super::trust::trust_store());
        if manager.is_empty() {
            return None;
        }
        manager.start_all();
        Some(manager)
    }

    /// Re-render while servers are still starting so their status shows up.
    pub(super) fn watch_mcp_startup(&self, cx: &mut Context<Self>) {
        if self.mcp.is_none() {
            return;
        }
        cx.spawn(async move |this, cx| {
            for _ in 0..120 {
                Timer::after(Duration::from_millis(500)).await;
                let done = cx.update(|cx| {
                    this.update(cx, |panel, cx| {
                        cx.notify();
                        panel
                            .mcp
                            .as_ref()
                            .map(|m| m.wait_ready(Duration::ZERO))
                            .unwrap_or(true)
                    })
                    .unwrap_or(true)
                });
                if done {
                    break;
                }
            }
        })
        .detach();
    }

    /// The panel's registry plus the tools of every running MCP server.
    pub(super) fn registry_with_mcp(&self) -> ToolRegistry {
        let mut registry = self.tool_registry.clone();
        if let Some(mcp) = &self.mcp {
            mcp.register_into(&mut registry);
        }
        registry
    }

    /// Stop all server processes (chat panel closed).
    pub(super) fn shutdown_mcp(&mut self) {
        if let Some(mcp) = self.mcp.take() {
            mcp.shutdown();
        }
    }

    pub(super) fn render_mcp_button(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let mcp = self.mcp.as_ref()?;
        let snapshot = mcp.snapshot();
        let running = snapshot
            .iter()
            .filter(|s| s.status == ServerStatus::Running)
            .count();
        Some(
            Button::new("agent-chat-mcp-toggle")
                .xsmall()
                .ghost()
                .label(format!("MCP: {running}/{}", snapshot.len()))
                .tooltip("External MCP servers from .pulsar/mcp.json. Click to show or hide.")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.mcp_open = !this.mcp_open;
                    cx.notify();
                })),
        )
    }

    /// Server list; renders nothing while closed or when MCP is not in use.
    pub(super) fn render_mcp_settings(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.mcp_open {
            return None;
        }
        let mcp = self.mcp.as_ref()?;
        let muted = cx.theme().muted_foreground;
        let warn = cx.theme().warning;
        let mut list = v_flex().w_full().gap_1().pb_2();
        for server in mcp.snapshot() {
            let status = match &server.status {
                ServerStatus::Failed(why) | ServerStatus::Crashed(why) => {
                    format!("{}: {why}", server.status.label())
                }
                other => other.label().to_string(),
            };
            let name = server.name.clone();
            let can_restart = !matches!(server.status, ServerStatus::Pending);
            list = list.child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(div().text_sm().child(format!("{} ({status})", server.name)))
                    .child(div().text_xs().text_color(muted).child(server.command.clone()))
                    .when(can_restart, |row| {
                        row.child(
                            Button::new(SharedString::from(format!("agent-chat-mcp-restart-{name}")))
                                .xsmall()
                                .ghost()
                                .label("Restart")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(mcp) = &this.mcp {
                                        mcp.restart_server(&name);
                                    }
                                    this.watch_mcp_startup(cx);
                                    cx.notify();
                                })),
                        )
                    }),
            );
            for tool in server.tools {
                let full = tool.full_name.clone();
                let on = tool.enabled;
                list = list.child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .pl_4()
                        .child(
                            Button::new(SharedString::from(format!("agent-chat-mcp-tool-{full}")))
                                .xsmall()
                                .ghost()
                                .label(if on { "On" } else { "Off" })
                                .tooltip("Offer this tool to the agent")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(mcp) = &this.mcp {
                                        mcp.set_tool_enabled(&full, !on);
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(div().text_xs().child(tool.name))
                        .child(
                            div()
                                .text_xs()
                                .text_color(if tool.read_only { muted } else { warn })
                                .child(if tool.read_only { "read-only" } else { "asks when tainted" }),
                        ),
                );
            }
        }
        Some(list)
    }
}

impl Drop for AgentChatPanel {
    fn drop(&mut self) {
        self.shutdown_mcp();
    }
}
