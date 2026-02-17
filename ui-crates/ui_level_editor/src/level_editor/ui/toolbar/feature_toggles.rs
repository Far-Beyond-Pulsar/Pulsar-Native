use gpui::*;
use ui::{button::{Button, ButtonVariants as _}, h_flex, Selectable};
use std::sync::Arc;

use crate::level_editor::ui::state::LevelEditorState;
use engine_backend::subsystems::render::helio_renderer::RendererCommand;

pub struct FeatureToggles;

impl FeatureToggles {
    pub fn render<V: 'static>(
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
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
                gpu_engine.clone(),
                "basic_materials",
            ))
            .child(Self::render_toggle_button(
                "toggle_lighting",
                "Lighting",
                state.feature_lighting_enabled,
                ui::IconName::Sun,
                state_arc.clone(),
                gpu_engine.clone(),
                "basic_lighting",
            ))
            .child(Self::render_toggle_button(
                "toggle_shadows",
                "Shadows",
                state.feature_shadows_enabled,
                ui::IconName::Circle,
                state_arc.clone(),
                gpu_engine.clone(),
                "procedural_shadows",
            ))
            .child(Self::render_toggle_button(
                "toggle_bloom",
                "Bloom",
                state.feature_bloom_enabled,
                ui::IconName::Star,
                state_arc.clone(),
                gpu_engine.clone(),
                "bloom",
            ))
    }

    fn render_toggle_button(
        id: &'static str,
        label: &'static str,
        enabled: bool,
        icon: ui::IconName,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        feature_name: &'static str,
    ) -> impl IntoElement
    {
        Button::new(id)
            .icon(icon)
            .tooltip(format!("Toggle {}", label))
            .selected(enabled)
            .on_click(move |_, _, _| {
                // Toggle state in UI
                let mut state = state_arc.write();
                match feature_name {
                    "basic_materials" => state.feature_materials_enabled = !state.feature_materials_enabled,
                    "basic_lighting" => state.feature_lighting_enabled = !state.feature_lighting_enabled,
                    "procedural_shadows" => state.feature_shadows_enabled = !state.feature_shadows_enabled,
                    "bloom" => state.feature_bloom_enabled = !state.feature_bloom_enabled,
                    _ => {}
                }
                drop(state);
                
                // Send command to renderer thread
                if let Ok(engine) = gpu_engine.try_lock() {
                    if let Some(ref helio_renderer) = engine.helio_renderer {
                        let _ = helio_renderer.command_sender.send(RendererCommand::ToggleFeature(feature_name.to_string()));
                        tracing::info!("[UI] Sent toggle command for feature: {}", feature_name);
                    }
                }
            })
    }
}
