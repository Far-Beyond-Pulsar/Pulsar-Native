//! Direct3D 11 Rendering Integration
//!
//! This module provides Direct3D 11 rendering capabilities for zero-copy composition
//! between GPUI (2D UI) and Bevy (3D rendering) using shared textures on Windows.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────┐
//! │     D3D11 Composition Pipeline      │
//! ├─────────────────────────────────────┤
//! │                                     │
//! │  Layer 0: Green Clear Color         │
//! │           (background)              │
//! │                                     │
//! │  Layer 1: Bevy 3D Content           │
//! │           (D3D12 → D3D11 shared)    │
//! │           (opaque blend)            │
//! │                                     │
//! │  Layer 2: GPUI UI                   │
//! │           (D3D11 shared texture)    │
//! │           (alpha blend)             │
//! │                                     │
//! │           ↓                         │
//! │    Swap Chain Present               │
//! └─────────────────────────────────────┘
//! ```
//!
//! ## Zero-Copy Design
//!
//! - Bevy renders to D3D12 shared texture
//! - D3D11 opens the same texture (no copy)
//! - GPUI renders to D3D11 shared texture
//! - Composition happens entirely on GPU
//! - No CPU-GPU transfers required
//!
//! ## Modules
//!
//! - `shaders` - HLSL shader source code

pub mod shaders;

