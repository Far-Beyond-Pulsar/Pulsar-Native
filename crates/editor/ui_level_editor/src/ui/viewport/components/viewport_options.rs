//! Viewport options toolbar component.
//!
//! Draggable transform and snap controls with an overlay menu.

use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use rust_i18n::t;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    popup_menu::PopupMenuExt,
    ActiveTheme, IconName, Selectable, Sizable,
};

use super::floating_toolbar::{create_drag_handle, toolbar_with_drag_handle, HandleSide};
use crate::state::{LevelEditorState, TransformTool};
use crate::ui::toolbar::snap_controls::{SnapPanel, TransformSnapControls};

/// Keep diagnostics available without expanding the floating bar into a strip.
fn overlay_menu(state_arc: Arc<parking_lot::RwLock<LevelEditorState>>) -> impl IntoElement {
    Button::new("viewport_overlays")
        .label("Overlays")
        .dropdown_caret(true)
        .small()
        .ghost()
        .popup_menu(move |menu, _, _| {
            let state = state_arc.read();
            let options: [(&str, bool, fn(&mut crate::state::OverlayDomain, bool)); 4] = [
                (
                    "Performance",
                    state.overlays.state.show_performance_overlay,
                    crate::state::OverlayDomain::set_show_performance_overlay,
                ),
                (
                    "GPU pipeline",
                    state.overlays.state.show_gpu_pipeline_overlay,
                    crate::state::OverlayDomain::set_show_gpu_pipeline_overlay,
                ),
                (
                    "Stats",
                    state.overlays.state.show_nerds_overlay,
                    crate::state::OverlayDomain::set_show_nerds_overlay,
                ),
                (
                    "Camera controls",
                    state.overlays.state.show_camera_mode_selector,
                    crate::state::OverlayDomain::set_show_camera_mode_selector,
                ),
            ];
            drop(state);
            let mut menu = menu;
            for (label, enabled, set_enabled) in options {
                let state = state_arc.clone();
                let handler = move |_: &mut Window, _: &mut App| {
                    set_enabled(&mut state.write().overlays, !enabled);
                };
                menu = if enabled {
                    menu.menu_handler_with_icon(label, IconName::Check, handler)
                } else {
                    menu.menu_handler(label, handler)
                };
            }
            menu
        })
}

/// Render gizmo tool selector buttons.
fn gizmo_tool_buttons(
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    state: &LevelEditorState,
) -> impl IntoElement {
    h_flex()
        .gap_1()
        .child({
            let state_clone = state_arc.clone();
            Button::new("select_tool")
                .icon(IconName::PcMouse)
                .small()
                .ghost()
                .tooltip("Select Tool (Q)")
                .selected(state.editor.current_tool == TransformTool::Select)
                .on_click(move |_, _, _| {
                    state_clone.write().editor.set_tool(TransformTool::Select);
                })
        })
        .child({
            let state_clone = state_arc.clone();
            Button::new("move_tool")
                .icon(IconName::ArrowUnion)
                .small()
                .ghost()
                .tooltip("Move Tool (W)")
                .selected(state.editor.current_tool == TransformTool::Move)
                .on_click(move |_, _, _| {
                    state_clone.write().editor.set_tool(TransformTool::Move);
                })
        })
        .child({
            let state_clone = state_arc.clone();
            Button::new("rotate_tool")
                .icon(IconName::Refresh)
                .small()
                .ghost()
                .tooltip("Rotate Tool (E)")
                .selected(state.editor.current_tool == TransformTool::Rotate)
                .on_click(move |_, _, _| {
                    state_clone.write().editor.set_tool(TransformTool::Rotate);
                })
        })
        .child({
            let state_clone = state_arc.clone();
            Button::new("scale_tool")
                .icon(IconName::Maximize)
                .small()
                .ghost()
                .tooltip("Scale Tool (R)")
                .selected(state.editor.current_tool == TransformTool::Scale)
                .on_click(move |_, _, _| {
                    state_clone.write().editor.set_tool(TransformTool::Scale);
                })
        })
}

/// Render the complete viewport options toolbar.
pub fn render_viewport_options<V>(
    state: &LevelEditorState,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    snap_panels: &[Entity<SnapPanel>; 3],
    is_dragging: bool,
    cx: &mut Context<V>,
) -> impl IntoElement
where
    V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
{
    if state.overlays.state.viewport_options_collapsed {
        return Button::new("expand_viewport_options")
            .icon(IconName::LayoutDashboard)
            .tooltip(t!("LevelEditor.Viewport.ViewportOptions"))
            .on_click(move |_, _, _| {
                state_arc
                    .write()
                    .overlays
                    .set_viewport_options_collapsed(false);
            })
            .into_any_element();
    }

    let drag_handle = create_drag_handle(
        state_arc.clone(),
        |s: &mut LevelEditorState, pos| s.overlays.positions.viewport_drag_start = pos,
        |s: &mut LevelEditorState, dragging| s.overlays.positions.is_dragging_viewport = dragging,
        HandleSide::Left,
        cx,
    );

    let toolbar_content = h_flex()
        .gap_2()
        .items_center()
        .child(gizmo_tool_buttons(state_arc.clone(), state))
        .child(div().h(px(20.0)).w_px().bg(cx.theme().border))
        .child(TransformSnapControls::render(snap_panels, state))
        .child(div().h(px(20.0)).w_px().bg(cx.theme().border))
        .child(overlay_menu(state_arc.clone()))
        .child(
            Button::new("collapse_viewport_options")
                .icon(IconName::Close)
                .small()
                .ghost()
                .on_click(move |_, _, _| {
                    state_arc
                        .write()
                        .overlays
                        .set_viewport_options_collapsed(true);
                }),
        );

    h_flex()
        .gap_0()
        .when(is_dragging, |f| f.cursor(CursorStyle::PointingHand))
        .child(toolbar_with_drag_handle(
            drag_handle,
            toolbar_content,
            HandleSide::Left,
            cx,
        ))
        .into_any_element()
}
