use gpui::*;
use ui::{button::{Button, ButtonVariants as _}, h_flex, Selectable};
use std::sync::Arc;

use crate::level_editor::ui::state::LevelEditorState;

pub struct FeatureToggles;

impl FeatureToggles {
    pub fn render<V: 'static>(
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        _cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        h_flex()
            .gap_1()
            .child(Self::render_toggle_button(
                "toggle_materials",
                "Materials",
                state.feature_materials_enabled,
                ui::IconName::Palette,
                state_arc.clone(),
                |state| state.feature_materials_enabled = !state.feature_materials_enabled,
            ))
            .child(Self::render_toggle_button(
                "toggle_lighting",
                "Lighting",
                state.feature_lighting_enabled,
                ui::IconName::Sun,
                state_arc.clone(),
                |state| state.feature_lighting_enabled = !state.feature_lighting_enabled,
            ))
            .child(Self::render_toggle_button(
                "toggle_shadows",
                "Shadows",
                state.feature_shadows_enabled,
                ui::IconName::Circle,
                state_arc.clone(),
                |state| state.feature_shadows_enabled = !state.feature_shadows_enabled,
            ))
            .child(Self::render_toggle_button(
                "toggle_bloom",
                "Bloom",
                state.feature_bloom_enabled,
                ui::IconName::Star,
                state_arc.clone(),
                |state| state.feature_bloom_enabled = !state.feature_bloom_enabled,
            ))
    }

    fn render_toggle_button<F>(
        id: &'static str,
        label: &'static str,
        enabled: bool,
        icon: ui::IconName,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        toggle_fn: F,
    ) -> impl IntoElement
    where
        F: Fn(&mut LevelEditorState) + 'static,
    {
        Button::new(id)
            .icon(icon)
            .tooltip(format!("Toggle {}", label))
            .selected(enabled)
            .on_click(move |_, _, _| {
                let mut state = state_arc.write();
                toggle_fn(&mut state);
            })
    }
}
