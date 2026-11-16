//! Floating viewport controls, performance HUDs and debug-view legends.
//!
//! The movable overlays are RnD panels on a Panel Canvas
//! (`ui::panel_canvas`, see `.agents/PANEL_CANVAS.md`).

use super::*;

use crate::ui::toolbar::view_mode_dropdown::ViewModePickerState;
use components::floating_toolbar::corner_grip;
use rust_i18n::t;
use ui::button::Button;
use ui::panel_canvas::{PanelCanvas, PanelRef, RndPanel, Slot, SlotAnchor, SlotFlow, SlotGroup};
use ui::IconName;

impl ViewportPanel {
    /// The view-mode picker this viewport shows: its own when it was split off,
    /// else the toolbar's.
    fn view_picker(&self, cx: &App) -> Entity<ViewModePickerState> {
        match &self.own_picker {
            Some(own) => own.picker.clone(),
            None => self.toolbar.read(cx).view_mode_picker.clone(),
        }
    }

    fn view_picker_label(&self, cx: &App) -> SharedString {
        match &self.own_picker {
            Some(own) => own.label(cx),
            None => self.toolbar.read(cx).view_mode_label(cx),
        }
    }

    /// Shared state changes must invalidate the floating controls even when
    /// Helio only composites a new frame without rebuilding the UI.
    ///
    /// `state` is the editor's shared state (tool, snaps); the overlays, camera
    /// mode and debug view are this viewport's own.
    pub(crate) fn controls_signature(&self, state: &LevelEditorState, cx: &App) -> impl PartialEq {
        let editor = &state.editor;
        let view_guard = self.own_view.then(|| self.view_state.read());
        let view: &LevelEditorState = view_guard.as_deref().unwrap_or(state);
        let overlays = &view.overlays.state;
        (
            (
                editor.current_tool,
                view.editor.camera_mode,
                view.editor.viewport_debug_mode,
                editor.location_snap.to_bits(),
                editor.rotation_snap.to_bits(),
                editor.scale_snap.to_bits(),
                self.input_state.get_move_speed().to_bits(),
            ),
            (
                overlays.show_viewport_options,
                overlays.viewport_options_collapsed,
                overlays.show_camera_mode_selector,
                overlays.camera_mode_selector_collapsed,
                overlays.show_performance_overlay,
                overlays.show_gpu_pipeline_overlay,
                overlays.show_render_stats_overlay,
                overlays.show_nerds_overlay,
                overlays.show_warnings_overlay,
                overlays.debug_overlays.clone(),
                overlays.camera_sync_locked,
            ),
            state.editor_loading.snapshot(),
            self.view_picker(cx).entity_id(),
        )
    }

    /// Render all viewport overlays.
    ///
    /// The movable ones are RnD panels on the viewport's Panel Canvas: they
    /// sit in its slots (see [`overlay_slots`]) and the user drags them
    /// between slots or anywhere inside the viewport. The loading card, the
    /// debug-view legend and the camera sync button stay fixed.
    ///
    /// `state` and `state_arc` are the editor's shared state; this viewport's
    /// own settings are `self.view_state`.
    pub(super) fn render_overlays<V>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let canvas_state = &self.overlay_canvas;
        let panel = |id: &str| PanelRef::new(canvas_state, id.to_string());
        // Pad the canvas by the scrollbar width (12px) so overlays don't sit
        // underneath the scrollbar track.
        let scrollbar_width = px(12.0);
        let mut overlays = PanelCanvas::new("viewport-overlays", canvas_state)
            .absolute()
            .top_0()
            .left_0()
            .right(scrollbar_width)
            .bottom(scrollbar_width);

        let view_arc = self.view_state.clone();
        let view_guard = self.own_view.then(|| view_arc.read());
        let view: &LevelEditorState = view_guard.as_deref().unwrap_or(state);

