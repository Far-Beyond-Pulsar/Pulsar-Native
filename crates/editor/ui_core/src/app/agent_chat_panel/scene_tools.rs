//! Panel side of the engine-native scene tools: where they find the open
//! level, and the Apply / Discard strip for sandbox diffs.

use super::*;
use agent_runtime::{Review, ReviewDecision};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _,
};

/// The scene the agent's scene tools act on: the level of the open Level
/// Editor, the same world the viewport renders.
struct OpenLevelScene;

impl agent_scene_tools::SceneSource for OpenLevelScene {
    fn scene(&self) -> Result<agent_scene_tools::SharedScene, String> {
        let editor = ui_level_editor::ai_sessions::find_editor(None)?;
        let state = editor.read();
        // Stopping Play restores the pre-Play level, so an edit made now
        // would silently vanish.
        if !state.scene.is_edit_mode() {
            return Err("Play-In-Editor is running; the scene cannot be edited until Play stops."
                .to_string());
        }
        Ok(state.scene.shared_scene())
    }

    fn check_instance(
        &self,
        world: &agent_scene_tools::World,
        instance: agent_scene_tools::Entity,
    ) -> Result<(), String> {
        // The editor's level rules: one sky, one wind, one directional light.
        engine_backend::scene::level_rules::check_instance(world, instance)
            .map_err(str::to_string)
    }
}

impl AgentChatPanel {
    /// The chat's tool registry: the default tools plus the scene tools.
    /// Installing the toolbox also registers its run extension (tool classes,
    /// revert, sandbox reports and reviews) with the agent runtime.
    pub(super) fn build_tool_registry() -> ToolRegistry {
        let mut registry = agent_chat_tools::build_default_registry();
        let toolbox = agent_scene_tools::install_global(Arc::new(OpenLevelScene));
        agent_scene_tools::register_tools(&mut registry, &toolbox);
        registry
    }

    fn answer_review(&mut self, id: &str, decision: ReviewDecision, cx: &mut Context<Self>) {
        self.safety_notice = Some(match agent_runtime::resolve_review(id, decision) {
            Ok(message) => message,
            Err(err) => format!("Could not finish the review: {err}"),
        });
        cx.notify();
    }

    /// One card per sandbox diff waiting for the user. Empty (and cheap)
    /// while there is none.
    pub(super) fn pending_review_cards(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        agent_runtime::pending_reviews()
            .into_iter()
            .map(|review| self.render_review_card(review, cx).into_any_element())
            .collect()
    }

    fn render_review_card(&self, review: Review, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = cx.theme().info;
        let id_apply = review.id.clone();
        let id_discard = review.id.clone();
        let lines: Vec<&str> = review.summary.lines().collect();
        const SHOWN: usize = 14;
        let mut body = lines.iter().take(SHOWN).copied().collect::<Vec<_>>().join("\n");
        if lines.len() > SHOWN {
            body.push_str(&format!("\n... {} more line(s) in the run report", lines.len() - SHOWN));
        }
        v_flex()
            .w_full()
            .gap_1()
            .p_2()
            .rounded(px(6.0))
            .border_1()
            .border_color(accent.opacity(0.5))
            .bg(accent.opacity(0.08))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::Info).size_3().text_color(accent))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(review.title.clone()),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .font_family("JetBrains Mono")
                    .text_color(cx.theme().muted_foreground)
                    .child(body),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(SharedString::from(format!("review-apply-{}", review.id)))
                            .xsmall()
                            .primary()
                            .label("Apply to scene")
                            .tooltip("Replay the sandbox's changes into the open level as one undoable step")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.answer_review(&id_apply, ReviewDecision::Apply, cx);
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("review-discard-{}", review.id)))
                            .xsmall()
                            .ghost()
                            .label("Discard")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.answer_review(&id_discard, ReviewDecision::Discard, cx);
                            })),
                    ),
            )
    }
}
