use gpui::prelude::FluentBuilder as _;
use gpui::*;
use rust_i18n::t;
use std::path::PathBuf;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, ContextModal as _, Sizable as _,
};

mod actions;
mod feature_toggles;
mod global_toolbar;
mod level_editor_menus;
mod mode_indicator;
mod multiplayer_panel;
mod playback_controls;
pub(crate) mod snap_controls;
mod time_scale_dropdown;
mod tool_mode_dropdown;
mod view;
pub(crate) mod view_mode_dropdown;

pub use actions::*;
pub use global_toolbar::{GlobalToolbarView, GLOBAL_TOOLBAR_HEIGHT};
use level_editor_menus::LevelEditorMenus;
use mode_indicator::ModeIndicator;
use tool_mode_dropdown::ToolModeDropdown;
pub use view::ToolbarView;

use crate::ui::mode_widgets::{active_mode_widgets, render_mode_widgets};
use crate::LevelEditorState;

/// The editor command row. Viewport controls float inside the viewport.
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
        let _scope = gpui::render_stats::scope("toolbar: panel render");
        let background = cx.theme().background;

        let _mode_scope = gpui::render_stats::scope("toolbar: mode controls");
        let mode_controls = active_mode_widgets(state, &gpu_engine);
        drop(_mode_scope);
        let has_mode_controls = !mode_controls.is_empty();

        v_flex().w_full().bg(background).child(
            h_flex()
                .w_full()
                .h(px(36.0))
                .px_2()
                .gap_2()
                .items_center()
                .child({
                    let _scope = gpui::render_stats::scope("toolbar: tool mode dropdown");
                    ToolModeDropdown::render(state, state_arc.clone(), cx)
                })
                .child(LevelEditorMenus::render(state))
                .when(has_mode_controls, |el| {
                    el.child(self.render_separator(cx))
                        .child(render_mode_widgets(
                            mode_controls,
                            state_arc.clone(),
                            gpu_engine.clone(),
                            cx,
                        ))
                })
                .child(div().flex_1())
                .child(self.render_viewport_split_buttons())
                .child(self.render_separator(cx))
                .child({
                    let _scope = gpui::render_stats::scope("toolbar: mode indicator");
                    ModeIndicator::render(state, cx)
                })
                .child(self.render_separator(cx))
                .child({
                    let _scope = gpui::render_stats::scope("toolbar: save button");
                    self.render_save_button(state_arc.clone(), gpu_engine.clone())
                })
                .when(Self::is_source_build(), |el| {
                    el.child(
                        self.render_save_as_default_button(state_arc.clone(), gpu_engine.clone()),
                    )
                }),
        )
    }

    /// Split the viewport into two or four. Clicking the layout that is
    /// already showing returns to a single viewport.
    fn render_viewport_split_buttons(&self) -> impl IntoElement {
        h_flex()
            .gap_1()
            .items_center()
            .child(
                Button::new("toolbar_split_viewports_half")
                    .icon(ui::IconName::PanelRight)
                    .small()
                    .ghost()
                    .tooltip(t!("LevelEditor.Viewport.SplitHalf"))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::ui::actions::SplitViewportsHalf), cx)
                    }),
            )
            .child(
                Button::new("toolbar_split_viewports_quad")
                    .icon(ui::IconName::LayoutDashboard)
                    .small()
                    .ghost()
                    .tooltip(t!("LevelEditor.Viewport.SplitQuad"))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::ui::actions::SplitViewportsQuad), cx)
                    }),
            )
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
                crate::ui::save::save_level(
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
    ) -> Option<crate::scene_edit::LevelEditorCameraState> {
        let camera = gpu_engine.lock().ok()?.editor_camera_state()?;
        Some(crate::scene_edit::LevelEditorCameraState {
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
                crate::ui::save::save_in_background(
                    state_clone.clone(),
                    path,
                    Self::editor_camera(&gpu_engine),
                    crate::ui::save::SaveKind::Copy,
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
}