        if let Some(progress) = state.editor_loading.snapshot() {
            overlays = overlays.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .w(px(280.0))
                            .rounded(px(8.0))
                            .p(px(16.0))
                            .bg(cx.theme().background)
                            .border_1()
                            .border_color(cx.theme().border)
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child(progress.label),
                            )
                            .child(
                                div()
                                    .h(px(6.0))
                                    .w_full()
                                    .rounded_full()
                                    .bg(cx.theme().muted_foreground.opacity(0.25))
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(progress.progress.clamp(0.0, 1.0)))
                                            .rounded_full()
                                            .bg(cx.theme().primary),
                                    ),
                            ),
                    ),
            );
        }

        // The picker owns the retained controls and their subscriptions.
        let picker = self.view_picker(cx);
        let view_mode_label = self.view_picker_label(cx);
        let snaps = self.toolbar.read(cx).snap_panels.clone();
        if view.overlays.state.show_viewport_options {
            let id = panel("viewport-options");
            let toolbar = components::viewport_options::render_viewport_options(
                state,
                state_arc.clone(),
                view,
                view_arc.clone(),
                &snaps,
                id.clone(),
                cx,
            );
            overlays = overlays.panel(RndPanel::new(id).default_slot("toolbar-left").child(toolbar));
        }
        if view.overlays.state.show_camera_mode_selector {
            let id = panel("camera-selector");
            let toolbar = components::camera_selector::render_camera_selector(
                view,
                view_arc.clone(),
                view.editor.camera_mode,
                self.input_state.clone(),
                &picker,
                view_mode_label,
                id.clone(),
                cx,
            );
            overlays = overlays.panel(RndPanel::new(id).default_slot("toolbar-right").child(toolbar));
        }

        // The plugins' debug overlays (the portals' links): each shown one
        // docks its panel and does its per-frame work; the others clear
        // what they gave the renderer.
        // Filled by `sync_view_settings` on the frame pump; until then, one
        // attempt at the renderer.
        let mailbox = self.mailbox.clone().or_else(|| {
            gpu_engine
                .try_lock()
                .ok()
                .and_then(|engine| engine.editor_mailbox())
        });
        let overlay_context = crate::extend::viewport::OverlayContext {
            state: &state_arc,
            gpu_engine,
            mailbox: mailbox.clone(),
        };
        let perspective = view.editor.camera_mode == crate::state::CameraMode::Perspective;
        for overlay in crate::extend::viewport::debug_overlays() {
            let shown = view.overlays.state.debug_overlays.contains(&overlay.id);
            if shown {
                let id = panel(&format!("debug-overlay-{}", overlay.id));
                let content = (overlay.panel)(&overlay_context, cx);
                let grip = corner_grip(id.clone(), cx);
                overlays = overlays.panel(
                    RndPanel::new(id)
                        .default_slot("right")
                        .w(px(320.0))
                        .child(
                            div()
                                .id(SharedString::from(format!("debug-overlay-scroll-{}", overlay.id)))
                                .w_full()
                                .max_h(px(360.0))
                                .overflow_y_scroll()
                                .child(content),
                        )
                        .child(grip),
                );
            }
            if shown && perspective {
                (overlay.perspective)(&overlay_context);
            } else if let Some(mailbox) = overlay_context.mailbox() {
                (overlay.hidden)(&mailbox);
            }
        }

        // What the overlay samples, on its own timer: the renderer's and the
        // window's frame counts and the input latency. Building it touches
        // nothing; it is used only when the overlay is created.
        let sampler: Sampler = {
            let frames = FrameSampler::new(gpu_engine.clone(), self.ui_frames.clone());
            let input = self.input_state.clone();
            std::rc::Rc::new(move |stats: &mut LiveStats| {
                frames.sample(stats);
                stats.record(
                    Metric::InputLatencyMs,
                    input.get_input_latency_us() as f64 / 1000.0,
                );
            })
        };

        // In the bottom-left strip, filling rightwards from the corner (each
        // one shown pushes the later ones right), in this order: the performance overlay, the render
        // pipeline and the render stats. Each is a view with its own refresh timers, not rebuilt here;
        // a closed one is dropped so its timers stop.
        if view.overlays.state.show_performance_overlay {
            let id = panel("performance");
            let content = render_performance_overlay(
                view,
                view_arc.clone(),
                &self.stats,
                sampler,
                &self.perf_overlay,
                cx,
            );
            let grip = corner_grip(id.clone(), cx);
            overlays = overlays.panel(
                RndPanel::new(id)
                    .default_slot("bottom-performance")
                    .child(content)
                    .child(grip),
            );
        } else {
            self.perf_overlay.borrow_mut().take();
        }
        if view.overlays.state.show_gpu_pipeline_overlay {
            let id = panel("gpu-pipeline");
            let content = render_gpu_pipeline_overlay(gpu_engine, &self.gpu_overlay, cx);
            let grip = corner_grip(id.clone(), cx);
            overlays = overlays.panel(
                RndPanel::new(id)
                    .default_slot("bottom-pipeline")
                    .child(content)
                    .child(grip),
            );
        } else {
            self.gpu_overlay.borrow_mut().take();
        }
        if view.overlays.state.show_render_stats_overlay {
            let id = panel("render-stats");
            let content = components::render_stats_overlay::render_render_stats_overlay(
                gpu_engine,
                &self.render_stats_overlay,
                cx,
            );
            let grip = corner_grip(id.clone(), cx);
            overlays = overlays.panel(
                RndPanel::new(id)
                    .default_slot("bottom-render-stats")
                    .child(content)
                    .child(grip),
            );
        } else {
            self.render_stats_overlay.borrow_mut().take();
        }

        // In the bottom-right corner: editor and renderer diagnostics. Its timer refreshes
        // the isolated overlay in place without invalidating the viewport.
        if view.overlays.state.show_nerds_overlay {
            let id = panel("nerds");
            let content = render_nerds_overlay(
                view_arc.clone(),
                gpu_engine,
                self.ui_frames.clone(),
                self.input_state.clone(),
                &self.nerds_overlay,
                cx,
            );
            let grip = corner_grip(id.clone(), cx);
            overlays = overlays.panel(
                RndPanel::new(id)
                    .default_slot("bottom-nerds")
                    // The cached Nerds view is width-full: give its card a
                    // concrete width so that resolves to the panel width.
                    .w(px(230.0))
                    .child(content)
                    .child(grip),
            );
        } else {
            self.nerds_overlay.borrow_mut().take();
        }

        if view.overlays.state.show_warnings_overlay {
            overlays = overlays.panel(components::warnings_overlay::render_warnings_overlay(
                gpu_engine,
                panel("warnings"),
                &self.warnings_overlay,
                cx,
            ));
        } else {
            self.warnings_overlay.borrow_mut().take();
        }

        // Debug passes can advertise a floating scale for a mode. Query only
        // while a debug mode is selected; lit view mode has no legend.
        // The mailbox is read, not the renderer: the render scheduler holds the
        // renderer's mutex for every frame, so a `try_lock` here missed most
        // renders and the legend flickered in and out.
        let active_legend = (view.editor.viewport_debug_mode != 0)
            .then(|| {
                mailbox.as_ref().and_then(|mailbox| {
                    mailbox
                        .viewport_debug_views()
                        .into_iter()
                        .find(|view_mode| view_mode.debug_mode == view.editor.viewport_debug_mode)
                        .and_then(|view_mode| view_mode.legend)
                })
            })
            .flatten();
        if let Some(legend) = active_legend {
            overlays = overlays.child(
                div()
                    .absolute()
                    .bottom_4()
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(components::debug_view_legend::render_debug_view_legend(
                        &legend,
                        cx.theme(),
                    )),
            );
        }

        // A split-off viewport can leave the camera sync: locked, its camera
        // moves with the others; unlocked, it is its own.
        if self.own_view {
            let locked = view.overlays.state.camera_sync_locked;
            let state = view_arc.clone();
            overlays = overlays.child(
                div()
                    .absolute()
                    .top_2()
                    .left(relative(0.5))
                    .ml(px(-16.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                    .child(
                        Button::new("viewport_camera_sync")
                            .icon(if locked {
                                IconName::Link
                            } else {
                                IconName::LinkSlash
                            })
                            .small()
                            .tooltip(if locked {
                                t!("LevelEditor.Viewport.CameraLocked")
                            } else {
                                t!("LevelEditor.Viewport.CameraUnlocked")
                            })
                            .on_click(move |_, _, _| {
                                let mut state = state.write();
                                let locked = state.overlays.state.camera_sync_locked;
                                state.overlays.state.camera_sync_locked = !locked;
                            }),
                    ),
            );
        }

        overlays
    }
}

