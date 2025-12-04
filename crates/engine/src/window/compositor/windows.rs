//! Windows D3D11 Compositor Implementation
//!
//! Implements GPU composition using Direct3D 11 with zero-copy shared texture integration.
//!
//! ## Architecture
//!
//! ```text
//! ┌───────────────────────────────────────────┐
//! │     D3D11 Compositor Pipeline             │
//! ├───────────────────────────────────────────┤
//! │ 1. Clear back buffer (black background)   │
//! │ 2. Draw Bevy texture (opaque, DX12→DX11)  │
//! │ 3. Draw GPUI texture (alpha-blended)      │
//! │ 4. Present swap chain                     │
//! └───────────────────────────────────────────┘
//! ```
//!
//! ## Shared Texture Handling
//!
//! - **Bevy (DX12)**: Uses `OpenSharedResource1` with NT handles
//! - **GPUI (DX11)**: Uses legacy `OpenSharedResource` (GetSharedHandle API)
//! - Both are opened into the same D3D11 device for zero-copy composition

use super::{Compositor, CompositorState};
use anyhow::{Context, Result};
use engine_backend::subsystems::render::NativeTextureHandle;
use gpui::SharedTextureHandle;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ptr;
use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::{
            Direct3D::Fxc::*,
            Direct3D::*,
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
    },
};

/// Direct3D 11 compositor for Windows
pub struct D3D11Compositor {
    /// Compositor state
    state: CompositorState,

    /// D3D11 device for rendering
    device: ID3D11Device,

    /// D3D11 immediate context
    context: ID3D11DeviceContext,

    /// Swap chain for presenting to window
    swap_chain: IDXGISwapChain1,

    /// Render target view (back buffer)
    render_target_view: ID3D11RenderTargetView,

    /// Blend state for alpha compositing
    blend_state: ID3D11BlendState,

    /// Vertex shader for fullscreen quad
    vertex_shader: ID3D11VertexShader,

    /// Pixel shader for texture sampling
    pixel_shader: ID3D11PixelShader,

    /// Vertex buffer (fullscreen quad)
    vertex_buffer: ID3D11Buffer,

    /// Input layout
    input_layout: ID3D11InputLayout,

    /// Sampler state for texture filtering
    sampler_state: ID3D11SamplerState,

    // === GPUI Texture State ===
    /// Shared GPUI texture (opened from GPUI's D3D11 device)
    gpui_shared_texture: Option<ID3D11Texture2D>,

    /// Persistent copy of GPUI texture (for frame persistence)
    gpui_persistent_texture: Option<ID3D11Texture2D>,

    /// Cached shader resource view for GPUI texture
    gpui_srv: Option<ID3D11ShaderResourceView>,

    /// Whether GPUI shared texture has been initialized
    gpui_initialized: bool,

    // === Bevy Texture State ===
    /// Bevy texture (opened from Bevy's D3D12 device via NT handle)
    bevy_texture: Option<ID3D11Texture2D>,

    /// Shader resource view for Bevy texture
    bevy_srv: Option<ID3D11ShaderResourceView>,
}

impl D3D11Compositor {
    /// Compile and create shaders for fullscreen quad rendering
    unsafe fn create_shaders(
        device: &ID3D11Device,
    ) -> Result<(ID3D11VertexShader, ID3D11PixelShader, Vec<u8>)> {
        // Vertex shader: Fullscreen quad with position and texcoord
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

        // Pixel shader: Sample texture with alpha
        let ps_source = r#"
Texture2D tex : register(t0);
SamplerState samplerState : register(s0);

struct PS_INPUT {
    float4 pos : SV_POSITION;
    float2 tex : TEXCOORD0;
};

float4 main(PS_INPUT input) : SV_TARGET {
    return tex.Sample(samplerState, input.tex);
}
"#;

        // Compile vertex shader
        let vs_blob = {
            let mut blob: Option<ID3DBlob> = None;
            let mut error_blob: Option<ID3DBlob> = None;
            D3DCompile(
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
            )
            .context("Failed to compile vertex shader")?;

            blob.context("Vertex shader blob is None")?
        };

        // Compile pixel shader
        let ps_blob = {
            let mut blob: Option<ID3DBlob> = None;
            let mut error_blob: Option<ID3DBlob> = None;
            D3DCompile(
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
            )
            .context("Failed to compile pixel shader")?;

            blob.context("Pixel shader blob is None")?
        };

        // Extract bytecode
        let vs_bytecode = std::slice::from_raw_parts(
            vs_blob.GetBufferPointer() as *const u8,
            vs_blob.GetBufferSize(),
        );
        let ps_bytecode = std::slice::from_raw_parts(
            ps_blob.GetBufferPointer() as *const u8,
            ps_blob.GetBufferSize(),
        );

        // Create shader objects
        let vertex_shader = device.CreateVertexShader(vs_bytecode, None)?;
        let pixel_shader = device.CreatePixelShader(ps_bytecode, None)?;

        Ok((vertex_shader, pixel_shader, vs_bytecode.to_vec()))
    }

