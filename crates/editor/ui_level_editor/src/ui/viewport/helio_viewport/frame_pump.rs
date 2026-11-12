use super::*;

impl HelioViewport {
    /// Start the once-per-view `on_next_frame` pump that repaints this view when
    /// the runtime has published a frame.
    ///
    /// Notifying only on a counter change is what keeps the rest of the editor
    /// cached: `Window::mark_view_dirty` walks the ancestor path, so every
    /// notify here also invalidates the viewport panel, the tab panel, the
    /// workspace and `LevelEditorPanel`. At Helio's ~60 FPS that is one such
    /// cascade per rendered frame instead of one per event-loop iteration.
    ///
    /// `awaiting_render` throttles the pump to at most one outstanding repaint.
    pub(super) fn start_frame_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;

        crate::ui::frame_pump::spawn_frame_pump(&cx.entity(), window, |this, window, cx| {
            this.poll_display_refresh(window, cx);

            // The loading overlay shows while the runtime builds its renderer
            // (the first build, the level's pipeline, a swap): a build compiles
            // pipelines and the view shows its last frame meanwhile. It ends
            // when the build is done or failed (the runtime's task says why).
            match (this.runtime.is_building(), this.initial_load.is_some()) {
                (true, false) => {
                    let loading = this
                        .shared_state
                        .read()
                        .editor_loading
                        .begin("Building the render pipeline…");
                    loading.set_progress(0.05);
                    this.initial_load = Some(loading);
                }
                (false, true)
                    if this.runtime.frames() > 0
                        || matches!(
                            this.runtime.state(),
                            engine_backend::services::render_runtimes::RuntimeState::Failed(_)
                        ) =>
                {
                    if let Some(loading) = this.initial_load.take() {
                        loading.set_progress(1.0);
                    }
                }
                _ => {}
            }

            let loading = this.shared_state.read().editor_loading.snapshot();
            if loading != this.last_loading_snapshot {
                this.last_loading_snapshot = loading;
                this.awaiting_render = true;
                cx.notify();
                return;
            }

            let screenshot_processing = this.shared_state.read().build.screenshot_processing;
            if screenshot_processing != this.last_screenshot_processing {
                this.last_screenshot_processing = screenshot_processing;
                this.awaiting_render = true;
                cx.notify();
                return;
            }

            let published = this.frames_published.load(Ordering::Acquire);
            if published == this.last_published_frame {
                return;
            }
            this.last_published_frame = published;

            // A published texture changes no element state, so the frame
            // only needs compositing — `refresh_buffers` marks the window
            // dirty without marking any view dirty, and every cached view
            // replays instead of rebuilding. Scene content (gizmo drags,
            // camera moves) reaches the texture through that path alone;
            // it never needs a view rebuild.
            //
            // But a view that never prepaints never observes new bounds,
            // and `WgpuSurface::prepaint` is what resizes the surface to
            // match its element. So a real notify is issued when the
            // window viewport size changed since the last full render
            // (window resize / maximize), or once per
            // `FULL_RENDER_INTERVAL` of wall-clock time as the fallback for
            // geometry changes with no size signal — panel splits and
            // undocks. That bounds their pickup latency without letting
            // the ancestor-chain rebuild dominate idle frames.
            let viewport_resized =
                Some(window.viewport_size()) != this.viewport_size_at_last_full_render;
            if this.last_full_render.elapsed() >= FULL_RENDER_INTERVAL
                || this.awaiting_render
                || viewport_resized
            {
                this.viewport_size_at_last_full_render = Some(window.viewport_size());
                this.awaiting_render = true;
                cx.notify();
            } else {
                window.refresh_buffers();
            }
        });
    }
}

/// Published frames between full re-renders of the viewport view.
///
/// Between these the pump uses `Window::refresh_buffers`, which composites the
/// new texture without invalidating any view. The periodic full render exists
/// solely so `WgpuSurface::prepaint` runs often enough to keep the surface
/// sized to its element — a purely buffer-refreshed viewport never observes
/// new bounds. Window-level resizes bypass the wait entirely (the pump
/// compares `Window::viewport_size` against the last full render), so this
/// interval only governs geometry changes with no size signal, such as panel
/// splits.
///
/// A fixed duration rather than a published-frame count so the cost does not
/// scale with the frame rate: one ancestor-chain rebuild (~9 ms measured) every
/// 2.5 seconds at any refresh rate.
const FULL_RENDER_INTERVAL: Duration = Duration::from_millis(2500);