/// The viewport overlay canvas's slot groups: two strips along the bottom.
/// `bottom-left` fills from the left corner rightwards, `bottom-right` from
/// the right corner leftwards (a group orders its slots from its anchor), so
/// each panel shown pushes its strip's later ones along instead of covering
/// them.
pub(super) fn overlay_groups() -> Vec<SlotGroup> {
    let inset = px(8.0);
    vec![
        SlotGroup::new("bottom-left", SlotAnchor::BottomLeft).inset(inset, inset),
        SlotGroup::new("bottom-right", SlotAnchor::BottomRight).inset(inset, inset),
    ]
}

/// Every bottom slot's size, so the strips line up evenly and an empty slot
/// previews at the same size whichever it is.
const BOTTOM_SLOT: (f32, f32) = (220.0, 150.0);

/// The bottom-left strip, from the corner rightwards: the render performance
/// panels' homes, then a spare.
const BOTTOM_LEFT: [&str; 4] = [
    "bottom-performance",
    "bottom-pipeline",
    "bottom-render-stats",
    "bottom-left-spare",
];

/// The bottom-right strip, from the corner leftwards: Nerds' home, then
/// spares.
const BOTTOM_RIGHT: [&str; 3] = ["bottom-nerds", "bottom-right-spare", "bottom-right-spare-2"];