    /// Create input layout matching vertex shader
    unsafe fn create_input_layout(
        device: &ID3D11Device,
        vs_bytecode: &[u8],
    ) -> Result<ID3D11InputLayout> {
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

        device
            .CreateInputLayout(&layout, vs_bytecode)
            .context("Failed to create input layout")
    }

    /// Create vertex buffer for fullscreen quad
    unsafe fn create_vertex_buffer(device: &ID3D11Device) -> Result<ID3D11Buffer> {
        #[repr(C)]
        struct Vertex {
            pos: [f32; 2],
            tex: [f32; 2],
        }

        let vertices = [
            Vertex {
                pos: [-1.0, -1.0],
                tex: [0.0, 1.0],
            },
            Vertex {
                pos: [-1.0, 1.0],
                tex: [0.0, 0.0],
            },
            Vertex {
                pos: [1.0, -1.0],
                tex: [1.0, 1.0],
            },
            Vertex {
                pos: [1.0, 1.0],
                tex: [1.0, 0.0],
            },
        ];

        let desc = D3D11_BUFFER_DESC {
            ByteWidth: std::mem::size_of_val(&vertices) as u32,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_VERTEX_BUFFER,
            CPUAccessFlags: D3D11_CPU_ACCESS_FLAG(0),
            MiscFlags: D3D11_RESOURCE_MISC_FLAG(0),
            StructureByteStride: 0,
        };

        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: vertices.as_ptr() as *const _,
            SysMemPitch: 0,
            SysMemSlicePitch: 0,
        };

        device
            .CreateBuffer(&desc, Some(&data))
            .context("Failed to create vertex buffer")
    }

    /// Create blend state for alpha compositing
    unsafe fn create_blend_state(device: &ID3D11Device) -> Result<ID3D11BlendState> {
        let desc = D3D11_BLEND_DESC {
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

        device
            .CreateBlendState(&desc)
            .context("Failed to create blend state")
    }

    /// Create sampler state for texture filtering
    unsafe fn create_sampler_state(device: &ID3D11Device) -> Result<ID3D11SamplerState> {
        let desc = D3D11_SAMPLER_DESC {
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

        device
            .CreateSamplerState(&desc)
            .context("Failed to create sampler state")
    }

    /// Initialize GPUI shared texture from handle
    unsafe fn init_gpui_texture(&mut self, handle: &SharedTextureHandle) -> Result<()> {
        if self.gpui_initialized {
            return Ok(());
        }

        // Extract NT handle from GPUI SharedTextureHandle
        let nt_handle = match handle {
            SharedTextureHandle::D3D11NTHandle { handle, .. } => *handle as isize,
            _ => return Ok(()), // Not a Windows handle, skip
        };

        // Open shared texture using legacy API (GPUI uses GetSharedHandle)
        let mut shared_texture: Option<ID3D11Texture2D> = None;
        self.device
            .OpenSharedResource(HANDLE(nt_handle as *mut _), &mut shared_texture)
            .context("Failed to open GPUI shared texture")?;

        if let Some(shared_tex) = shared_texture {
            // Get texture description
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            shared_tex.GetDesc(&mut desc);

            // Create persistent copy (not shared)
            desc.MiscFlags = D3D11_RESOURCE_MISC_FLAG(0).0 as u32;
            desc.Usage = D3D11_USAGE_DEFAULT;
            desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;

            let persistent_texture = self.device.CreateTexture2D(&desc, None)?;

            // Create SRV for persistent texture
            let srv = self
                .device
                .CreateShaderResourceView(&persistent_texture, None)?;

            self.gpui_shared_texture = Some(shared_tex);
            self.gpui_persistent_texture = Some(persistent_texture);
            self.gpui_srv = Some(srv);
            self.gpui_initialized = true;

            tracing::info!("✅ Initialized GPUI shared texture in compositor");
        }

        Ok(())
    }

    /// Draw a texture using fullscreen quad
    unsafe fn draw_texture(&mut self, srv: &ID3D11ShaderResourceView, enable_blend: bool) {
        // Set blend state
        if enable_blend {
            self.context
                .OMSetBlendState(Some(&self.blend_state), None, 0xffffffff);
        } else {
            self.context.OMSetBlendState(None, None, 0xffffffff);
        }

        // Set shaders
        self.context.VSSetShader(&self.vertex_shader, None);
        self.context.PSSetShader(&self.pixel_shader, None);

        // Set input layout
        self.context.IASetInputLayout(&self.input_layout);

        // Set vertex buffer
        let stride = 16u32;
        let offset = 0u32;
        self.context.IASetVertexBuffers(
            0,
            1,
            Some(&Some(self.vertex_buffer.clone())),
            Some(&stride),
            Some(&offset),
        );

        // Set topology
        self.context
            .IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);

        // Set texture and sampler
        self.context
            .PSSetShaderResources(0, Some(&[Some(srv.clone())]));
        self.context
            .PSSetSamplers(0, Some(&[Some(self.sampler_state.clone())]));

        // Set viewport
        let viewport = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: self.state.width as f32,
            Height: self.state.height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        self.context.RSSetViewports(Some(&[viewport]));

        // Draw fullscreen quad
        self.context.Draw(4, 0);
    }
}