// Platform-specific implementation
#[cfg(target_os = "windows")]
pub use self::windows_impl::*;

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::shaders::*;
    use raw_window_handle::{ HasWindowHandle, RawWindowHandle };
    use std::sync::Arc;
    use winit::window::Window as WinitWindow;
    use windows::{
        core::*,
        Win32::{
            Foundation::*,
            Graphics::{ Direct3D::*, Direct3D11::*, Direct3D::Fxc::*, Dxgi::{ Common::*, * } },
        },
    };

    /// Initialize Direct3D 11 device, context, and swap chain for a window
    ///
    /// Creates the core D3D11 resources needed for rendering:
    /// - Device (represents the GPU)
    /// - Immediate context (for issuing draw commands)
    /// - Swap chain (for presenting to window)
    ///
    /// # Arguments
    /// * `winit_window` - Window to create swap chain for
    ///
    /// # Returns
    /// Tuple of (device, context, swap_chain) on success, or None if initialization fails
    ///
    /// # Safety
    /// Uses unsafe Windows API calls. The caller must ensure the window remains valid.
    pub unsafe fn initialize_d3d11(
        winit_window: &Arc<WinitWindow>
    ) -> Option<(ID3D11Device, ID3D11DeviceContext, IDXGISwapChain1)> {
        // Get window handle
        let window_handle = winit_window.window_handle().ok()?;
        let hwnd = match window_handle.as_raw() {
            RawWindowHandle::Win32(handle) => {
                // Get the window handle value - hwnd.get() returns isize
                HWND(handle.hwnd.get() as isize as *mut _)
            }
            _ => {
                return None;
            }
        };

        let size = winit_window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        // Create D3D11 device and immediate context
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let mut feature_level = D3D_FEATURE_LEVEL_11_0;

        let create_flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;

        let result = D3D11CreateDevice(
            None, // Use default adapter
            D3D_DRIVER_TYPE_HARDWARE, // Hardware acceleration
            HMODULE::default(), // No software rasterizer
            create_flags, // Creation flags
            Some(&[D3D_FEATURE_LEVEL_11_0]), // Required feature level
            D3D11_SDK_VERSION, // SDK version
            Some(&mut device), // Output device
            Some(&mut feature_level), // Output feature level
            Some(&mut context) // Output context
        );

        if result.is_err() {
            tracing::error!("❌ Failed to create D3D11 device");
            return None;
        }

        let device = device?;
        let context = context?;

        // Get DXGI factory from device for swap chain creation
        let dxgi_device: IDXGIDevice = device.cast().ok()?;
        let dxgi_adapter = dxgi_device.GetAdapter().ok()?;
        let dxgi_factory: IDXGIFactory2 = dxgi_adapter.GetParent().ok()?;

        // Create swap chain descriptor
        let swap_chain_desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM, // 32-bit BGRA
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1, // No multisampling
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2, // Double buffering
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD, // Flip model
            Flags: 0,
            Scaling: DXGI_SCALING_STRETCH,
            AlphaMode: DXGI_ALPHA_MODE_UNSPECIFIED,
            Stereo: false.into(),
        };

        // Create swap chain for window
        let swap_chain = dxgi_factory
            .CreateSwapChainForHwnd(&device, hwnd, &swap_chain_desc, None, None)
            .ok()?;

        Some((device, context, swap_chain))
    }

    /// Compile HLSL shader source code at runtime
    ///
    /// Uses D3DCompile to compile HLSL to bytecode that can be used to create
    /// shader objects.
    ///
    /// # Arguments
    /// * `source` - HLSL source code as string
    /// * `target` - Shader model target (e.g., "vs_5_0" for vertex shader, "ps_5_0" for pixel shader)
    ///
    /// # Returns
    /// Compiled shader bytecode as Vec<u8>, or None if compilation fails
    ///
    /// # Safety
    /// Uses unsafe Windows API calls for shader compilation
    pub unsafe fn compile_shader(source: &str, target: &str) -> Option<Vec<u8>> {
        let mut blob: Option<ID3DBlob> = None;
        let mut error_blob: Option<ID3DBlob> = None;

        let target_cstr = format!("{}\0", target);
        let result = D3DCompile(
            source.as_ptr() as *const _, // Source code
            source.len(), // Source length
            None, // Source name (optional)
            None, // Defines (optional)
            None, // Include handler (optional)
            s!("main"), // Entry point
            PCSTR(target_cstr.as_ptr()), // Target profile
            0, // Flags1
            0, // Flags2
            &mut blob, // Output bytecode
            Some(&mut error_blob) // Output errors
        );

        if result.is_err() {
            if let Some(err) = error_blob {
                let err_msg = std::slice::from_raw_parts(
                    err.GetBufferPointer() as *const u8,
                    err.GetBufferSize()
                );
                tracing::error!("❌ Shader compile error: {}", String::from_utf8_lossy(err_msg));
            }
            return None;
        }

        blob.map(|b| {
            let bytecode = std::slice::from_raw_parts(
                b.GetBufferPointer() as *const u8,
                b.GetBufferSize()
            );
            bytecode.to_vec()
        })
    }

    /// Create vertex and pixel shaders from embedded source code
    ///
    /// Compiles the shaders defined in the `shaders` module and creates
    /// D3D11 shader objects.
    ///
    /// # Arguments
    /// * `device` - D3D11 device to create shaders with
    ///
    /// # Returns
    /// Tuple of (vertex_shader, pixel_shader, vs_bytecode) on success.
    /// The bytecode is needed for creating the input layout.
    ///
    /// # Safety
    /// Uses unsafe D3D11 API calls
    pub unsafe fn create_shaders(
        device: &ID3D11Device
    ) -> Option<(ID3D11VertexShader, ID3D11PixelShader, Vec<u8>)> {
        tracing::debug!("🔨 Compiling shaders at runtime...");

        // Compile both shaders
        let vs_bytecode = compile_shader(VERTEX_SHADER_SOURCE, "vs_5_0")?;
        let ps_bytecode = compile_shader(PIXEL_SHADER_SOURCE, "ps_5_0")?;

        // Create shader objects
        let mut vertex_shader = None;
        let mut pixel_shader = None;

        device.CreateVertexShader(&vs_bytecode, None, Some(&mut vertex_shader as *mut _)).ok()?;
        device.CreatePixelShader(&ps_bytecode, None, Some(&mut pixel_shader as *mut _)).ok()?;

        tracing::debug!("✅ Shaders compiled successfully");

        Some((vertex_shader?, pixel_shader?, vs_bytecode))
    }

    /// Create input layout for vertex shader
    ///
    /// Defines the vertex buffer layout that matches the vertex shader input.
    /// Our vertices have position (float2) and texture coordinates (float2).
    ///
    /// # Arguments
    /// * `device` - D3D11 device
    /// * `vs_bytecode` - Compiled vertex shader bytecode (needed for validation)
    ///
    /// # Returns
    /// Input layout object on success
    ///
    /// # Safety
    /// Uses unsafe D3D11 API calls
    pub unsafe fn create_input_layout(
        device: &ID3D11Device,
        vs_bytecode: &[u8]
    ) -> Option<ID3D11InputLayout> {
        let layout = [
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: s!("POSITION"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32_FLOAT, // 2 floats (X, Y)
                InputSlot: 0,
                AlignedByteOffset: 0, // Starts at offset 0
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: s!("TEXCOORD"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32_FLOAT, // 2 floats (U, V)
                InputSlot: 0,
                AlignedByteOffset: 8, // After position (2 * 4 bytes)
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
        ];

        let mut input_layout = None;
        device.CreateInputLayout(&layout, vs_bytecode, Some(&mut input_layout as *mut _)).ok()?;
        input_layout
    }

    /// Create fullscreen quad vertex buffer
    ///
    /// Creates a vertex buffer containing two triangles (4 vertices) that form
    /// a fullscreen quad in normalized device coordinates.
    ///
    /// Vertex layout:
    /// ```text
    /// (-1, 1)      (1, 1)
    ///    +----------+
    ///    |\         |
    ///    | \        |
    ///    |  \       |
    ///    |   \      |
    ///    |    \     |
    ///    |     \    |
    ///    |      \   |
    ///    |       \  |
    ///    |        \ |
    ///    |         \|
    ///    +----------+
    /// (-1,-1)      (1,-1)
    /// ```
    ///
    /// # Arguments
    /// * `device` - D3D11 device
    ///
    /// # Returns
    /// Vertex buffer on success
    ///
    /// # Safety
    /// Uses unsafe D3D11 API calls
    pub unsafe fn create_vertex_buffer(device: &ID3D11Device) -> Option<ID3D11Buffer> {
        #[repr(C)]
        struct Vertex {
            pos: [f32; 2], // Position
            tex: [f32; 2], // Texture coordinates
        }

        let vertices = [
            Vertex { pos: [-1.0, -1.0], tex: [0.0, 1.0] }, // Bottom-left
            Vertex { pos: [-1.0, 1.0], tex: [0.0, 0.0] }, // Top-left
            Vertex { pos: [1.0, 1.0], tex: [1.0, 0.0] }, // Top-right
            Vertex { pos: [1.0, -1.0], tex: [1.0, 1.0] }, // Bottom-right
        ];

        let buffer_desc = D3D11_BUFFER_DESC {
            ByteWidth: (vertices.len() * std::mem::size_of::<Vertex>()) as u32,
            Usage: D3D11_USAGE_IMMUTABLE, // Static buffer
            BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32, // Vertex buffer
            CPUAccessFlags: 0, // No CPU access needed
            MiscFlags: 0,
            StructureByteStride: 0,
        };

        let init_data = D3D11_SUBRESOURCE_DATA {
            pSysMem: vertices.as_ptr() as *const _,
            SysMemPitch: 0,
            SysMemSlicePitch: 0,
        };

        let mut buffer = None;
        device.CreateBuffer(&buffer_desc, Some(&init_data), Some(&mut buffer as *mut _)).ok()?;
        buffer
    }

    /// Create blend state for alpha compositing
    ///
    /// Configures alpha blending for transparent UI rendering:
    /// - Formula: `dest_color = src_alpha * src_color + (1 - src_alpha) * dest_color`
    /// - This allows UI to be composited over 3D content with proper transparency
    ///
    /// # Arguments
    /// * `device` - D3D11 device
    ///
    /// # Returns
    /// Blend state object on success
    ///
    /// # Safety
    /// Uses unsafe D3D11 API calls
    pub unsafe fn create_blend_state(device: &ID3D11Device) -> Option<ID3D11BlendState> {
        let blend_desc = D3D11_BLEND_DESC {
            AlphaToCoverageEnable: false.into(),
            IndependentBlendEnable: false.into(), // Same blend for all render targets
            RenderTarget: [
                D3D11_RENDER_TARGET_BLEND_DESC {
                    BlendEnable: true.into(),
                    SrcBlend: D3D11_BLEND_SRC_ALPHA, // Multiply source by source alpha
                    DestBlend: D3D11_BLEND_INV_SRC_ALPHA, // Multiply dest by (1 - source alpha)
                    BlendOp: D3D11_BLEND_OP_ADD, // Add the two results
                    SrcBlendAlpha: D3D11_BLEND_ONE, // Keep source alpha
                    DestBlendAlpha: D3D11_BLEND_ZERO, // Ignore dest alpha
                    BlendOpAlpha: D3D11_BLEND_OP_ADD,
                    RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
                },
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
            ],
        };

        let mut blend_state = None;
        device.CreateBlendState(&blend_desc, Some(&mut blend_state as *mut _)).ok()?;
        blend_state
    }

    /// Create texture sampler state
    ///
    /// Configures texture sampling for rendering:
    /// - Linear filtering for smooth scaling
    /// - Clamp addressing to prevent wrapping
    ///
    /// # Arguments
    /// * `device` - D3D11 device
    ///
    /// # Returns
    /// Sampler state object on success
    ///
    /// # Safety
    /// Uses unsafe D3D11 API calls
    pub unsafe fn create_sampler_state(device: &ID3D11Device) -> Option<ID3D11SamplerState> {
        let sampler_desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR, // Linear filtering
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP, // Clamp U coordinate
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP, // Clamp V coordinate
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP, // Clamp W coordinate
            MipLODBias: 0.0,
            MaxAnisotropy: 1,
            ComparisonFunc: D3D11_COMPARISON_NEVER,
            BorderColor: [0.0; 4],
            MinLOD: 0.0,
            MaxLOD: f32::MAX,
        };

        let mut sampler_state = None;
        device.CreateSamplerState(&sampler_desc, Some(&mut sampler_state as *mut _)).ok()?;
        sampler_state
    }
}

// Stub implementation for non-Windows platforms
#[cfg(not(target_os = "windows"))]
mod windows_impl {
    // Empty module for cross-platform compatibility
    // D3D11 is Windows-only
}
