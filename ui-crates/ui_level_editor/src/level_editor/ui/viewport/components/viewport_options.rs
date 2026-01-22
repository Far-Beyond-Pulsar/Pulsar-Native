//! Viewport options toolbar component.
//!
//! This component provides a floating toolbar for toggling visual options
//! like grid, wireframe, and lighting, as well as overlay controls.

use std::sync::Arc;

use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{button::{Button, ButtonVariants as _}, h_flex, switch::Switch, ActiveTheme, IconName, Selectable, StyledExt};
use rust_i18n::t;

use crate::level_editor::ui::state::LevelEditorState;
use super::toggle_button::create_state_toggle;
use super::floating_toolbar::{toolbar_with_drag_handle, create_drag_handle};

/// Visual toggle configuration.
struct VisualToggle {
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
}

const VISUAL_TOGGLES: &[VisualToggle] = &[
    VisualToggle {
        id: "toggle_grid",
        icon: IconName::LayoutDashboard,
        tooltip: "Toggle Grid",
    },
    VisualToggle {
        id: "toggle_wireframe",
        icon: IconName::Triangle,
        tooltip: "Toggle Wireframe",
    },
    VisualToggle {
        id: "toggle_lighting",
        icon: IconName::Sun,
        tooltip: "Toggle Lighting",
    },
];

/// Render visual toggle buttons (grid, wireframe, lighting).
fn visual_toggles(
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    state: &LevelEditorState,
) -> impl IntoElement {
    h_flex()
        .gap_1()
        .child(create_state_toggle(
            "toggle_grid",
            IconName::LayoutDashboard,
            &t!("LevelEditor.Viewport.ToggleGrid").to_string(),
            state.show_grid,
            state_arc.clone(),
            |s: &mut LevelEditorState| s.toggle_grid(),
        ))
        .child(create_state_toggle(
            "toggle_wireframe",
            IconName::Triangle,
            &t!("LevelEditor.Viewport.ToggleWireframe").to_string(),
            state.show_wireframe,
            state_arc.clone(),
            |s: &mut LevelEditorState| s.toggle_wireframe(),
        ))
        .child(create_state_toggle(
            "toggle_lighting",
            IconName::Sun,
            &t!("LevelEditor.Viewport.ToggleLighting").to_string(),
            state.show_lighting,
            state_arc.clone(),
            |s: &mut LevelEditorState| s.toggle_lighting(),
        ))
}

/// Render overlay toggle switches (performance, camera).
fn overlay_toggles<V: 'static>(
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    state: &LevelEditorState,
    cx: &Context<V>,
) -> impl IntoElement
where
    V: Render,
{
    h_flex()
        .gap_2()
        .items_center()
        .child(
            h_flex()
                .gap_1()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t!("LevelEditor.ViewportOptions.Perf").to_string()),
                )
                .child({
                    let state_clone = state_arc.clone();
                    Switch::new("toggle_perf")
                        .checked(state.show_performance_overlay)
                        .on_click(move |checked, _, _| {
                            state_clone
                                .write()
                                .set_show_performance_overlay(*checked);
                        })
                }),
        )
        .child(
            h_flex()
                .gap_1()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t!("LevelEditor.ViewportOptions.GPU").to_string()),
                )
                .child({
                    let state_clone = state_arc.clone();
                    Switch::new("toggle_gpu")
                        .checked(state.show_gpu_pipeline_overlay)
                        .on_click(move |checked, _, _| {
                            state_clone.write().set_show_gpu_pipeline_overlay(*checked);
                        })
                }),
        )
        .child(
            h_flex()
                .gap_1()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t!("LevelEditor.ViewportOptions.Cam").to_string()),
                )
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("GPU"),
                )
                .child({
                    let state_clone = state_arc.clone();
                    Switch::new("toggle_gpu")
                        .checked(state.show_gpu_pipeline_overlay)
                        .on_click(move |checked, _, _| {
                            state_clone
                                .write()
                                .set_show_gpu_pipeline_overlay(*checked);
                        })
                }),
        )
        .child(
            h_flex()
                .gap_1()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t!("LevelEditor.ViewportOptions.Cam").to_string()),
                )
                .child({
                    let state_clone = state_arc.clone();
                    Switch::new("toggle_cam")
                        .checked(state.show_camera_mode_selector)
                        .on_click(move |checked, _, _| {
                            state_clone
                                .write()
                                .set_show_camera_mode_selector(*checked);
                        })
                }),
        )
}

/// Render the complete viewport options toolbar.
pub fn render_viewport_options<V: 'static>(
    state: &LevelEditorState,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    is_dragging: bool,
    cx: &mut Context<V>,
) -> impl IntoElement
where
    V: EventEmitter<ui::dock::PanelEvent> + Render,
{
    if state.viewport_options_collapsed {
        return Button::new("expand_viewport_options")
            .icon(IconName::LayoutDashboard)
            .tooltip(t!("LevelEditor.Viewport.ViewportOptions"))
            .on_click(move |_, _, _| {
                state_arc.write().set_viewport_options_collapsed(false);
            })
            .into_any_element();
    }

    let drag_handle = create_drag_handle(
        state_arc.clone(),
        |s: &mut LevelEditorState, pos| s.viewport_overlay_drag_start = pos,
        |s: &mut LevelEditorState, dragging| s.is_dragging_viewport_overlay = dragging,
        cx,
    );

    let toolbar_content = h_flex()
        .gap_2()
        .items_center()
        .child(visual_toggles(state_arc.clone(), state))
        .child(div().h(px(20.0)).w_px().bg(cx.theme().border))
        .child(overlay_toggles(state_arc.clone(), state, cx))
        .child(
            Button::new("collapse_viewport_options")
                .icon(IconName::Close)
                .ghost()
                .on_click(move |_, _, _| {
                    state_arc.write().set_viewport_options_collapsed(true);
                }),
        );

    h_flex()
        .gap_0()
        .h(px(42.0))
        .when(is_dragging, |f| f.cursor(CursorStyle::PointingHand))
        .child(toolbar_with_drag_handle(drag_handle, toolbar_content, cx))
        .into_any_element()
}