impl Compositor for D3D11Compositor {
    fn init(
        window: &impl HasWindowHandle,
        width: u32,
        height: u32,
        scale_factor: f32,
    ) -> Result<Self> {
        unsafe {
            tracing::info!(
                "🎨 Initializing D3D11 compositor: {}x{} @ {}x",
                width,
                height,
                scale_factor
            );

            // Create D3D11 device
            let mut device = None;
            let mut context = None;
            let mut feature_level = Default::default();

            D3D11CreateDevice(
                None,
                D3_DRIVER_TYPE_HARDWARE,
                HMODULE(ptr::null_mut()),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut feature_level),
                Some(&mut context),
            )
            .context("Failed to create D3D11 device")?;

            let device = device.context("D3D11 device is None")?;
            let context = context.context("D3D11 context is None")?;

            // Get window handle
            let hwnd = match window.window_handle()?.as_raw() {
                RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as isize as *mut _),
                _ => anyhow::bail!("Expected Win32 window handle"),
            };

            // Create swap chain
            let dxgi_device: IDXGIDevice = device.cast()?;
            let adapter = dxgi_device.GetAdapter()?;
            let dxgi_factory: IDXGIFactory2 = adapter.GetParent()?;

            let swap_chain_desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
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
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: 0,
            };

            let swap_chain = dxgi_factory
                .CreateSwapChainForHwnd(&device, hwnd, &swap_chain_desc, None, None)
                .context("Failed to create swap chain")?;

            // Create render target view
            let back_buffer: ID3D11Texture2D = swap_chain.GetBuffer(0)?;
            let render_target_view = device.CreateRenderTargetView(&back_buffer, None)?;

            // Create shaders and pipeline resources
            let (vertex_shader, pixel_shader, vs_bytecode) = Self::create_shaders(&device)?;
            let input_layout = Self::create_input_layout(&device, &vs_bytecode)?;
            let vertex_buffer = Self::create_vertex_buffer(&device)?;
            let blend_state = Self::create_blend_state(&device)?;
            let sampler_state = Self::create_sampler_state(&device)?;

            tracing::info!("✅ D3D11 compositor initialized successfully");

            Ok(Self {
                state: CompositorState {
                    width,
                    height,
                    scale_factor,
                    needs_render: true,
                },
                device,
                context,
                swap_chain,
                render_target_view,
                blend_state,
                vertex_shader,
                pixel_shader,
                vertex_buffer,
                input_layout,
                sampler_state,
                gpui_shared_texture: None,
                gpui_persistent_texture: None,
                gpui_srv: None,
                gpui_initialized: false,
                bevy_texture: None,
                bevy_srv: None,
            })
        }
    }

    fn begin_frame(&mut self) -> Result<()> {
        unsafe {
            // Clear to black background
            let black = [0.0f32, 0.0, 0.0, 1.0];
            self.context
                .ClearRenderTargetView(&self.render_target_view, &black);

            // Set render target
            self.context
                .OMSetRenderTargets(Some(&[Some(self.render_target_view.clone())]), None);
        }

        Ok(())
    }

    fn composite_bevy(&mut self, handle: &NativeTextureHandle) -> Result<Option<()>> {
        unsafe {
            // Extract D3D11 handle (NT handle from D3D12)
            let nt_handle = match handle {
                NativeTextureHandle::D3D11(h) => *h as isize,
                _ => return Ok(None),
            };

            // Try to open Bevy's D3D12 shared texture using D3D11.1 API
            let device1: ID3D11Device1 = self.device.cast()?;
            let bevy_texture: ID3D11Texture2D = device1
                .OpenSharedResource1(HANDLE(nt_handle as *mut _))
                .context("Failed to open Bevy shared texture")?;

            // Validate texture size matches window
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            bevy_texture.GetDesc(&mut desc);

            if desc.Width != self.state.width || desc.Height != self.state.height {
                // Size mismatch - Bevy hasn't resized yet, skip this frame
                return Ok(None);
            }

            // Create or reuse SRV
            let needs_new_srv = self
                .bevy_texture
                .as_ref()
                .map_or(true, |t| t.as_raw() != bevy_texture.as_raw());

            if needs_new_srv {
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

                let srv = self
                    .device
                    .CreateShaderResourceView(&bevy_texture, Some(&srv_desc))?;
                self.bevy_texture = Some(bevy_texture);
                self.bevy_srv = Some(srv);
            }

            // Draw Bevy texture (opaque, no blending)
            if let Some(srv) = &self.bevy_srv {
                self.draw_texture(srv, false);
            }
        }

        Ok(Some(()))
    }

    fn composite_gpui(&mut self, handle: &SharedTextureHandle, should_render: bool) -> Result<()> {
        unsafe {
            // Initialize GPUI texture on first use
            if !self.gpui_initialized {
                self.init_gpui_texture(handle)?;
            }

            // Copy from shared texture to persistent buffer if GPUI rendered
            if should_render {
                if let (Some(shared), Some(persistent)) =
                    (&self.gpui_shared_texture, &self.gpui_persistent_texture)
                {
                    self.context.CopyResource(persistent, shared);
                }
            }

            // Draw GPUI texture (alpha-blended on top)
            if let Some(srv) = &self.gpui_srv {
                self.draw_texture(srv, true);
            }
        }

        Ok(())
    }

    fn present(&mut self) -> Result<()> {
        unsafe {
            self.swap_chain
                .Present(1, DXGI_PRESENT(0))
                .ok()
                .context("Failed to present swap chain")
        }
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        unsafe {
            tracing::info!("🔄 Resizing D3D11 compositor to {}x{}", width, height);

            // Release render target view
            drop(self.render_target_view.clone());

            // Flush context
            self.context.Flush();

            // Resize swap chain buffers
            self.swap_chain
                .ResizeBuffers(2, width, height, DXGI_FORMAT_B8G8R8A8_UNORM, 0)
                .context("Failed to resize swap chain")?;

            // Recreate render target view
            let back_buffer: ID3D11Texture2D = self.swap_chain.GetBuffer(0)?;
            self.render_target_view = self.device.CreateRenderTargetView(&back_buffer, None)?;

            // Mark GPUI texture for reinitialization
            self.gpui_initialized = false;
            self.gpui_shared_texture = None;
            self.gpui_persistent_texture = None;
            self.gpui_srv = None;

            // Update state
            self.state.width = width;
            self.state.height = height;

            tracing::info!("✅ D3D11 compositor resized successfully");
        }

        Ok(())
    }

    fn state(&self) -> &CompositorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut CompositorState {
        &mut self.state
    }
}
