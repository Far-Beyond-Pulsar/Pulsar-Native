//! Window resize handling
//!
//! This module contains the complete logic for handling window resize events,
//! including resizing GPUI and D3D11 resources.

use gpui::{px, DevicePixels};
use winit::dpi::PhysicalSize;
use winit::window::WindowId;
use crate::window::WinitGpuiApp;

#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dxgi::Common::*;
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dxgi::*;
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct3D11::*;

/// Handle window resize events
///
/// This function handles complete window resize logic:
/// 1. Resize GPUI renderer (GPU buffers) - physical pixels
/// 2. Update GPUI logical size (UI layout) - logical pixels
/// 3. Mark GPUI textures for re-initialization
/// 4. Resize D3D11 swap chain (Windows only)
/// 5. Recreate render target view with new back buffer
///
/// # Arguments
/// * `app` - The application state
/// * `window_id` - ID of the window being resized
/// * `new_size` - New physical size of the window
pub fn handle_resize(
    app: &mut WinitGpuiApp,
    window_id: WindowId,
    new_size: PhysicalSize<u32>,
) {
    let window_state = app.windows.get_mut(&window_id);
    let Some(window_state) = window_state else {
        return;
    };

    // Resize GPUI renderer and update logical size
    if let Some(gpui_window_ref) = &window_state.gpui_window {
        let scale_factor = window_state.winit_window.scale_factor() as f32;

        // Physical pixels for renderer (what GPU renders at)
        let physical_size = gpui::size(
            DevicePixels(new_size.width as i32),
            DevicePixels(new_size.height as i32),
        );

        // Logical pixels for GPUI layout (physical / scale)
        let logical_size = gpui::size(
            px(new_size.width as f32 / scale_factor),
            px(new_size.height as f32 / scale_factor),
        );

        let _ = window_state.gpui_app.update(|app| {
            let _ = gpui_window_ref.update(app, |_view, window, _cx| {
                // Resize renderer (GPU buffers) - platform-agnostic
                if let Err(e) = window.resize_renderer(physical_size) {
                    tracing::error!("❌ Failed to resize GPUI renderer: {:?}", e);
                } else {
                    tracing::debug!("✨ Resized GPUI renderer to {:?}", physical_size);

                    // CRITICAL: GPUI recreates its texture when resizing
                    // Mark for re-initialization
                    #[cfg(target_os = "windows")]
                    {
                        window_state.shared_texture_initialized = false;
                        window_state.shared_texture = None;
                        window_state.persistent_gpui_texture = None;
                        window_state.persistent_gpui_srv = None;

                        tracing::debug!("🔄 Marked GPUI shared texture for re-initialization after resize");
                    }
                }

                // Update logical size (for UI layout)
                window.update_logical_size(logical_size);
                tracing::debug!("✨ Updated GPUI logical size to {:?} (scale {})", logical_size, scale_factor);

                // Mark window as dirty to trigger UI re-layout
                window.refresh();
            });
        });
    }

    // Resize D3D11 swap chain to match new window size (Windows only)
    #[cfg(target_os = "windows")]
    unsafe {
        resize_d3d11_swap_chain(window_state, new_size);
    }

    // Request redraw after resize
    window_state.needs_render = true;
    window_state.winit_window.request_redraw();
}

/// Resize D3D11 swap chain and recreate render target view (Windows only)
#[cfg(target_os = "windows")]
unsafe fn resize_d3d11_swap_chain(
    window_state: &mut crate::window::WindowState,
    new_size: PhysicalSize<u32>,
) {
    if let (Some(swap_chain), Some(d3d_device), Some(d3d_context)) =
        (window_state.swap_chain.as_ref(), window_state.d3d_device.as_ref(), window_state.d3d_context.as_ref()) {

        tracing::debug!("🖼️ Resizing D3D11 swap chain to {}x{}", new_size.width, new_size.height);

        // Flush any pending commands to ensure context is clean
        d3d_context.Flush();

        // Must release render target view before resizing
        if window_state.render_target_view.is_some() {
            window_state.render_target_view = None;
            tracing::debug!("🔄 Released render target view before resize");
        }

        // Resize the swap chain buffers
        let resize_result = swap_chain.ResizeBuffers(
            2, // Buffer count (same as creation)
            new_size.width,
            new_size.height,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            DXGI_SWAP_CHAIN_FLAG(0), // Flags
        );

        if let Err(e) = resize_result {
            tracing::error!("❌ Failed to resize swap chain: {:?}", e);
            tracing::error!("❌ This may indicate a device lost condition - rendering may be degraded");
        } else {
            tracing::debug!("✨ Successfully resized swap chain");

            // Recreate render target view with new back buffer
            if let Ok(back_buffer) = swap_chain.GetBuffer::<ID3D11Texture2D>(0) {
                let mut rtv: Option<ID3D11RenderTargetView> = None;
                if d3d_device.CreateRenderTargetView(&back_buffer, None, Some(&mut rtv as *mut _)).is_ok() {
                    window_state.render_target_view = rtv;
                    tracing::debug!("✨ Recreated render target view for resized swap chain");
                } else {
                    tracing::error!("❌ Failed to recreate render target view");
                }
            } else {
                tracing::error!("❌ Failed to get back buffer after resize");
            }
        }
    }
}
