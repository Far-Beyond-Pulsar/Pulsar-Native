//! Floating viewport controls, performance HUDs and debug-view legends.

use super::*;

impl ViewportPanel {
    /// Shared state changes must invalidate the floating controls even when
    /// Helio only composites a new frame without rebuilding the UI.
    pub(crate) fn controls_signature(&self, state: &LevelEditorState, cx: &App) -> impl PartialEq {
        let editor = &state.editor;
        let overlays = &state.overlays.state;
        let positions = &state.overlays.positions;
        (
            (
                editor.current_tool,
                editor.camera_mode,
                editor.viewport_debug_mode,
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
            ),
            (
                positions.viewport,
                positions.camera,
                positions.is_dragging_viewport,
                positions.is_dragging_camera,
            ),
            self.toolbar.read(cx).view_mode_picker.entity_id(),
        )
    }

    /// Render all viewport overlays.
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

        // The command toolbar owns the retained controls and their subscriptions.
        let toolbar = self.toolbar.read(cx);
        let picker = toolbar.view_mode_picker.clone();
        let view_mode_label = toolbar.view_mode_label(cx);
        let snaps = toolbar.snap_panels.clone();
        if state.overlays.state.show_viewport_options {
            overlays = overlays.child(
                div()
                    .absolute()
                    .top(px(state.overlays.positions.viewport.1))
                    .left(px(state.overlays.positions.viewport.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                    .child(components::viewport_options::render_viewport_options(
                        state,
                        state_arc.clone(),
                        &snaps,
                        state.overlays.positions.is_dragging_viewport,
                        cx,
                    )),
            );
        }
        if state.overlays.state.show_camera_mode_selector {
            overlays = overlays.child(
                div()
                    .absolute()
                    .top(px(state.overlays.positions.camera.1))
                    .right(px(state.overlays.positions.camera.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                    .child(components::camera_selector::render_camera_selector(
                        state,
                        state_arc.clone(),
                        state.editor.camera_mode,
                        self.input_state.clone(),
                        &picker,
                        view_mode_label,
                        state.overlays.positions.is_dragging_camera,
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
        if state.overlays.state.show_performance_overlay {
            overlays = overlays.child(div().absolute().bottom_2().left_2().max_w(px(400.0)).child(
                render_performance_overlay(
                    state,
                    state_arc.clone(),
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
        if state.overlays.state.show_gpu_pipeline_overlay {
            let overlay_div = if state.overlays.state.show_performance_overlay {
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
        if state.overlays.state.show_nerds_overlay {
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
                        state_arc,
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
        let active_legend = (state.editor.viewport_debug_mode != 0)
            .then(|| {
                gpu_engine
                    .lock()
                    .ok()
                    .and_then(|engine| engine.editor_mailbox())
                    .and_then(|mailbox| {
                        mailbox
                            .viewport_debug_views()
                            .into_iter()
                            .find(|view| view.debug_mode == state.editor.viewport_debug_mode)
                            .and_then(|view| view.legend)
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

        overlays
    }
}
