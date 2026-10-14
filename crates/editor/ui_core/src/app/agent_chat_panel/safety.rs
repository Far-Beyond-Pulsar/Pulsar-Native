//! Panel side of the agent safety features: the confirmation prompt, the
//! confirmation-policy setting and checkpoint revert.

use super::*;
use agent_runtime::{ConfirmPolicy, ConfirmReply};
use std::fs;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _,
};

const SETTINGS_FILE: &str = "agent_safety.json";

impl AgentChatPanel {
    fn safety_settings_path() -> Option<PathBuf> {
        let root = engine_state::get_project_path().map(PathBuf::from)?;
        Some(root.join(".pulsar").join(SETTINGS_FILE))
    }

    pub(super) fn load_confirm_policy() -> ConfirmPolicy {
        Self::safety_settings_path()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|value| value.get("confirm_policy")?.as_str().map(ConfirmPolicy::parse))
            .unwrap_or_default()
    }

    fn save_confirm_policy(policy: ConfirmPolicy) {
        let Some(path) = Self::safety_settings_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let body = serde_json::json!({ "confirm_policy": policy.as_str() });
        if let Err(err) = fs::write(&path, body.to_string()) {
            tracing::warn!("[agent_chat] could not save {}: {err}", path.display());
        }
    }

    /// Next policy in the always-ask / ask-when-tainted / auto cycle.
    pub(super) fn cycle_confirm_policy(&mut self, cx: &mut Context<Self>) {
        let all = ConfirmPolicy::ALL;
        let ix = all
            .iter()
            .position(|p| *p == self.confirm_policy)
            .unwrap_or(0);
        self.confirm_policy = all[(ix + 1) % all.len()];
        Self::save_confirm_policy(self.confirm_policy);
        cx.notify();
    }

    pub(super) fn queue_confirm(
        &mut self,
        id: String,
        tool: String,
        args: serde_json::Value,
        tainted: bool,
        reply: ConfirmReply,
        cx: &mut Context<Self>,
    ) {
        let raw = args.to_string();
        let args_preview = if raw.chars().count() > 160 {
            format!("{}\u{2026}", raw.chars().take(160).collect::<String>())
        } else {
            raw
        };
        self.pending_confirms.push(PendingConfirm {
            id,
            tool,
            args_preview,
            tainted,
            reply,
        });
        cx.notify();
    }

    fn answer_confirm(&mut self, id: &str, approve: bool, cx: &mut Context<Self>) {
        if let Some(ix) = self.pending_confirms.iter().position(|p| p.id == id) {
            let pending = self.pending_confirms.remove(ix);
            if approve {
                pending.reply.approve();
            } else {
                pending.reply.deny();
            }
            cx.notify();
        }
    }

    /// Deny whatever is still waiting, e.g. when the run ends.
    pub(super) fn drop_pending_confirms(&mut self) {
        for pending in self.pending_confirms.drain(..) {
            pending.reply.deny();
        }
    }

    /// Mark the newest tool-call group as the one that owns the run's checkpoint.
    pub(super) fn attach_checkpoint(&mut self, run_id: String, cx: &mut Context<Self>) {
        for item in self.display_items.iter_mut().rev() {
            if let DisplayItem::ToolCallGroup {
                checkpoint_run_id, ..
            } = item
            {
                checkpoint_run_id.get_or_insert(run_id);
                break;
            }
        }
        cx.notify();
    }

    pub(super) fn revert_run(&mut self, run_id: &str, cx: &mut Context<Self>) {
        let root = engine_state::get_project_path()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        self.safety_notice = Some(match agent_runtime::revert_checkpoint(&root, run_id) {
            Ok(report) => format!("Reverted run changes. {}", report.summary()),
            Err(err) => format!("Revert failed: {err}"),
        });
        cx.notify();
    }

    /// Approve/deny prompts, the revert outcome and the policy toggle. Renders
    /// nothing at all while there is nothing to say and the policy is default.
    pub(super) fn render_safety_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.pending_confirms.is_empty() && self.safety_notice.is_none() {
            return None;
        }
        let warn = cx.theme().warning;
        Some(
            v_flex()
                .w_full()
                .gap_1()
                .pb_2()
                .children(self.pending_confirms.iter().map(|pending| {
                    let id_approve = pending.id.clone();
                    let id_deny = pending.id.clone();
                    v_flex()
                        .w_full()
                        .gap_1()
                        .p_2()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(warn.opacity(0.5))
                        .bg(warn.opacity(0.08))
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(Icon::new(IconName::TriangleAlert).size_3().text_color(warn))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(if pending.tainted {
                                            format!(
                                                "Allow '{}'? Untrusted content is in this chat.",
                                                pending.tool
                                            )
                                        } else {
                                            format!("Allow '{}'?", pending.tool)
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family("JetBrains Mono")
                                .text_color(cx.theme().muted_foreground)
                                .child(pending.args_preview.clone()),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new(SharedString::from(format!("confirm-ok-{}", pending.id)))
                                        .xsmall()
                                        .primary()
                                        .label("Approve")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.answer_confirm(&id_approve, true, cx);
                                        })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("confirm-no-{}", pending.id)))
                                        .xsmall()
                                        .ghost()
                                        .label("Deny")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.answer_confirm(&id_deny, false, cx);
                                        })),
                                ),
                        )
                }))
                .when_some(self.safety_notice.clone(), |el, notice| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(notice),
                    )
                }),
        )
    }

    /// Small footer control showing the active policy; click to cycle.
    pub(super) fn render_confirm_policy_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("agent-chat-confirm-policy")
            .xsmall()
            .ghost()
            .icon(IconName::Settings)
            .label(format!("Confirm: {}", self.confirm_policy.label()))
            .tooltip("When the agent must ask before editing or running commands. Click to change.")
            .on_click(cx.listener(|this, _, _, cx| this.cycle_confirm_policy(cx)))
    }
}
