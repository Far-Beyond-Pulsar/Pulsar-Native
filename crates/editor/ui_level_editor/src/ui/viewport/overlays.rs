//! Viewport panel overlays: performance HUDs and debug-view legends.

use super::*;

impl ViewportPanel {
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
