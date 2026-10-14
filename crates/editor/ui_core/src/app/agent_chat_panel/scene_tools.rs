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

    /// An agent changed the live level: mark it unsaved and bump its revision
    /// so pollers redraw. Undo-stack integration is deliberately not done
    /// here: the level editor's history stores whole-scene before/after
    /// snapshots per entry, which would cost O(scene) for every agent tool
    /// call, and an editor undo racing the agent's own per-run revert (which
    /// restores the same objects from its journal) would fight over the same
    /// state. The agent's own undo is the run's Revert button, which is
    /// conflict-aware.
    fn scene_edited(&self, _summary: &str) {
        if let Ok(editor) = ui_level_editor::ai_sessions::find_editor(None) {
            editor.write().scene.bump_revision(true);
        }
    }
}

impl AgentChatPanel {
    pub(super) fn load_scene_settings() -> agent_scene_tools::SceneSettings {
        let settings = engine_state::get_project_path()
            .map(|root| agent_scene_tools::SceneSettings::load(&PathBuf::from(root)))
            .unwrap_or_default();
        if let Some(toolbox) = agent_scene_tools::global_toolbox() {
            toolbox.set_settings(settings.clone());
        }
        settings
    }

    fn save_scene_settings(&self) {
        if let Some(toolbox) = agent_scene_tools::global_toolbox() {
            toolbox.set_settings(self.scene_settings.clone());
        }
        if let Some(root) = engine_state::get_project_path() {
            if let Err(err) = self.scene_settings.save(&PathBuf::from(root)) {
                tracing::warn!("[agent_chat] could not save agent_scene.json: {err}");
            }
        }
    }

    /// Footer control for the project's scene sandbox mode; click cycles
    /// off / overlay / copy.
    pub(super) fn render_scene_sandbox_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("agent-chat-scene-sandbox")
            .xsmall()
            .ghost()
            .label(format!("Scene sandbox: {}", self.scene_settings.sandbox_mode.label()))
            .tooltip(
                "How sandboxed scene runs keep their edits away from the live level. \
                 overlay: record the run's edits over the live scene, no copy (default). \
                 copy: fork a full private copy of the scene (capped by copy_max_objects in \
                 .pulsar/agent_scene.json, 0 = unlimited). off: sandbox runs edit the live \
                 scene directly. A harness can override this with `sandbox: overlay|copy|off`. \
                 Click to change.",
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.scene_settings.sandbox_mode = this.scene_settings.sandbox_mode.next();
                this.save_scene_settings();
                cx.notify();
            }))
    }

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
