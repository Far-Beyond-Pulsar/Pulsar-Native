use engine_fs::virtual_fs;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use rust_i18n::t;
use std::path::PathBuf;
use std::sync::Arc;
use ui::{
    ActiveTheme, ContextModal as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    notification::Notification,
};

mod actions;
mod build;
mod feature_toggles;
mod mode_indicator;
mod multiplayer_dropdown;
mod playback_controls;
mod time_scale_dropdown;
mod tool_mode_dropdown;
mod view;

pub use actions::*;
use build::build_core::BuildCoreButton;
use build::build_dropdowns::BuildDropdowns;
use feature_toggles::FeatureToggles;
use mode_indicator::ModeIndicator;
use multiplayer_dropdown::MultiplayerDropdown;
use playback_controls::PlaybackControls;
use time_scale_dropdown::TimeScaleDropdown;
use tool_mode_dropdown::ToolModeDropdown;
pub use view::ToolbarView;

use crate::level_editor::ui::mode_widgets::{active_mode_widgets, render_mode_widgets};
use crate::level_editor::LevelEditorState;

/// Premium Toolbar - A beautifully crafted control panel for game development
///
/// Features:
/// - **Tool Mode**: Dropdown for switching between Level Edit, Terrain, etc.
/// - **Playback Controls**: Intuitive play/pause/stop for simulation
/// - **Time Scale**: Smooth dropdown for speed control with checkmarks
/// - **Multiplayer Mode**: Clean dropdown for networking options
/// - **Build Settings**: Professional dropdowns for config & platform
/// - **Mode Indicator**: Polished badge showing current mode
/// - **Performance Toggle**: Quick access to profiling overlay
///
/// Design Philosophy:
/// - Use dropdowns where choice clarity matters (time scale, multiplayer, build)
/// - Use badges for status display (mode indicator)
/// - Use icon buttons for toggles (profiling)
/// - Everything styled to blend seamlessly with the editor
/// - All controls modify real state immediately
pub struct ToolbarPanel;

impl ToolbarPanel {
    pub fn new() -> Self {
        Self
    }

    pub fn render<V>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let theme = cx.theme();

        let mode_controls = active_mode_widgets(state, &gpu_engine);
        let has_mode_controls = !mode_controls.is_empty();

