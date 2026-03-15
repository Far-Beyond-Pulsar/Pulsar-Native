/// Viewport component that composites a helio-render-v2 wgpu surface via WGPUI.
///
/// The surface handle is created by the level editor panel with
/// `window.create_wgpu_surface()` and handed to this viewport.  The helio
/// render thread writes into the surface's back buffer and calls
/// `handle.present()` each frame.  WGPUI composites the front buffer texture
/// automatically via its `surfaces_pipeline`.
///
/// Usage:
///
/// ```rust,ignore
/// // In panel.rs new_internal():
/// let surface = window
///     .create_wgpu_surface(1600, 900, wgpu::TextureFormat::Bgra8UnormSrgb)
///     .expect("WGPUI surface creation failed");
///
/// let viewport = cx.new(|cx| BevyViewport::new(cx));
/// viewport.update(cx, |vp, _cx| vp.set_surface(surface.clone()));
///
/// gpu_renderer_builder = gpu_renderer_builder.surface(surface);
/// ```

use gpui::*;
use gpui::prelude::FluentBuilder;
use std::sync::Arc;

/// Bevy viewport component — renders via a `WgpuSurfaceHandle`.
pub struct BevyViewport {
    surface: Option<WgpuSurfaceHandle>,
    focus_handle: FocusHandle,
}

impl BevyViewport {
    pub fn new<V: 'static>(cx: &mut Context<V>) -> Self {
        Self {
            surface: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Attach the WGPUI wgpu surface.  Call this once from the panel after
    /// `window.create_wgpu_surface()`.
    pub fn set_surface(&mut self, handle: WgpuSurfaceHandle) {
        self.surface = Some(handle);
    }

    pub fn has_surface(&self) -> bool {
        self.surface.is_some()
    }
}

impl Focusable for BevyViewport {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for BevyViewport {}

impl Render for BevyViewport {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        window.request_animation_frame();

        if let Some(ref handle) = self.surface {
            wgpu_surface(handle.clone())
                .size_full()
                .into_any_element()
        } else {
            // Surface not yet assigned — show a dark placeholder
            div()
                .size_full()
                .bg(gpui::rgb(0x111111))
                .into_any_element()
        }
    }
}