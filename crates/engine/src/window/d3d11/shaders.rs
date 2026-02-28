//! HLSL Shader Source Code
//!
//! This module contains HLSL shader source code for compositing GPUI and Bevy
//! textures using GPU alpha blending on Windows.
//!
//! ## Shaders
//!
//! - **Vertex Shader** - Simple passthrough that transforms normalized device coordinates
//! - **Pixel Shader** - Samples texture with alpha channel
//!
//! ## Usage
//!
//! These shaders are compiled at runtime using D3DCompile:
//!
//! ```rust,ignore
//! let vs_bytecode = compile_shader(VERTEX_SHADER_SOURCE, "vs_5_0")?;
//! let ps_bytecode = compile_shader(PIXEL_SHADER_SOURCE, "ps_5_0")?;
//! ```
//!
//! ## Rendering Pipeline
//!
//! ```text
//! Fullscreen Quad Vertices
//!          ↓
//!   Vertex Shader (passthrough)
//!          ↓
//!   Rasterization
//!          ↓
//!   Pixel Shader (texture sample)
//!          ↓
//!   Alpha Blending (if enabled)
//!          ↓
//!   Back Buffer
//! ```

/// Vertex shader source code
///
/// A simple passthrough vertex shader that:
/// 1. Takes 2D position and UV coordinates as input
/// 2. Transforms position to clip space (adds Z=0, W=1)
/// 3. Passes UV coordinates unchanged to pixel shader
///
/// ## Input Layout
/// - POSITION: float2 (normalized device coordinates -1 to 1)
/// - TEXCOORD0: float2 (texture coordinates 0 to 1)
///
/// ## Output
/// - SV_POSITION: float4 (clip space position)
/// - TEXCOORD0: float2 (texture coordinates for pixel shader)
pub const VERTEX_SHADER_SOURCE: &str = r#"
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

/// Pixel shader source code
///
/// Samples a texture at the interpolated UV coordinates and returns the color.
/// The alpha channel is preserved for alpha blending.
///
/// ## Resources
/// - t0: Texture2D gpuiTexture (the UI or 3D texture to sample)
/// - s0: SamplerState (linear filtering, clamp addressing)
///
/// ## Input
/// - SV_POSITION: float4 (pixel position, unused)
/// - TEXCOORD0: float2 (texture coordinates from vertex shader)
///
/// ## Output
/// - SV_TARGET: float4 (RGBA color with alpha)
pub const PIXEL_SHADER_SOURCE: &str = r#"
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
