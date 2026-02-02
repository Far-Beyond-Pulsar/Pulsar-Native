//! Camera mode selector component.
//!
//! This component provides a floating toolbar for selecting camera modes
//! and adjusting camera speed.

use std::sync::Arc;

use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{button::{Button, ButtonVariants as _}, h_flex, v_flex, ActiveTheme, IconName, Selectable, Sizable, StyledExt};
use rust_i18n::t;

use crate::level_editor::ui::state::{CameraMode, LevelEditorState};
use super::toggle_button::create_state_toggle;
use super::floating_toolbar::{toolbar_with_drag_handle, create_drag_handle};

/// Camera mode button configuration.
struct CameraModeButton {
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    mode: CameraMode,
}

const CAMERA_MODES: &[CameraModeButton] = &[
    CameraModeButton {
        id: "camera_perspective",
        icon: IconName::Cube,
        tooltip: "Perspective View",
        mode: CameraMode::Perspective,
    },
    CameraModeButton {
        id: "camera_orthographic",
        icon: IconName::Square,
        tooltip: "Orthographic View",
        mode: CameraMode::Orthographic,
    },
    CameraModeButton {
        id: "camera_top",
        icon: IconName::ArrowUp,
        tooltip: "Top View",
        mode: CameraMode::Top,
    },
    CameraModeButton {
        id: "camera_front",
        icon: IconName::ArrowRight,
        tooltip: "Front View",
        mode: CameraMode::Front,
    },
    CameraModeButton {
        id: "camera_side",
        icon: IconName::ArrowLeft,
        tooltip: "Side View",
        mode: CameraMode::Side,
    },
];

/// Render camera mode buttons.
fn camera_mode_buttons(
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    current_mode: CameraMode,
) -> impl IntoElement {
    h_flex().gap_1().children(CAMERA_MODES.iter().map(|mode_btn| {
        let state_clone = state_arc.clone();
        let target_mode = mode_btn.mode;
        let icon = mode_btn.icon.clone();
        let id = mode_btn.id;
        
        // Translate tooltip dynamically
        let tooltip = match mode_btn.mode {
            CameraMode::Perspective => t!("LevelEditor.Camera.PerspectiveView"),
            CameraMode::Orthographic => t!("LevelEditor.Camera.OrthographicView"),
            CameraMode::Top => t!("LevelEditor.Camera.TopView"),
            CameraMode::Front => t!("LevelEditor.Camera.FrontView"),
            CameraMode::Side => t!("LevelEditor.Camera.SideView"),
        };
        
        Button::new(id)
            .icon(icon)
            .tooltip(tooltip)
            .selected(matches!(current_mode, m if m == target_mode))
            .on_click(move |_, _, _| {
                state_clone.write().set_camera_mode(target_mode);
            })
    }))
}

/// Render camera speed controls.
fn camera_speed_controls<S, V: 'static>(
    input_state: Arc<S>,
    cx: &Context<V>,
) -> impl IntoElement
where
    S: 'static,
    S: CameraSpeedControl,
    V: Render,
{
    let current_speed = input_state.get_move_speed();
    
    h_flex()
        .gap_1()
        .items_center()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(t!("LevelEditor.Camera.Speed").to_string()),
        )
        .child({
            let input_clone = input_state.clone();
            Button::new("speed_down")
                .icon(IconName::Minus)
                .small()
                .tooltip(t!("LevelEditor.Camera.DecreaseSpeed"))
                .on_click(move |_, _, _| {
                    input_clone.adjust_move_speed(-2.0);
                })
        })
        .child(
            div()
                .text_xs()
                .min_w(px(40.0))
                .text_center()
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().foreground)
                .child(format!("{:.1}", current_speed)),
        )
        .child({
            let input_clone = input_state.clone();
            Button::new("speed_up")
                .icon(IconName::Plus)
                .small()
                .tooltip(t!("LevelEditor.Camera.IncreaseSpeed"))
                .on_click(move |_, _, _| {
                    input_clone.adjust_move_speed(2.0);
                })
        })
}

/// Trait for camera speed control.
pub trait CameraSpeedControl {
    fn adjust_move_speed(&self, delta: f32);
    fn get_move_speed(&self) -> f32;
}

/// Render the complete camera mode selector overlay.
pub fn render_camera_selector<V, S>(
    state: &LevelEditorState,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    camera_mode: CameraMode,
    input_state: Arc<S>,
    is_dragging: bool,
    cx: &mut Context<V>,
) -> impl IntoElement
where
    V: EventEmitter<ui::dock::PanelEvent> + Render + 'static,
    S: CameraSpeedControl + 'static,
{
    if state.camera_mode_selector_collapsed {
        return Button::new("expand_camera_mode")
            .icon(IconName::Cube)
            .tooltip(t!("LevelEditor.Camera.CameraMode"))
            .on_click(move |_, _, _| {
                state_arc
                    .write()
                    .set_camera_mode_selector_collapsed(false);
            })
            .into_any_element();
    }

    let drag_handle = create_drag_handle(
        state_arc.clone(),
        |s: &mut LevelEditorState, pos| s.camera_overlay_drag_start = pos,
        |s: &mut LevelEditorState, dragging| s.is_dragging_camera_overlay = dragging,
        cx,
    );

    let toolbar_content = h_flex()
        .gap_2()
        .items_center()
        .child(camera_mode_buttons(state_arc.clone(), camera_mode))
        .child(div().h(px(20.0)).w_px().bg(cx.theme().border))
        .child(camera_speed_controls(input_state.clone(), cx))
        .child(
            Button::new("collapse_camera_mode")
                .icon(IconName::Close)
                .ghost()
                .on_click(move |_, _, _| {
                    state_arc
                        .write()
                        .set_camera_mode_selector_collapsed(true);
                }),
        );

    h_flex()
        .gap_0()
        .h(px(42.0))
        .when(is_dragging, |f| f.cursor(CursorStyle::PointingHand))
        .child(toolbar_with_drag_handle(drag_handle, toolbar_content, cx))
        .into_any_element()
}