        h_flex()
            .w_full()
            .h(px(48.0))
            .px_4()
            .gap_3()
            .items_center()
            .bg(theme.sidebar.opacity(0.98))
            .border_b_1()
            .border_color(theme.border.opacity(0.8))
            .shadow_sm()
            .child(ToolModeDropdown::render(state, state_arc.clone(), cx))
            .child(self.render_separator(cx))
            .when(has_mode_controls, |el| {
                el.child(render_mode_widgets(
                    mode_controls,
                    state_arc.clone(),
                    gpu_engine.clone(),
                    cx,
                ))
                .child(self.render_separator(cx))
            })
            .child(PlaybackControls::render(state, state_arc.clone(), cx))
            .child(self.render_separator(cx))
            .child(TimeScaleDropdown::render(state, state_arc.clone(), cx))
            .child(self.render_separator(cx))
            .child(MultiplayerDropdown::render(state, state_arc.clone(), cx))
            .child(self.render_separator(cx))
            .child(BuildDropdowns::render(state, state_arc.clone(), cx))
            .child(self.render_separator(cx))
            .child(FeatureToggles::render(
                state,
                state_arc.clone(),
                gpu_engine.clone(),
                cx,
            ))
            .child(div().flex_1())
            .child(ModeIndicator::render(state, cx))
            .child(self.render_separator(cx))
            .child(BuildCoreButton::render(state, state_arc.clone(), cx))
            .child(self.render_separator(cx))
            .child(self.render_save_button(state_arc.clone(), gpu_engine.clone()))
            .when(Self::is_source_build(), |el| {
                el.child(self.render_separator(cx)).child(
                    self.render_save_as_default_button(state_arc.clone(), gpu_engine.clone()),
                )
            })
            .child(self.render_separator(cx))
            .child(self.render_profiling_button(state, state_arc.clone(), cx))
    }

    fn default_level_path() -> Option<PathBuf> {
        let project_str = engine_state::get_project_path()?;
        Some(
            PathBuf::from(project_str)
                .join("scene")
                .join("default.level"),
        )
    }

    fn render_save_button(
        &self,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    ) -> impl IntoElement {
        let state_clone = state_arc.clone();
        Button::new("toolbar_save_scene")
            .label("Save")
            .primary()
            .tooltip("Save scene")
            .on_click(move |_, window, cx| {
                let target_path = {
                    let state = state_clone.read();
                    state
                        .scene
                        .current_scene
                        .clone()
                        .or_else(Self::default_level_path)
                };

                let Some(path) = target_path else {
                    tracing::warn!("Save skipped: no current scene and no project path");
                    return;
                };

                // Background save (#967); sets the level's path on success.
                crate::level_editor::ui::save::save_level(
                    state_clone.clone(),
                    path,
                    Self::editor_camera(&gpu_engine),
                    window,
                    cx,
                );
            })
    }

    /// The editor camera pose to store in the saved level.
    fn editor_camera(
        gpu_engine: &Arc<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    ) -> Option<crate::level_editor::scene_edit::LevelEditorCameraState> {
        let camera = gpu_engine.lock().ok()?.editor_camera_state()?;
        Some(crate::level_editor::scene_edit::LevelEditorCameraState {
            position: camera.position,
            yaw: camera.yaw,
            pitch: camera.pitch,
        })
    }

    fn is_source_build() -> bool {
        engine_state::EngineContext::global()
            .map(|ctx| {
                ctx.store
                    .get_or_init::<engine_state::DevContext>()
                    .read()
                    .is_source_build
            })
            .unwrap_or(false)
    }

    fn render_save_as_default_button(
        &self,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    ) -> impl IntoElement {
        let state_clone = state_arc.clone();
        Button::new("save_as_default_level")
            .label("Save as Default")
            .icon(ui::IconName::Star)
            .tooltip("Save this scene as the engine's built-in default level (source builds only)")
            .on_click(move |_, window, cx| {
                // Resolve the target path: <workspace_root>/assets/default.level
                let target_path = engine_state::EngineContext::global()
                    .and_then(|ctx| {
                        ctx.store
                            .get_or_init::<engine_state::DevContext>()
                            .read()
                            .source_path
                            .clone()
                    })
                    .map(|root| root.join("assets").join("default.level"));

                let Some(path) = target_path else {
                    window.push_notification(
                        ui::notification::Notification::error(
                            t!("Notification.Title.SaveAsDefaultLevel").to_string(),
                        )
                        .message(
                            t!("Notification.Message.CannotDetermineWorkspaceRoot").to_string(),
                        ),
                        cx,
                    );
                    return;
                };

                if let Some(parent) = path.parent() {
                    if let Err(e) = engine_fs::virtual_fs::create_dir_all(parent) {
                        window.push_notification(
                            ui::notification::Notification::error(
                                t!("Notification.Title.SaveAsDefaultLevel").to_string(),
                            )
                            .message(
                                t!(
                                    "Notification.Message.CouldNotCreateAssetsDirectory",
                                    error => e.to_string()
                                )
                                .to_string(),
                            ),
                            cx,
                        );
                        return;
                    }
                }

                // A copy in the background (#967): the level keeps its own
                // path and unsaved state. Failures are notified by the saver.
                let saved_to = path.clone();
                crate::level_editor::ui::save::save_in_background(
                    state_clone.clone(),
                    path,
                    Self::editor_camera(&gpu_engine),
                    crate::level_editor::ui::save::SaveKind::Copy,
                    window,
                    cx,
                    move |result, window, cx| {
                        if result.is_ok() {
                            tracing::info!("Default level saved to {:?}", saved_to);
                            window.push_notification(
                                ui::notification::Notification::success(
                                    t!("Notification.Title.SaveAsDefaultLevel").to_string(),
                                )
                                .message(
                                    t!(
                                        "Notification.Message.SavedTo",
                                        path => saved_to.display().to_string()
                                    )
                                    .to_string(),
                                ),
                                cx,
                            );
                        }
                    },
                );
            })
    }

    fn render_separator<V: 'static>(&self, cx: &mut Context<V>) -> impl IntoElement {
        div().h_6().w_px().bg(cx.theme().border.opacity(0.4))
    }

    fn render_profiling_button<V>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        _cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let state_clone = state_arc.clone();
        let is_profiling = state.overlays.state.show_performance_overlay;
        let btn = Button::new("toggle_profiling")
            .icon(ui::IconName::Activity)
            .tooltip(t!("LevelEditor.Toolbar.TogglePerformance"))
            .on_click(move |_, _, _| {
                let mut s = state_clone.write();
                s.overlays.state.show_performance_overlay =
                    !s.overlays.state.show_performance_overlay;
            });
        if is_profiling { btn.primary() } else { btn }
    }
}
