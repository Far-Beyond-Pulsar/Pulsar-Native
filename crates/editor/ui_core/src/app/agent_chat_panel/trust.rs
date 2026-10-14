//! Panel side of project trust: files a cloned repository can ship
//! (`.pulsar/agent_memory.md`, `.pulsar/agents/*.md`, `.pulsar/mcp.json`) are
//! not used until the user accepts them here. The card shows the exact
//! content (or a diff against what was accepted before); nothing is applied
//! until Trust is pressed. The rules live in `agent_runtime::trust`.

use super::*;
use agent_runtime::trust::{scan_project, PendingItem, PendingKind, TrustState, TrustStore};
use std::sync::OnceLock;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _,
};

/// The user-global trust record (`<config dir>/pulsar/agent_trust.json`).
pub(super) fn trust_store() -> Arc<TrustStore> {
    static STORE: OnceLock<Arc<TrustStore>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            Arc::new(match dirs::config_dir() {
                Some(dir) => TrustStore::open(dir.join("pulsar").join(agent_runtime::trust::TRUST_FILE)),
                None => TrustStore::in_memory(),
            })
        })
        .clone()
}

/// Longest stretch of a file shown in the card.
const SHOWN_CHARS: usize = 1_500;

fn shown(text: &str) -> String {
    if text.chars().count() <= SHOWN_CHARS {
        text.to_string()
    } else {
        let cut: String = text.chars().take(SHOWN_CHARS).collect();
        format!("{cut}\n... ({} more characters)", text.chars().count() - SHOWN_CHARS)
    }
}

impl AgentChatPanel {
    /// Re-read what is waiting for approval. Cheap; called when a chat is
    /// opened and after each decision.
    pub(super) fn refresh_trust_pending(&mut self) {
        let mut pending = Vec::new();
        if let Some(root) = engine_state::get_project_path().map(PathBuf::from) {
            pending.extend(scan_project(&root, &trust_store()));
        }
        if let Some(mcp) = &self.mcp {
            pending.extend(mcp.pending_approvals());
        }
        self.plan_memory.pending_trust = pending;
    }

    fn approve_trust(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(item) = self.plan_memory.pending_trust.get(index).cloned() else {
            return;
        };
        match item.kind {
            PendingKind::Mcp => {
                if let Some(mcp) = &self.mcp {
                    mcp.approve(&item.title);
                }
                self.watch_mcp_startup(cx);
            }
            PendingKind::Memory | PendingKind::Harness => {
                if let Some(root) = engine_state::get_project_path().map(PathBuf::from) {
                    trust_store().trust(&root, &item.item, &item.content);
                }
            }
        }
        self.push_steward_log(format!("Trusted {} {}", item.kind.label().to_lowercase(), item.title));
        self.refresh_trust_pending();
        cx.notify();
    }

    /// The card listing project files that wait for approval, or `None`.
    pub(super) fn render_trust_card(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.plan_memory.pending_trust.is_empty() {
            return None;
        }
        let muted = cx.theme().muted_foreground;
        let warn = cx.theme().warning;
        let mut card = v_flex()
            .id("agent-chat-trust-card")
            .w_full()
            .max_h(px(300.0))
            .overflow_y_scroll()
            .gap_1()
            .p_2()
            .mb_2()
            .rounded(px(6.0))
            .border_1()
            .border_color(warn)
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(Icon::new(IconName::TriangleAlert).size_3().text_color(warn))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("This project ships agent files you have not approved. They are not used until you trust them."),
                    ),
            );
        for (i, item) in self.plan_memory.pending_trust.iter().enumerate() {
            card = card.child(self.render_trust_item(i, item, muted, cx));
        }
        Some(card)
    }

    fn render_trust_item(
        &self,
        index: usize,
        item: &PendingItem,
        muted: Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = match item.state {
            TrustState::Changed => "changed since you last trusted it",
            _ => "new",
        };
        let body = match &item.diff {
            Some(diff) => shown(diff),
            None => shown(&item.content),
        };
        v_flex()
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(format!("{}: {} ({state})", item.kind.label(), item.title)),
                    )
                    .child(
                        Button::new(SharedString::from(format!("trust-ok-{index}")))
                            .xsmall()
                            .primary()
                            .label("Trust")
                            .tooltip("Use this from now on (until its content changes)")
                            .on_click(cx.listener(move |this, _, _, cx| this.approve_trust(index, cx))),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .p_1()
                    .rounded(px(4.0))
                    .bg(cx.theme().muted.opacity(0.3))
                    .text_xs()
                    .text_color(muted)
                    .font_family("JetBrains Mono")
                    .child(body),
            )
    }
}
