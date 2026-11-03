//! GPU decal types.

use bytemuck::{Pod, Zeroable};

/// GPU decal type discriminant.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecalType {
    AlbedoNormal = 0,
    NormalOnly = 1,
    Emissive = 2,
    All = 3,
}

/// GPU decal blend mode.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecalBlendMode {
    Translucent = 0,
    AlphaBlend = 1,
    Additive = 2,
    Multiply = 3,
}

/// Per-decal GPU data. 128 bytes.
///
/// # WGSL equivalent
/// ```wgsl
/// struct GpuDecal {
///     transform:          mat4x4<f32>,  // world-to-decal-space (affine, ortho projection)
///     color:              vec4<f32>,    // tint + opacity
///     albedo_texture_index: u32,        // bindless index, 0xFFFFFFFF = use color only
///     normal_texture_index: u32,
///     roughness_texture_index: u32,
///     metalness_texture_index: u32,
///     blend_mode:         u32,          // DecalBlendMode
///     decal_type:         u32,          // DecalType
///     fade_time:          f32,          // lifetime (0 = persistent)
///     fade_start_delay:   f32,          // seconds before fade begins
///     age:                f32,          // updated per frame on CPU or GPU
///     normal_adapt:       u32,          // 1 = adapt normal to surface orientation
///     _pad0:              f32,
///     _pad1:              f32,
/// }
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuDecal {
    /// World-to-decal-space transform (affine, ortho projection)
    pub transform: [f32; 16],
    /// Tint color (xyz) + opacity (w)
    pub color: [f32; 4],
    /// Bindless texture index for albedo (u32::MAX = use color only)
    pub albedo_texture_index: u32,
    /// Bindless texture index for normal map
    pub normal_texture_index: u32,
    /// Bindless texture index for roughness
    pub roughness_texture_index: u32,
    /// Bindless texture index for metalness
    pub metalness_texture_index: u32,
    /// DecalBlendMode discriminant
    pub blend_mode: u32,
    /// DecalType discriminant
    pub decal_type: u32,
    /// Fade duration in seconds (0 = persistent)
    pub fade_time: f32,
    /// Seconds before fade begins
    pub fade_start_delay: f32,
    /// Age in seconds (updated per frame)
    pub age: f32,
    /// 1 = adapt decal normal perturbation to match the underlying surface normal
    pub normal_adapt: u32,
    pub _pad0: f32,
    pub _pad1: f32,
}
