use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _}, h_flex, ActiveTheme, IconName, Selectable, Sizable,
};

use super::state::{LevelEditorState, TransformTool};
use super::actions::*;

/// Toolbar - Transform tools and quick actions
pub struct ToolbarPanel;

impl ToolbarPanel {
    pub fn new() -> Self {
        Self
    }

    pub fn render<V: 'static>(&self, state: &LevelEditorState, cx: &mut Context<V>) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        h_flex()
            .w_full()
            .h(px(40.0))
            .px_3()
            .gap_1()
            .items_center()
            .bg(cx.theme().sidebar)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                // Transform tools
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("tool_select")
                            .icon(IconName::CursorPointer)
                            .tooltip("Select (S)")
                            .selected(matches!(state.current_tool, TransformTool::Select))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.dispatch_action(&SelectTool);
                            }))
                    )
                    .child(
                        Button::new("tool_move")
                            .icon(IconName::Drag)
                            .tooltip("Move (M)")
                            .selected(matches!(state.current_tool, TransformTool::Move))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.dispatch_action(&MoveTool);
                            }))
                    )
                    .child(
                        Button::new("tool_rotate")
                            .icon(IconName::RotateCameraRight)
                            .tooltip("Rotate (R)")
                            .selected(matches!(state.current_tool, TransformTool::Rotate))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.dispatch_action(&RotateTool);
                            }))
                    )
                    .child(
                        Button::new("tool_scale")
                            .icon(IconName::Enlarge)
                            .tooltip("Scale (T)")
                            .selected(matches!(state.current_tool, TransformTool::Scale))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.dispatch_action(&ScaleTool);
                            }))
                    )
            )
            .child(
                // Separator
                div()
                    .h_8()
                    .w_px()
                    .bg(cx.theme().border)
                    .mx_2()
            )
            .child(
                // Play/Stop controls
                h_flex()
                    .gap_1()
                    .child(
                        if state.is_edit_mode() {
                            Button::new("play")
                                .icon(IconName::Play)
                                .tooltip("Play (Ctrl+P)")
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.dispatch_action(&PlayScene);
                                }))
                                .into_any_element()
                        } else {
                            Button::new("play_disabled")
                                .icon(IconName::Play)
                                .tooltip("Already playing")
                                .ghost()
                                .into_any_element()
                        }
                    )
                    .child(
                        if state.is_play_mode() {
                            Button::new("stop")
                                .icon(IconName::X)
                                .tooltip("Stop (Ctrl+.)")
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.dispatch_action(&StopScene);
                                }))
                                .into_any_element()
                        } else {
                            Button::new("stop_disabled")
                                .icon(IconName::X)
                                .tooltip("Not playing")
                                .ghost()
                                .into_any_element()
                        }
                    )
            )
            .child(
                // Separator
                div()
                    .h_8()
                    .w_px()
                    .bg(cx.theme().border)
                    .mx_2()
            )
            .child(
                // Object creation tools
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("add_mesh")
                            .icon(IconName::Plus)
                            .tooltip("Add Mesh")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.dispatch_action(&AddObjectOfType {
                                    object_type: "Mesh".to_string()
                                });
                            }))
                    )
                    .child(
                        Button::new("add_light")
                            .icon(IconName::Sun)
                            .tooltip("Add Light")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.dispatch_action(&AddObjectOfType {
                                    object_type: "Light".to_string()
                                });
                            }))
                    )
                    .child(
                        Button::new("add_camera")
                            .icon(IconName::Camera)
                            .tooltip("Add Camera")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.dispatch_action(&AddObjectOfType {
                                    object_type: "Camera".to_string()
                                });
                            }))
                    )
            )
            .child(
                // Spacer
                div().flex_1()
            )
            .child(
                // Scene file actions
                h_flex()
                    .gap_1()
                    .child({
                        let mut btn = Button::new("save_scene")
                            .icon(IconName::FloppyDisk)
                            .tooltip("Save Scene (Ctrl+S)");

                        if state.has_unsaved_changes {
                            btn = btn.text_color(cx.theme().warning);
                        }

                        btn.on_click(cx.listener(|_, _, _, cx| {
                            cx.dispatch_action(&SaveScene);
                        }))
                    })
                    .child(
                        Button::new("open_scene")
                            .icon(IconName::FolderOpen)
                            .tooltip("Open Scene (Ctrl+O)")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.dispatch_action(&OpenScene);
                            }))
                    )
                    .child(
                        Button::new("new_scene")
                            .icon(IconName::FolderPlus)
                            .tooltip("New Scene (Ctrl+N)")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.dispatch_action(&NewScene);
                            }))
                    )
            )
    }
}
