use gpui::prelude::FluentBuilder as _;
use gpui::*;
use rust_i18n::t;
use std::path::PathBuf;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, ContextModal as _, Selectable as _, Sizable as _,
};

mod actions;
mod feature_toggles;
mod global_toolbar;
mod level_editor_menus;
mod mode_indicator;
mod multiplayer_panel;
mod playback_controls;
mod snap_controls;
mod time_scale_dropdown;
mod tool_mode_dropdown;
mod view;
mod view_mode_dropdown;

pub use actions::*;
use feature_toggles::FeatureToggles;
pub use global_toolbar::{GlobalToolbarView, GLOBAL_TOOLBAR_HEIGHT};
use level_editor_menus::LevelEditorMenus;
use mode_indicator::ModeIndicator;
use snap_controls::SnapPanel;
use snap_controls::TransformSnapControls;
use tool_mode_dropdown::ToolModeDropdown;
pub use view::ToolbarView;
use view_mode_dropdown::{ViewModeDropdown, ViewModePickerState};

use crate::state::TransformTool;
use crate::ui::mode_widgets::{active_mode_widgets, render_mode_widgets};
use crate::ui::viewport::components::camera_selector::render_camera_controls;
use crate::LevelEditorState;

/// The editor command row and its docked viewport controls row.
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
        snap_panels: &[Entity<SnapPanel>; 3],
        view_mode_picker: &Entity<ViewModePickerState>,
        input_state: Arc<crate::ui::viewport::input_state::InputState>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let _scope = gpui::render_stats::scope("toolbar: panel render");
        let background = cx.theme().background;
        let border = cx.theme().border;
        let muted_foreground = cx.theme().muted_foreground;

        let _mode_scope = gpui::render_stats::scope("toolbar: mode controls");
        let mode_controls = active_mode_widgets(state, &gpu_engine);
        drop(_mode_scope);
        let has_mode_controls = !mode_controls.is_empty();

        v_flex()
            .w_full()
            .bg(background)
            .child(
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
                            self.render_save_as_default_button(
                                state_arc.clone(),
                                gpu_engine.clone(),
                            ),
                        )
                    }),
            )
            .child(
                h_flex()
                    .w_full()
                    .h(px(38.0))
                    .px_2()
                    .gap_2()
                    .items_center()
                    .border_t_1()
                    .border_color(border.opacity(0.45))
                    .bg(background.opacity(0.55))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted_foreground)
                            .child("Viewport"),
                    )
                    .child(self.render_separator(cx))
                    .child(div().text_xs().text_color(muted_foreground).child("Mode"))
                    .child(ViewModeDropdown::render(view_mode_picker))
                    .child(self.render_separator(cx))
                    .child(render_camera_controls(
                        state_arc.clone(),
                        state.editor.camera_mode,
                        input_state,
                        cx,
                    ))
                    .child(self.render_separator(cx))
                    .child(Self::render_transform_tools(state_arc.clone(), state))
                    .child(self.render_separator(cx))
                    .child(div().text_xs().text_color(muted_foreground).child("Snap"))
                    .child(TransformSnapControls::render(snap_panels, state))
                    .child(self.render_separator(cx))
                    .child(div().text_xs().text_color(muted_foreground).child("Render"))
                    .child({
                        let _scope = gpui::render_stats::scope("toolbar: feature toggles");
                        FeatureToggles::render(state, state_arc.clone(), gpu_engine, cx)
                    })
                    .child(self.render_separator(cx))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_foreground)
                            .child("Overlays"),
                    )
                    .child(Self::render_diagnostic_controls(state_arc.clone(), state))
                    .child(div().flex_1()),
            )
    }

    fn render_transform_tools(
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        state: &LevelEditorState,
    ) -> impl IntoElement {
        h_flex()
            .gap_1()
            .items_center()
            .child(Self::transform_tool_button(
                "select_tool",
                "Select",
                "Q",
                ui::IconName::PcMouse,
                TransformTool::Select,
                state.editor.current_tool,
                state_arc.clone(),
            ))
            .child(Self::transform_tool_button(
                "move_tool",
                "Move",
                "W",
                ui::IconName::ArrowUnion,
                TransformTool::Move,
                state.editor.current_tool,
                state_arc.clone(),
            ))
            .child(Self::transform_tool_button(
                "rotate_tool",
                "Rotate",
                "E",
                ui::IconName::Refresh,
                TransformTool::Rotate,
                state.editor.current_tool,
                state_arc.clone(),
            ))
            .child(Self::transform_tool_button(
                "scale_tool",
                "Scale",
                "R",
                ui::IconName::Maximize,
                TransformTool::Scale,
                state.editor.current_tool,
                state_arc,
            ))
    }

    fn transform_tool_button(
        id: &'static str,
        label: &'static str,
        shortcut: &'static str,
        icon: ui::IconName,
        tool: TransformTool,
        current: TransformTool,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    ) -> impl IntoElement {
        Button::new(id)
            .icon(icon)
            .label(label)
            .small()
            .ghost()
            .selected(current == tool)
            .tooltip(format!("{label} Tool ({shortcut})"))
            .on_click(move |_, _, _| state_arc.write().editor.set_tool(tool))
    }

    fn render_diagnostic_controls(
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        state: &LevelEditorState,
    ) -> impl IntoElement {
        h_flex()
            .gap_1()
            .items_center()
            .child(Self::diagnostic_button(
                "viewport_perf_toggle",
                "Perf",
                state.overlays.state.show_performance_overlay,
                state_arc.clone(),
                |overlays, enabled| overlays.set_show_performance_overlay(enabled),
            ))
            .child(Self::diagnostic_button(
                "viewport_gpu_toggle",
                "GPU",
                state.overlays.state.show_gpu_pipeline_overlay,
                state_arc.clone(),
                |overlays, enabled| overlays.set_show_gpu_pipeline_overlay(enabled),
            ))
            .child(Self::diagnostic_button(
                "viewport_stats_toggle",
                "Stats",
                state.overlays.state.show_nerds_overlay,
                state_arc,
                |overlays, enabled| overlays.set_show_nerds_overlay(enabled),
            ))
    }

    fn diagnostic_button(
        id: &'static str,
        label: &'static str,
        enabled: bool,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        set_enabled: impl Fn(&mut crate::state::OverlayDomain, bool) + 'static,
    ) -> impl IntoElement {
        Button::new(id)
            .label(label)
            .small()
            .ghost()
            .selected(enabled)
            .on_click(move |_, _, _| {
                let mut state = state_arc.write();
                set_enabled(&mut state.overlays, !enabled);
            })
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
