//! Viewport panel overlays: performance HUD, camera selector, and options.

use super::*;

impl ViewportPanel {
    /// Render all viewport overlays.
    pub(super) fn render_overlays<V>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        perf_snapshot: PerformanceSnapshot,
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
            .bottom(scrollbar_width)
            // Top-left: Viewport options
            .child(
                div()
                    .absolute()
                    .top(px(state.overlays.positions.viewport.1))
                    .left(px(state.overlays.positions.viewport.0))
                    .child(
                        // Retained as a layer. The viewport panel is notified
                        // several times a second so that `WgpuSurface::prepaint`
                        // observes new bounds (see `helio_viewport`'s frame
                        // pump), and every one of those notifies repaints this
                        // toolbar even though none of them can change it. The
                        // key below is its complete input set, so on those
                        // frames it composites its recorded primitives instead.
                        //
                        // Geometry is not in the key and does not need to be:
                        // dragging the overlay moves the positioning div above,
                        // which changes the layer's bounds, and a layer whose
                        // bounds changed re-renders regardless of its key.
                        div()
                            .id("viewport-options-layer")
                            .layer_keyed(viewport_options_key(state))
                            .child(render_viewport_options(
                                state,
                                state_arc.clone(),
                                state.overlays.positions.is_dragging_viewport,
                                cx,
                            )),
                    ),
            );

        // Top-right: Camera selector
        if state.overlays.state.show_camera_mode_selector {
            // Same reasoning as the viewport options above, with one extra
            // input: the selector displays the live camera move speed, which
            // lives in `input_state` rather than in `LevelEditorState` and so
            // would be invisible to any state-derived key.
            let camera_key = (
                state.overlays.state.camera_mode_selector_collapsed,
                state.editor.camera_mode as u8,
                state.overlays.positions.is_dragging_camera,
                self.input_state.get_move_speed().to_bits(),
            );
            overlays = overlays.child(
                div()
                    .absolute()
                    .top(px(state.overlays.positions.camera.1))
                    .right(px(state.overlays.positions.camera.0))
                    .child(
                        div()
                            .id("camera-selector-layer")
                            .layer_keyed(camera_key)
                            .child(render_camera_selector(
                                state,
                                state_arc.clone(),
                                state.editor.camera_mode,
                                self.input_state.clone(),
                                state.overlays.positions.is_dragging_camera,
                                cx,
                            )),
                    ),
            );
        }

        // Bottom-left: Performance overlay
        if state.overlays.state.show_performance_overlay {
            overlays = overlays.child(div().absolute().bottom_2().left_2().max_w(px(400.0)).child(
                render_performance_overlay(state, state_arc.clone(), perf_snapshot, cx),
            ));
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
                render_gpu_pipeline_overlay(state, state_arc.clone(), gpu_engine, cx),
            ));
        }

        overlays
    }
}