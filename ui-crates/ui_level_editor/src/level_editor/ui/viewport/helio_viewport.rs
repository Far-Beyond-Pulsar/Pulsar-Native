//! HelioViewport — GPUI Render component that renders Helio 3D scenes.
//!
//! This follows the exact same pattern as the reference example `wgpu_surface.rs`:
//!   1. Call `window.create_wgpu_surface()` lazily on first render.
//!   2. Each frame: `surface.back_view_with_size()` → render → `swap_buffers()`.
//!   3. Include `wgpu_surface(handle)` in the element tree so GPUI composits it.

use std::sync::{Arc, Mutex};

use gpui::*;
use gpui::prelude::FluentBuilder;
use engine_backend::services::gpu_renderer::GpuRenderer;

/// A GPUI component that drives the Helio renderer into a `WgpuSurfaceHandle`.
///
/// Drop this as a child element wherever you want the 3D viewport to appear.
pub struct HelioViewport {
    /// Shared handle to the GPU renderer (owns the Helio `Renderer`).
    pub gpu_engine: Arc<Mutex<GpuRenderer>>,

    /// The GPUI-managed back-buffer surface.  Lazily created on first `render()`.
    surface: Option<WgpuSurfaceHandle>,

    /// GPUI focus handle so keyboard events can be routed here.
    focus_handle: FocusHandle,
}

impl HelioViewport {
    /// Create a new viewport.  Pass the same `Arc<Mutex<GpuRenderer>>` used
    /// everywhere else in the editor so the scene is shared.
    pub fn new<V: 'static>(
        gpu_engine: Arc<Mutex<GpuRenderer>>,
        cx: &mut Context<V>,
    ) -> Self {
        Self {
            gpu_engine,
            surface: None,
            focus_handle: cx.focus_handle(),
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
        // Drive continuous animation — GPUI will call render() every frame.
        window.request_animation_frame();

        let format = wgpu::TextureFormat::Rgba8UnormSrgb;

        // ── Lazy surface creation ──────────────────────────────────────────
        // Mirrors the reference example: the surface is created once, on the
        // first render() call, when the window is guaranteed to exist.
        if self.surface.is_none() {
            match window.create_wgpu_surface(1600, 900, format) {
                Some(s) => {
                    tracing::info!("[HELIO-VIEWPORT] ✅ WgpuSurface created (format={:?})", format);
                    self.surface = Some(s);
                }
                None => {
                    tracing::warn!("[HELIO-VIEWPORT] ❌ create_wgpu_surface returned None");
                }
            }
        }

        // ── Per-frame render ───────────────────────────────────────────────
        // Exactly matches the reference:
        //   if let Some((view, (dw, dh))) = self.surface.back_view_with_size() { … }
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

        // ── Element tree ───────────────────────────────────────────────────
        div()
            .size_full()
            .track_focus(&self.focus_handle)
            .id("helio_viewport")
            .when_some(self.surface.clone(), |el, surface| {
                el.child(
                    wgpu_surface(surface)
                        .absolute()
                        .inset_0(),
                )
            })
    }
}