/// The viewport overlay canvas's slots.
///
/// - `toolbar-left`, `toolbar-right`: one toolbar each (a toolbar is wider
///   than its slot, so the slot is full once it holds one).
/// - `left`, `right`: columns under the toolbars.
/// - The bottom strips ([`BOTTOM_LEFT`], [`BOTTOM_RIGHT`]), all slots one size.
pub(super) fn overlay_slots() -> Vec<Slot> {
    let inset = px(8.0);
    let mut slots = vec![
        Slot::new("toolbar-left", SlotAnchor::TopLeft, size(px(160.0), px(42.0))).inset(inset, inset),
        Slot::new("toolbar-right", SlotAnchor::TopRight, size(px(160.0), px(42.0))).inset(inset, inset),
        Slot::new("left", SlotAnchor::TopLeft, size(px(320.0), px(250.0)))
            .inset(inset, px(58.0))
            .flow(SlotFlow::Column),
        Slot::new("right", SlotAnchor::TopRight, size(px(320.0), px(250.0)))
            .inset(inset, px(58.0))
            .flow(SlotFlow::Column),
    ];
    let bottom = size(px(BOTTOM_SLOT.0), px(BOTTOM_SLOT.1));
    slots.extend(
        BOTTOM_LEFT
            .map(|id| Slot::new(id, SlotAnchor::BottomLeft, bottom).group("bottom-left")),
    );
    slots.extend(
        BOTTOM_RIGHT
            .map(|id| Slot::new(id, SlotAnchor::BottomRight, bottom).group("bottom-right")),
    );
    slots
}
