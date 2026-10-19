//! Floating viewport controls, performance HUDs and debug-view legends.

use super::*;

use crate::ui::toolbar::view_mode_dropdown::ViewModePickerState;
use rust_i18n::t;
use ui::button::{Button, ButtonVariants as _};
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
        let positions = &view.overlays.positions;
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
                overlays.show_nerds_overlay,
                overlays.camera_sync_locked,
            ),
            (
                positions.viewport,
                positions.camera,
                positions.is_dragging_viewport,
                positions.is_dragging_camera,
            ),
            state
                .scene_hydration_progress
                .as_ref()
                .map(|progress| progress.load(std::sync::atomic::Ordering::Relaxed)),
            self.view_picker(cx).entity_id(),
        )
    }

    /// Render all viewport overlays.
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
        // Pad the overlay container by the scrollbar width (12px) so overlays
        // don't sit underneath the scrollbar track.
        let scrollbar_width = px(12.0);
        let mut overlays = div()
            .absolute()
            .top_0()
            .left_0()
            .right(scrollbar_width)
            .bottom(scrollbar_width);

        let view_arc = self.view_state.clone();
        let view_guard = self.own_view.then(|| view_arc.read());
        let view: &LevelEditorState = view_guard.as_deref().unwrap_or(state);

        if let Some(progress) = &state.scene_hydration_progress {
            let progress = f32::from_bits(progress.load(std::sync::atomic::Ordering::Relaxed));
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
                                    .child("Hydrating scene…"),
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
                                            .w(relative(progress.clamp(0.0, 1.0)))
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
            overlays = overlays.child(
                div()
                    .absolute()
                    .top(px(view.overlays.positions.viewport.1))
                    .left(px(view.overlays.positions.viewport.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                    .child(components::viewport_options::render_viewport_options(
                        state,
                        state_arc.clone(),
                        view,
                        view_arc.clone(),
                        &snaps,
                        view.overlays.positions.is_dragging_viewport,
                        cx,
                    )),
            );
        }
        if view.overlays.state.show_camera_mode_selector {
            overlays = overlays.child(
                div()
                    .absolute()
                    .top(px(view.overlays.positions.camera.1))
                    .right(px(view.overlays.positions.camera.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                    .child(components::camera_selector::render_camera_selector(
                        view,
                        view_arc.clone(),
                        view.editor.camera_mode,
                        self.input_state.clone(),
                        &picker,
                        view_mode_label,
                        view.overlays.positions.is_dragging_camera,
                        cx,
                    )),
            );
        }

        // What the overlay samples, on its own timer: the renderer's statistics
        // and the input latency.
        let sampler: Sampler = {
            let engine = gpu_engine.clone();
            let input = self.input_state.clone();
            std::rc::Rc::new(move |stats: &mut LiveStats| {
                if let Some(snapshot) = EngineFrameSnapshot::read_stats(&engine) {
                    snapshot.record_into(stats);
                }
                stats.record(
                    Metric::InputLatencyMs,
                    input.get_input_latency_us() as f64 / 1000.0,
                );
            })
        };

        // Bottom-left: Performance overlay. A view with its own refresh timers,
        // not rebuilt here; see `components/performance_overlay`.
        if view.overlays.state.show_performance_overlay {
            overlays = overlays.child(div().absolute().bottom_2().left_2().max_w(px(400.0)).child(
                render_performance_overlay(
                    view,
                    view_arc.clone(),
                    &self.stats,
                    sampler,
                    &self.perf_overlay,
                    cx,
                ),
            ));
        } else {
            // Closed: drop the overlay so its timers stop.
            self.perf_overlay.borrow_mut().take();
        }

        // GPU Pipeline overlay - positions next to performance overlay if both visible
        if view.overlays.state.show_gpu_pipeline_overlay {
            let overlay_div = if view.overlays.state.show_performance_overlay {
                // Position to the right of performance overlay
                div().absolute().bottom_2().left(px(300.0)) // 400px width + 10px gap
            } else {
                // Take performance overlay's position
                div().absolute().bottom_2().left_2()
            };

            overlays = overlays.child(overlay_div.max_w(px(400.0)).child(
                render_gpu_pipeline_overlay(gpu_engine, &self.gpu_overlay, cx),
            ));
        } else {
            self.gpu_overlay.borrow_mut().take();
        }

        // Bottom-right: editor and renderer diagnostics. Its timer refreshes
        // the isolated overlay in place without invalidating the viewport.
        if view.overlays.state.show_nerds_overlay {
            overlays = overlays.child(
                div()
                    .absolute()
                    .bottom_2()
                    .right_2()
                    // The cached Nerds view is width-full. Give its absolute
                    // positioning wrapper a concrete width so that width-full
                    // resolves to the panel width instead of the whole editor.
                    .w(px(230.0))
                    .child(render_nerds_overlay(
                        view_arc.clone(),
                        gpu_engine,
                        self.input_state.clone(),
                        &self.nerds_overlay,
                        cx,
                    )),
            );
        } else {
            self.nerds_overlay.borrow_mut().take();
        }

        // Debug passes can advertise a floating scale for a mode. Query only
        // while a debug mode is selected; lit view mode has no legend.
        let active_legend = (view.editor.viewport_debug_mode != 0)
            .then(|| {
                gpu_engine
                    .lock()
                    .ok()
                    .and_then(|engine| engine.editor_mailbox())
                    .and_then(|mailbox| {
                        mailbox
                            .viewport_debug_views()
                            .into_iter()
                            .find(|view_mode| {
                                view_mode.debug_mode == view.editor.viewport_debug_mode
                            })
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
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
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
