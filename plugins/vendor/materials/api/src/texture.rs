//! A material texture as a host uploads it (an image, its format and how it
//! is sampled) and the transform a material applies to a texture's UVs.
//! The host decodes images into [`TextureUpload`]s; the scene's texture
//! store keeps the GPU textures, and the materials' table binds them.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureSamplerDesc {
    pub address_mode_u: wgpu::AddressMode,
    pub address_mode_v: wgpu::AddressMode,
    pub address_mode_w: wgpu::AddressMode,
    pub mag_filter: wgpu::FilterMode,
    pub min_filter: wgpu::FilterMode,
    pub mipmap_filter: wgpu::MipmapFilterMode,
}

impl Default for TextureSamplerDesc {
    fn default() -> Self {
        Self {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TextureUpload {
    pub label: Option<String>,
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    pub data: Vec<u8>,
    pub sampler: TextureSamplerDesc,
}

impl TextureUpload {
    pub fn rgba8(
        label: impl Into<String>,
        width: u32,
        height: u32,
        srgb: bool,
        data: Vec<u8>,
        sampler: TextureSamplerDesc,
    ) -> Self {
        Self {
            label: Some(label.into()),
            width,
            height,
            format: if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            data,
            sampler,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextureTransform {
    pub offset: [f32; 2],
    pub scale: [f32; 2],
    pub rotation_radians: f32,
}

impl Default for TextureTransform {
    fn default() -> Self {
        Self {
            offset: [0.0, 0.0],
            scale: [1.0, 1.0],
            rotation_radians: 0.0,
        }
    }
}
