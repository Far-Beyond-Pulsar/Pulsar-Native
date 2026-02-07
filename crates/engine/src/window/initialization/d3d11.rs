//! D3D11 rendering pipeline initialization
//!
//! This module contains the complete logic for initializing the D3D11 rendering
//! pipeline including device, swap chain, shaders, and all rendering resources.

#[cfg(target_os = "windows")]
use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::{
            Direct3D::*,
            Direct3D11::*,
            Direct3D::Fxc::*,
            Dxgi::{Common::*, *},
        },
    },
};

#[cfg(target_os = "windows")]
use raw_window_handle::{RawWindowHandle, HasWindowHandle};
#[cfg(target_os = "windows")]
use winit::window::WindowId;
#[cfg(target_os = "windows")]
use crate::window::{WinitGpuiApp, WindowState};

/// Initialize complete D3D11 rendering pipeline
///
/// This function sets up the entire D3D11 rendering pipeline for a window:
/// 1. Create D3D11 device and context
/// 2. Get DXGI factory and adapter
/// 3. Create swap chain for the window HWND
/// 4. Create render target view from back buffer
/// 5. Create blend state for alpha compositing
/// 6. Compile vertex and pixel shaders at runtime
/// 7. Create input layout matching vertex shader
/// 8. Create fullscreen quad vertex buffer
/// 9. Create texture sampler state
///
/// # Arguments
/// * `app` - The application state
/// * `window_id` - ID of the window to initialize D3D11 for
///
/// # Safety
/// This function uses unsafe Windows API calls and must only be called on Windows.
#[cfg(target_os = "windows")]
pub unsafe fn initialize_d3d11_pipeline(
    app: &mut WinitGpuiApp,
    window_id: &WindowId,
) {
    profiling::profile_scope!("Window::InitD3D11");

    let window_state = app.windows.get_mut(window_id).expect("Window state must exist");
    let winit_window = window_state.winit_window.clone();
    let size = winit_window.inner_size();

    tracing::debug!("✨ Initializing D3D11 for GPU blitting...");

    let mut device = None;
    let mut context = None;
    let mut feature_level = Default::default();

    let result = D3D11CreateDevice(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        HMODULE(std::ptr::null_mut()),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        None,
        D3D11_SDK_VERSION,
        Some(&mut device),
        Some(&mut feature_level),
        Some(&mut context),
    );

    // Extract device and validate before proceeding
    let Some(device) = device else {
        tracing::error!("❌ Failed to create D3D11 device");
        return;
    };

    if result.is_err() {
        tracing::error!("❌ D3D11CreateDevice failed: {:?}", result);
        return;
    }

    let window_state = app.windows.get_mut(window_id).expect("Window state must exist");
    window_state.d3d_device = Some(device.clone());
    window_state.d3d_context = context;
    tracing::debug!("✨ D3D11 device created successfully!");

    // Create swap chain for the winit window
    let parent_raw = match winit_window.window_handle() {
        Ok(handle) => handle.as_raw(),
        Err(e) => {
            tracing::error!("❌ Failed to get window handle: {:?}", e);
            return;
        }
    };

    let hwnd = match parent_raw {
        RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as isize as *mut _),
        _ => {
            tracing::error!("❌ Window handle is not Win32");
            return;
        }
    };

    // Get DXGI interfaces with proper error handling
    let dxgi_device: IDXGIDevice = match device.cast() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("❌ Failed to cast device to IDXGIDevice: {:?}", e);
            return;
        }
    };

    let adapter = match dxgi_device.GetAdapter() {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("❌ Failed to get DXGI adapter: {:?}", e);
            return;
        }
    };

    let dxgi_factory: IDXGIFactory2 = match adapter.GetParent() {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("❌ Failed to get DXGI factory: {:?}", e);
            return;
        }
    };

        // Swap chain must use physical pixels
        let physical_width = size.width;
        let physical_height = size.height;
        tracing::debug!("🖼️ Creating swap chain: physical {}x{}, scale {}",
            physical_width, physical_height, winit_window.scale_factor());

        let swap_chain_desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: physical_width,
            Height: physical_height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: FALSE,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_NONE,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,  // Ignore alpha for solid window
            Flags: 0,
        };

        let swap_chain = dxgi_factory.CreateSwapChainForHwnd(
            &device,
            hwnd,
            &swap_chain_desc,
            None,
            None,
        );

        if let Ok(swap_chain) = swap_chain {
            let window_state = app.windows.get_mut(window_id).expect("Window state must exist");
            window_state.swap_chain = Some(swap_chain.clone());
            tracing::debug!("✨ Swap chain created for winit window!");

            // Create render target view from swap chain back buffer
            if let Ok(back_buffer) = swap_chain.GetBuffer::<ID3D11Texture2D>(0) {
                let mut rtv: Option<ID3D11RenderTargetView> = None;
                if device.CreateRenderTargetView(&back_buffer, None, Some(&mut rtv as *mut _)).is_ok() {
                    window_state.render_target_view = rtv;
                    tracing::debug!("✨ Render target view created!");
                } else {
                    tracing::error!("❌ Failed to create render target view");
                }
            } else {
                tracing::error!("❌ Failed to get back buffer from swap chain");
            }

            // Create blend state for alpha blending
            let blend_desc = D3D11_BLEND_DESC {
                AlphaToCoverageEnable: FALSE,
                IndependentBlendEnable: FALSE,
                RenderTarget: [
                    D3D11_RENDER_TARGET_BLEND_DESC {
                        BlendEnable: TRUE,
                        SrcBlend: D3D11_BLEND_SRC_ALPHA,
                        DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                        BlendOp: D3D11_BLEND_OP_ADD,
                        SrcBlendAlpha: D3D11_BLEND_ONE,
                        DestBlendAlpha: D3D11_BLEND_ZERO,
                        BlendOpAlpha: D3D11_BLEND_OP_ADD,
                        RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
                    },
                    D3D11_RENDER_TARGET_BLEND_DESC::default(),
                    D3D11_RENDER_TARGET_BLEND_DESC::default(),
                    D3D11_RENDER_TARGET_BLEND_DESC::default(),
                    D3D11_RENDER_TARGET_BLEND_DESC::default(),
                    D3D11_RENDER_TARGET_BLEND_DESC::default(),
                    D3D11_RENDER_TARGET_BLEND_DESC::default(),
                    D3D11_RENDER_TARGET_BLEND_DESC::default(),
                ],
            };

            let mut blend_state = None;
            if device.CreateBlendState(&blend_desc, Some(&mut blend_state as *mut _)).is_ok() {
                window_state.blend_state = blend_state;
                tracing::debug!("✨ Blend state created for alpha composition!");
            } else {
                tracing::error!("❌ Failed to create blend state");
            }

            // Create shaders for GPU alpha blending by compiling HLSL at runtime
            tracing::debug!("🔧 Compiling shaders at runtime...");

            // Vertex shader source: passthrough with position and texcoord
            let vs_source = r#"
struct VS_INPUT {
    float2 pos : POSITION;
    float2 tex : TEXCOORD0;
};

struct PS_INPUT {
    float4 pos : SV_POSITION;
    float2 tex : TEXCOORD0;
};

PS_INPUT main(VS_INPUT input) {
    PS_INPUT output;
    output.pos = float4(input.pos, 0.0f, 1.0f);
    output.tex = input.tex;
    return output;
}
"#;

            // Pixel shader source: sample texture with alpha
            let ps_source = r#"
Texture2D gpuiTexture : register(t0);
SamplerState samplerState : register(s0);

struct PS_INPUT {
    float4 pos : SV_POSITION;
    float2 tex : TEXCOORD0;
};

float4 main(PS_INPUT input) : SV_TARGET {
    return gpuiTexture.Sample(samplerState, input.tex);
}
"#;

            // Compile vertex shader
            let vs_bytecode_blob = {
                let mut blob: Option<ID3DBlob> = None;
                let mut error_blob: Option<ID3DBlob> = None;
                let result = D3DCompile(
                    vs_source.as_ptr() as *const _,
                    vs_source.len(),
                    None,
                    None,
                    None,
                    s!("main"),
                    s!("vs_5_0"),
                    0,
                    0,
                    &mut blob,
                    Some(&mut error_blob),
                );

                if result.is_err() {
                    if let Some(err) = error_blob {
                        let err_msg = std::slice::from_raw_parts(
                            err.GetBufferPointer() as *const u8,
                            err.GetBufferSize(),
                        );
                        tracing::debug!("❌ VS compile error: {}", String::from_utf8_lossy(err_msg));
                    }
                }
                blob
            };

            // Compile pixel shader
            let ps_bytecode_blob = {
                let mut blob: Option<ID3DBlob> = None;
                let mut error_blob: Option<ID3DBlob> = None;
                let result = D3DCompile(
                    ps_source.as_ptr() as *const _,
                    ps_source.len(),
                    None,
                    None,
                    None,
                    s!("main"),
                    s!("ps_5_0"),
                    0,
                    0,
                    &mut blob,
                    Some(&mut error_blob),
                );

                if result.is_err() {
                    if let Some(err) = error_blob {
                        let err_msg = std::slice::from_raw_parts(
                            err.GetBufferPointer() as *const u8,
                            err.GetBufferSize(),
                        );
                        tracing::debug!("❌ PS compile error: {}", String::from_utf8_lossy(err_msg));
                    }
                }
                blob
            };

            let vs_bytecode = if let Some(blob) = &vs_bytecode_blob {
                std::slice::from_raw_parts(
                    blob.GetBufferPointer() as *const u8,
                    blob.GetBufferSize(),
                )
            } else {
                &[] as &[u8]
            };

            let ps_bytecode = if let Some(blob) = &ps_bytecode_blob {
                std::slice::from_raw_parts(
                    blob.GetBufferPointer() as *const u8,
                    blob.GetBufferSize(),
                )
            } else {
                &[] as &[u8]
            };

            if vs_bytecode.is_empty() || ps_bytecode.is_empty() {
                tracing::debug!("❌ Shader compilation failed!");
            }

            // Create D3D11 shader objects from compiled bytecode
            let mut vertex_shader = None;
            let mut pixel_shader = None;

            let vs_result = if !vs_bytecode.is_empty() {
                device.CreateVertexShader(vs_bytecode, None, Some(&mut vertex_shader as *mut _))
            } else {
                Err(Error::from(E_FAIL))
            };

            let ps_result = if !ps_bytecode.is_empty() {
                device.CreatePixelShader(ps_bytecode, None, Some(&mut pixel_shader as *mut _))
            } else {
                Err(Error::from(E_FAIL))
            };

            if vs_result.is_ok() && ps_result.is_ok() {
                window_state.vertex_shader = vertex_shader;
                window_state.pixel_shader = pixel_shader;
                tracing::debug!("✨ Shaders created from bytecode!");
            } else {
                tracing::debug!("❌ Failed to create shaders - VS: {:?}, PS: {:?}", vs_result, ps_result);
            }

            if window_state.vertex_shader.is_some() && window_state.pixel_shader.is_some() {
                // Create input layout that matches the vertex shader
                let layout = [
                    D3D11_INPUT_ELEMENT_DESC {
                        SemanticName: s!("POSITION"),
                        SemanticIndex: 0,
                        Format: DXGI_FORMAT_R32G32_FLOAT,
                        InputSlot: 0,
                        AlignedByteOffset: 0,
                        InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                        InstanceDataStepRate: 0,
                    },
                    D3D11_INPUT_ELEMENT_DESC {
                        SemanticName: s!("TEXCOORD"),
                        SemanticIndex: 0,
                        Format: DXGI_FORMAT_R32G32_FLOAT,
                        InputSlot: 0,
                        AlignedByteOffset: 8,
                        InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                        InstanceDataStepRate: 0,
                    },
                ];

                let mut input_layout = None;
                if device.CreateInputLayout(&layout, vs_bytecode, Some(&mut input_layout as *mut _)).is_ok() {
                    window_state.input_layout = input_layout;
                    tracing::debug!("✨ Input layout created!");
                } else {
                    tracing::error!("❌ Failed to create input layout");
                }
            }

            // Create vertex buffer for fullscreen quad
            #[repr(C)]
            struct Vertex {
                pos: [f32; 2],
                tex: [f32; 2],
            }

            let vertices = [
                Vertex { pos: [-1.0, -1.0], tex: [0.0, 1.0] },
                Vertex { pos: [-1.0,  1.0], tex: [0.0, 0.0] },
                Vertex { pos: [ 1.0, -1.0], tex: [1.0, 1.0] },
                Vertex { pos: [ 1.0,  1.0], tex: [1.0, 0.0] },
            ];

            let vb_desc = D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of_val(&vertices) as u32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
                StructureByteStride: 0,
            };

            let vb_data = D3D11_SUBRESOURCE_DATA {
                pSysMem: vertices.as_ptr() as *const _,
                SysMemPitch: 0,
                SysMemSlicePitch: 0,
            };

            let mut vertex_buffer = None;
            if device.CreateBuffer(&vb_desc, Some(&vb_data), Some(&mut vertex_buffer as *mut _)).is_ok() {
                window_state.vertex_buffer = vertex_buffer;
                tracing::debug!("✨ Vertex buffer created!");
            } else {
                tracing::error!("❌ Failed to create vertex buffer");
            }

            // Create sampler state
            let sampler_desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MipLODBias: 0.0,
                MaxAnisotropy: 1,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                BorderColor: [0.0, 0.0, 0.0, 0.0],
                MinLOD: 0.0,
                MaxLOD: f32::MAX,
            };

            let mut sampler_state = None;
            if device.CreateSamplerState(&sampler_desc, Some(&mut sampler_state as *mut _)).is_ok() {
                window_state.sampler_state = sampler_state;
                tracing::debug!("✨ Sampler state created!");
            } else {
                tracing::error!("❌ Failed to create sampler state");
            }

            tracing::debug!("🎉 D3D11 pipeline initialization complete!");
            tracing::debug!("💡 Shared texture will be retrieved on first render");
        } else {
            tracing::error!("❌ Failed to create swap chain");
        }
}

/// Stub for non-Windows platforms
#[cfg(not(target_os = "windows"))]
pub fn initialize_d3d11_pipeline(_app: &mut crate::window::WinitGpuiApp, _window_id: &winit::window::WindowId) {
    // D3D11 is Windows-only
}
