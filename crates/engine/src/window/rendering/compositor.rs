//! D3D11 GPU composition and rendering
//!
//! This module contains the complete D3D11 composition logic for rendering
//! multiple layers (background, Bevy 3D, GPUI UI) to the screen with zero-copy
//! GPU texture sharing.
//! 
//! WARNING: This module is Windows-only and will soon be depricated as we
//!          transition to WGPUI which will allow gpui-internal surfaces

use winit::window::WindowId;
use crate::window::WinitGpuiApp;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering};

#[cfg(target_os = "windows")]
use windows::{
    core::*,
    Win32::{
        Foundation::HANDLE,
        Graphics::{
            Direct3D::*,
            Direct3D11::*,
            Dxgi::Common::*,
            Dxgi::*,
        },
    },
};

/// Handle window redraw with complete D3D11 composition
///
/// This function implements 3-layer GPU composition:
/// - Layer 0 (bottom): Black background
/// - Layer 1 (middle): Bevy 3D rendering (opaque, from shared D3D12→D3D11 texture)
/// - Layer 2 (top): GPUI UI (transparent, alpha-blended)
///
/// Features:
/// - Zero-copy GPU texture sharing between Bevy (D3D12) and compositor (D3D11)
/// - Lazy GPUI rendering (only when needs_render is true)
/// - Continuous Bevy rendering for real-time 3D viewports
/// - Device error recovery and diagnostics
/// - Texture size mismatch handling (stretching)
///
/// # Arguments
/// * `app` - The application state
/// * `window_id` - ID of the window to redraw
#[cfg(target_os = "windows")]
pub unsafe fn handle_redraw(app: &mut WinitGpuiApp, window_id: WindowId) {
    profiling::profile_scope!("Render::Composite");

    // Track frame time for profiler (thread-safe atomic storing microseconds since program start)
    use std::time::SystemTime;
    static LAST_FRAME_TIME_MICROS: AtomicU64 = AtomicU64::new(0);

    let now_micros = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;

    let last_micros = LAST_FRAME_TIME_MICROS.swap(now_micros, Ordering::Relaxed);
    if last_micros != 0 {
        let frame_time_ms = (now_micros.saturating_sub(last_micros)) as f32 / 1000.0;
        profiling::record_frame_time(frame_time_ms);
    }
    
    // Claim Bevy renderer first (needs mutable app reference)
    {
        profiling::profile_scope!("Render::ClaimBevy");
        claim_helio_renderer(app, &window_id);
    }

    // Now get window state mutably
    let window_state = app.windows.get_mut(&window_id);
    let Some(window_state) = window_state else {
        return;
    };

    let should_render_gpui = window_state.needs_render;

    // Diagnostic: Show decoupled rendering rates (thread-safe atomics)
    static COMPOSITOR_FRAME_COUNT: AtomicU32 = AtomicU32::new(0);
    static GPUI_FRAME_COUNT: AtomicU32 = AtomicU32::new(0);
    COMPOSITOR_FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
    if should_render_gpui {
        GPUI_FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    // Render GPUI if needed
    if should_render_gpui {
        profiling::profile_scope!("GPU::GPUI::Render");
        if let Some(gpui_window_ref) = &window_state.gpui_window {
            {
                profiling::profile_scope!("GPU::GPUI::RefreshWindows");
                let _ = window_state.gpui_app.update(|app| {
                    app.refresh_windows();
                });
            }
            {
                profiling::profile_scope!("GPU::GPUI::DrawWindows");
                let _ = window_state.gpui_app.update(|app| {
                    app.draw_windows();
                });
            }
        }
        window_state.needs_render = false;
    }

    // Lazy initialization of shared texture on first render
    if !window_state.shared_texture_initialized && window_state.gpui_window.is_some() && window_state.d3d_device.is_some() {
        profiling::profile_scope!("GPU::Compositor::InitSharedTexture");
        initialize_shared_texture(window_state);
    }

    // Perform D3D11 composition
    {
        profiling::profile_scope!("GPU::Compositor::ComposeFrame");
        compose_frame(window_state, should_render_gpui);
    }

    // Request continuous redraws if we have a Bevy renderer
    if window_state.helio_renderer.is_some() {
        window_state.winit_window.request_redraw();
    }
}

/// Non-Windows stub
#[cfg(not(target_os = "windows"))]
pub fn handle_redraw(_app: &mut WinitGpuiApp, _window_id: WindowId) {
    // D3D11 composition is Windows-only
}

#[cfg(target_os = "windows")]
unsafe fn initialize_shared_texture(window_state: &mut crate::window::WindowState) {
    let Some(gpui_window_ref) = window_state.gpui_window.as_ref() else {
        tracing::error!("❌ Cannot initialize shared texture: GPUI window not available");
        return;
    };

    let Some(device) = window_state.d3d_device.as_ref() else {
        tracing::error!("❌ Cannot initialize shared texture: D3D11 device not available");
        return;
    };

    let handle_result = window_state.gpui_app.update(|app| {
        gpui_window_ref.update(app, |_view, window, _cx| {
            window.get_shared_texture_handle()
        })
    });

    if let Ok(opt_handle_ptr) = handle_result {
        if let Ok(Some(handle_ptr)) = opt_handle_ptr {
            tracing::debug!("✨ Got shared texture handle from GPUI: {:?}", handle_ptr);

            let handle_value: isize = *(&handle_ptr as *const _ as *const isize);
            let mut texture: Option<ID3D11Texture2D> = None;
            let result = device.OpenSharedResource(
                HANDLE(handle_value as *mut _),
                &mut texture
            );

            match result {
                Ok(_) => {
                    if let Some(shared_texture_val) = texture {
                        let mut desc = D3D11_TEXTURE2D_DESC::default();
                        shared_texture_val.GetDesc(&mut desc);

                        desc.MiscFlags = D3D11_RESOURCE_MISC_FLAG(0).0 as u32;
                        desc.Usage = D3D11_USAGE_DEFAULT;
                        desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;

                        let mut persistent_texture: Option<ID3D11Texture2D> = None;
                        let create_result = device.CreateTexture2D(&desc, None, Some(&mut persistent_texture));

                        if create_result.is_ok() {
                            if let Some(tex) = &persistent_texture {
                                let mut srv: Option<ID3D11ShaderResourceView> = None;
                                let srv_result = device.CreateShaderResourceView(tex, None, Some(&mut srv));

                                if srv_result.is_ok() && srv.is_some() {
                                    window_state.persistent_gpui_srv = srv;
                                    tracing::debug!("✨ Created cached SRV for persistent texture");
                                } else {
                                    tracing::error!("❌ Failed to create SRV: {:?}", srv_result);
                                }

                                window_state.persistent_gpui_texture = persistent_texture;
                                tracing::debug!("✨ Created persistent GPUI texture buffer!");
                            } else {
                                tracing::error!("❌ Persistent texture is None despite successful creation");
                            }
                        } else {
                            tracing::error!("❌ Failed to create persistent texture: {:?}", create_result);
                        }

                        window_state.shared_texture = Some(shared_texture_val);
                        window_state.shared_texture_initialized = true;
                        tracing::debug!("✨ Opened shared texture in winit D3D11 device!");
                    }
                }
                Err(e) => {
                    tracing::debug!("❌ Failed to open shared texture: {:?}", e);
                    window_state.shared_texture_initialized = true;
                }
            }
        } else {
            tracing::debug!("⏳ GPUI hasn't created shared texture yet, will retry next frame");
        }
    }
}

#[cfg(target_os = "windows")]
unsafe fn claim_helio_renderer(app: &mut WinitGpuiApp, window_id: &WindowId) {
    let window_state = app.windows.get_mut(window_id).unwrap();

    if window_state.helio_renderer.is_none() {
        let Some(window_id_u64) = app.window_id_map.get_id(window_id) else {
            tracing::warn!("⚠️ Window ID not registered for Bevy renderer claim");
            return;
        };

        // Try to get renderer for this window
        if let Some(handle) = app.engine_context.renderers.get(window_id_u64) {
            if let Some(helio_renderer) = handle.as_bevy::<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>() {
                window_state.helio_renderer = Some(helio_renderer.clone());
            }
        } else if let Some(handle) = app.engine_context.renderers.get(0) {
            // Claim renderer from window ID 0 (pending renderer)
            if let Some(helio_renderer) = handle.as_bevy::<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>() {
                let new_handle = engine_state::TypedRendererHandle::bevy(window_id_u64, helio_renderer.clone());
                app.engine_context.renderers.register(window_id_u64, new_handle);
                app.engine_context.renderers.unregister(0);
                window_state.helio_renderer = Some(helio_renderer.clone());
            }
        }
    }
}

#[cfg(target_os = "windows")]
unsafe fn compose_frame(window_state: &mut crate::window::WindowState, should_render_gpui: bool) {
    let context = match window_state.d3d_context.as_ref() {
        Some(ctx) => ctx.clone(),
        None => return,
    };

    let Some(shared_texture) = window_state.shared_texture.clone() else { return };
    let Some(persistent_texture) = window_state.persistent_gpui_texture.clone() else { return };
    let Some(srv) = window_state.persistent_gpui_srv.clone() else { return };
    let Some(swap_chain) = window_state.swap_chain.clone() else { return };
    let Some(render_target_view) = window_state.render_target_view.clone() else { return };
    let Some(blend_state) = window_state.blend_state.clone() else { return };
    let Some(vertex_shader) = window_state.vertex_shader.clone() else { return };
    let Some(pixel_shader) = window_state.pixel_shader.clone() else { return };
    let Some(vertex_buffer) = window_state.vertex_buffer.clone() else { return };
    let Some(input_layout) = window_state.input_layout.clone() else { return };
    let Some(sampler_state) = window_state.sampler_state.clone() else { return };

    // Check device status periodically (thread-safe atomic)
    static DEVICE_CHECK_COUNTER: AtomicU32 = AtomicU32::new(0);
    let counter = DEVICE_CHECK_COUNTER.fetch_add(1, Ordering::Relaxed);
    if counter % 300 == 0 {
        if let Some(device) = window_state.d3d_device.as_ref() {
            let device_reason = device.GetDeviceRemovedReason();
            if device_reason.is_err() {
                tracing::error!("[COMPOSITOR] ⚠️ D3D11 device has been removed! Reason: {:?}", device_reason);
                window_state.bevy_texture = None;
                window_state.bevy_srv = None;
            }
        }
    }

    // Copy GPUI texture if it rendered this frame
    if should_render_gpui {
        context.CopyResource(&persistent_texture, &shared_texture);
    }

    // Clear to black
    let black = [0.0f32, 0.0, 0.0, 1.0];
    context.ClearRenderTargetView(&render_target_view, &black);
    context.OMSetRenderTargets(Some(&[Some(render_target_view.clone())]), None);

    // LAYER 1: Draw Bevy texture
    render_bevy_layer(window_state, &context);

    // LAYER 2: Draw GPUI texture with alpha blending
    let blend_factor = [0.0f32, 0.0, 0.0, 0.0];
    context.OMSetBlendState(Some(&blend_state), Some(&blend_factor), 0xffffffff);

    context.VSSetShader(&vertex_shader, None);
    context.PSSetShader(&pixel_shader, None);
    context.IASetInputLayout(&input_layout);

    let stride = 16u32;
    let offset = 0u32;
    context.IASetVertexBuffers(0, 1, Some(&Some(vertex_buffer.clone())), Some(&stride), Some(&offset));
    context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
    context.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
    context.PSSetSamplers(0, Some(&[Some(sampler_state.clone())]));

    let size = window_state.winit_window.inner_size();
    let viewport = D3D11_VIEWPORT {
        TopLeftX: 0.0,
        TopLeftY: 0.0,
        Width: size.width as f32,
        Height: size.height as f32,
        MinDepth: 0.0,
        MaxDepth: 1.0,
    };
    context.RSSetViewports(Some(&[viewport]));
    context.Draw(4, 0);

    // Present
    let present_result = swap_chain.Present(1, DXGI_PRESENT(0));
    if present_result.is_err() {
        static PRESENT_ERROR_COUNT: AtomicU32 = AtomicU32::new(0);
        let count = PRESENT_ERROR_COUNT.fetch_add(1, Ordering::Relaxed);
        if count == 0 || count % 600 == 0 {
            tracing::error!("[COMPOSITOR] ❌ Present failed: {:?}", present_result);
        }
    }
}

#[cfg(target_os = "windows")]
unsafe fn render_bevy_layer(window_state: &mut crate::window::WindowState, context: &ID3D11DeviceContext) {
    use gpui::GpuTextureHandle;

    let Some(gpu_renderer_arc) = window_state.helio_renderer.clone() else { return };
    let Ok(gpu_renderer) = gpu_renderer_arc.lock() else { return };
    let Some(ref helio_renderer_inst) = gpu_renderer.helio_renderer else { return };
    let Some(gpu_handle) = helio_renderer_inst.get_current_native_handle() else { return };

    // GpuTextureHandle stores the handle as an isize internally
    // We need to access it - for now, assume it's in the native_handle field
    let handle_ptr = gpu_handle.native_handle as usize;

    let mut bevy_texture_local: Option<ID3D11Texture2D> = None;
    let device = window_state.d3d_device.as_ref().unwrap();

    let open_result: windows::core::Result<()> = match device.cast::<ID3D11Device1>() {
        Ok(device1) => {
            let result: windows::core::Result<ID3D11Texture2D> = device1.OpenSharedResource1(HANDLE(handle_ptr as *mut _));
            match result {
                Ok(tex) => {
                    bevy_texture_local = Some(tex);
                    Ok(())
                }
                Err(e) => Err(e)
            }
        }
        Err(_) => {
            device.OpenSharedResource(HANDLE(handle_ptr as *mut _), &mut bevy_texture_local)
        }
    };

    if let Err(e) = open_result {
        let hresult = e.code().0;
        let is_device_error = hresult == 0x887A0005_u32 as i32 || hresult == 0x887A0006_u32 as i32 || hresult == 0x887A0007_u32 as i32;

        static OPEN_ERROR_COUNT: AtomicU32 = AtomicU32::new(0);
        static LAST_WAS_DEVICE_ERROR: AtomicBool = AtomicBool::new(false);
        let count = OPEN_ERROR_COUNT.fetch_add(1, Ordering::Relaxed);

        if is_device_error {
            let was_device_error = LAST_WAS_DEVICE_ERROR.swap(true, Ordering::Relaxed);
            if !was_device_error || count % 600 == 1 {
                tracing::error!("[COMPOSITOR] ❌ GPU DEVICE REMOVED/SUSPENDED: {:?}", e);
            }
            window_state.bevy_texture = None;
            window_state.bevy_srv = None;
        } else {
            LAST_WAS_DEVICE_ERROR.store(false, Ordering::Relaxed);
            if count == 0 || count % 60 == 0 {
                tracing::error!("[COMPOSITOR] ❌ Failed to open Bevy shared resource: {:?} (count: {})", e, count + 1);
            }
        }
        return;
    }

    let Some(ref bevy_tex) = bevy_texture_local else { return };

    // Validate size
    let mut bevy_tex_desc = D3D11_TEXTURE2D_DESC::default();
    bevy_tex.GetDesc(&mut bevy_tex_desc as *mut D3D11_TEXTURE2D_DESC);
    let window_size = window_state.winit_window.inner_size();

    if bevy_tex_desc.Width != window_size.width || bevy_tex_desc.Height != window_size.height {
        static SIZE_MISMATCH_COUNT: AtomicU32 = AtomicU32::new(0);
        SIZE_MISMATCH_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    // Create or reuse SRV
    if window_state.bevy_texture.is_none() || window_state.bevy_texture.as_ref().map(|t| t.as_raw()) != Some(bevy_tex.as_raw()) {
        create_bevy_srv(window_state, bevy_tex);
    }

    // Draw Bevy texture
    if let Some(ref bevy_shader_view) = window_state.bevy_srv {
        let vertex_shader = window_state.vertex_shader.as_ref().unwrap();
        let pixel_shader = window_state.pixel_shader.as_ref().unwrap();
        let input_layout = window_state.input_layout.as_ref().unwrap();
        let vertex_buffer = window_state.vertex_buffer.as_ref().unwrap();
        let sampler_state = window_state.sampler_state.as_ref().unwrap();

        context.OMSetBlendState(None, None, 0xffffffff);
        context.VSSetShader(vertex_shader, None);
        context.PSSetShader(pixel_shader, None);
        context.IASetInputLayout(input_layout);

        let stride = 16u32;
        let offset = 0u32;
        context.IASetVertexBuffers(0, 1, Some(&Some(vertex_buffer.clone())), Some(&stride), Some(&offset));
        context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
        context.PSSetShaderResources(0, Some(&[Some(bevy_shader_view.clone())]));
        context.PSSetSamplers(0, Some(&[Some(sampler_state.clone())]));

        let size = window_state.winit_window.inner_size();
        let viewport = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: size.width as f32,
            Height: size.height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        context.RSSetViewports(Some(&[viewport]));
        context.Draw(4, 0);
    }
}

#[cfg(target_os = "windows")]
unsafe fn create_bevy_srv(window_state: &mut crate::window::WindowState, bevy_tex: &ID3D11Texture2D) {
    let device = window_state.d3d_device.as_ref().unwrap();

    let srv_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_SRV {
                MostDetailedMip: 0,
                MipLevels: 1,
            },
        },
    };

    let mut new_srv: Option<ID3D11ShaderResourceView> = None;
    let srv_result = device.CreateShaderResourceView(bevy_tex, Some(&srv_desc), Some(&mut new_srv));

    if let Err(e) = srv_result {
        let hresult = e.code().0;
        let is_device_error = hresult == 0x887A0005_u32 as i32 || hresult == 0x887A0006_u32 as i32 || hresult == 0x887A0007_u32 as i32;

        static SRV_ERROR_COUNT: AtomicU32 = AtomicU32::new(0);
        let count = SRV_ERROR_COUNT.fetch_add(1, Ordering::Relaxed);
        if is_device_error {
            if count == 0 || count % 600 == 0 {
                tracing::error!("[COMPOSITOR] ❌ GPU device error creating SRV: {:?}", e);
            }
            window_state.bevy_texture = None;
            window_state.bevy_srv = None;
        } else if count == 0 || count % 60 == 0 {
            tracing::error!("[COMPOSITOR] ❌ Failed to create SRV: {:?} (count: {})", e, count + 1);
        }
    }

    window_state.bevy_texture = Some(bevy_tex.clone());
    window_state.bevy_srv = new_srv;
}
