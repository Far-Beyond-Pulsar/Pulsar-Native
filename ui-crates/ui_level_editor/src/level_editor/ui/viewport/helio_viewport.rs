//! HelioViewport — GPUI Render component that renders Helio 3D scenes.
//!
//! Follows the reference example `wgpu_surface_basic.rs`:
//!   1. Create `WgpuSurfaceHandle` lazily on first render.
//!   2. Each frame: `back_view_with_size()` → render → `swap_buffers()`.
//!   3. Return `wgpu_surface(handle)` in the element tree so GPUI composits it.

use std::sync::{Arc, Mutex};

use gpui::*;
use engine_backend::services::gpu_renderer::GpuRenderer;

/// A GPUI component that drives the Helio renderer into a `WgpuSurfaceHandle`.
pub struct HelioViewport {
    pub gpu_engine: Arc<Mutex<GpuRenderer>>,
    surface: Option<WgpuSurfaceHandle>,
    focus_handle: FocusHandle,
    debug_replace_with_yellow: bool,
}

impl HelioViewport {
    pub fn new<V: 'static>(
        gpu_engine: Arc<Mutex<GpuRenderer>>,
        debug_replace_with_yellow: bool,
        cx: &mut Context<V>,
    ) -> Self {
        Self {
            gpu_engine,
            surface: None,
            focus_handle: cx.focus_handle(),
            debug_replace_with_yellow,
        }
    }
}

impl Focusable for HelioViewport {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for HelioViewport {}

impl Render for HelioViewport {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.debug_replace_with_yellow {
            return div()
                .relative()
                .size_full()
                .track_focus(&self.focus_handle)
                .id("helio_viewport_debug_yellow")
                .bg(rgb(0xffff00))
                .into_any_element();
        }

        // Keep rendering continuously for real-time 3D viewport updates.
        window.request_animation_frame();

        let format = wgpu::TextureFormat::Rgba8UnormSrgb;

        // Lazy surface creation (once).
        if self.surface.is_none() {
            match window.create_wgpu_surface(1600, 900, format) {
                Some(s) => {
                    tracing::info!("[HELIO-VIEWPORT] WgpuSurface created (format={:?})", format);
                    self.surface = Some(s);
                }
                None => {
                    tracing::warn!("[HELIO-VIEWPORT] create_wgpu_surface returned None");
                }
            }
        }

        // Render into the back buffer, then swap.
        if let Some(ref surface) = self.surface {
            if let Some((view, (w, h))) = surface.back_view_with_size() {
                if let Ok(mut engine) = self.gpu_engine.try_lock() {
                    engine.render_frame_to_surface(
                        surface.device(),
                        surface.queue(),
                        &view,
                        w,
                        h,
                        surface.format(),
                    );
                }
                drop(view);
                surface.swap_buffers();
            }
        }

        // Element tree fills the parent panel. The WGPU surface is composited
        // as an absolute layer inside this full-size container.
        if let Some(ref surface) = self.surface {
            div()
                .relative()
                .size_full()
                .track_focus(&self.focus_handle)
                .id("helio_viewport")
                .child(
                    wgpu_surface(surface.clone())
                        .absolute()
                        .inset_0(),
                )
                .into_any_element()
        } else {
            div()
                .relative()
                .track_focus(&self.focus_handle)
                .id("helio_viewport")
                .size_full()
                .bg(rgb(0xff0000))
                .child("Waiting for WgpuSurface...")
                .into_any_element()
        }
    }
}
