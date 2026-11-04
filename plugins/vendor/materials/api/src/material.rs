//! GPU material types: the material row's ABI, its flags and classes, and
//! how a device binds the material texture table. Every kind that draws a
//! material (the meshes' drawers, the voxel terrain, decals, sprites,
//! portals) reads them from here.

use bytemuck::{Pod, Zeroable};

/// Most material textures a table holds on this platform: the device's
/// texture-table cap ([`helio_core::TEXTURE_TABLE_LIMIT`], which a host
/// asks the device for).
pub const MAX_MATERIAL_TEXTURES: usize = helio_core::TEXTURE_TABLE_LIMIT;

/// Native features the descriptor-indexed material table needs: the
/// device's texture binding arrays ([`helio_core::TEXTURE_BINDING_ARRAYS`],
/// which a host asks for). A partial set uses the baseline bindings.
pub const BINDLESS_MATERIAL_FEATURES: wgpu::Features = helio_core::TEXTURE_BINDING_ARRAYS;

/// Sampled-texture slots reserved for non-material inputs in the most
/// constrained material-consuming stage. Decal collection reads depth plus
/// four G-buffer textures alongside the material table.
pub const EXPANDED_MATERIAL_TEXTURE_RESERVE: usize = 5;

/// GPU material texture binding implementation selected for one device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialBindingMode {
    BindingArray,
    Expanded,
}

/// One authoritative material binding contract shared by the scene and every
/// pass that reads material textures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialBindingConfig {
    pub mode: MaterialBindingMode,
    pub max_textures: usize,
}

impl MaterialBindingConfig {
    pub fn for_device(device: &wgpu::Device) -> Self {
        Self::from_capabilities(device.features(), device.limits())
    }

    /// Select the material binding representation from adapter/device capabilities.
    ///
    /// Keeping this decision pure lets device creation, scene capacity, and pass
    /// construction share one auditable capability contract.
    pub fn from_capabilities(features: wgpu::Features, limits: wgpu::Limits) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let mode = if features.contains(BINDLESS_MATERIAL_FEATURES) {
            MaterialBindingMode::BindingArray
        } else {
            MaterialBindingMode::Expanded
        };
        #[cfg(target_arch = "wasm32")]
        let mode = MaterialBindingMode::Expanded;

        let sampled_texture_limit = limits.max_sampled_textures_per_shader_stage as usize;
        let sampler_limit = limits.max_samplers_per_shader_stage as usize;
        let stage_limit = match mode {
            MaterialBindingMode::BindingArray => sampled_texture_limit.min(sampler_limit),
            MaterialBindingMode::Expanded => sampled_texture_limit
                .saturating_sub(EXPANDED_MATERIAL_TEXTURE_RESERVE)
                .min(sampler_limit),
        };
        let binding_limit = match mode {
            MaterialBindingMode::BindingArray => usize::MAX,
            MaterialBindingMode::Expanded => {
                (limits.max_bindings_per_bind_group as usize).saturating_sub(2) / 2
            }
        };
        let max_textures = MAX_MATERIAL_TEXTURES.min(stage_limit).min(binding_limit);
        assert!(
            max_textures > 0,
            "Helio requires at least one sampled texture and sampler binding"
        );
        Self { mode, max_textures }
    }

    pub fn uses_binding_arrays(self) -> bool {
        self.mode == MaterialBindingMode::BindingArray
    }

    pub fn append_layout_entries(
        self,
        entries: &mut Vec<wgpu::BindGroupLayoutEntry>,
        first_binding: u32,
        visibility: wgpu::ShaderStages,
    ) {
        let texture_entry = |binding, count| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count,
        };
        let sampler_entry = |binding, count| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count,
        };

        match self.mode {
            MaterialBindingMode::BindingArray => {
                let count = std::num::NonZeroU32::new(self.max_textures as u32)
                    .expect("material texture table is non-empty");
                entries.push(texture_entry(first_binding, Some(count)));
                entries.push(sampler_entry(first_binding + 1, Some(count)));
            }
            MaterialBindingMode::Expanded => {
                for index in 0..self.max_textures as u32 {
                    entries.push(texture_entry(first_binding + index, None));
                }
                for index in 0..self.max_textures as u32 {
                    entries.push(sampler_entry(
                        first_binding + self.max_textures as u32 + index,
                        None,
                    ));
                }
            }
        }
    }

    pub fn append_bind_group_entries<'a>(
        self,
        entries: &mut Vec<wgpu::BindGroupEntry<'a>>,
        first_binding: u32,
        texture_views: &'a [&'a wgpu::TextureView],
        samplers: &'a [&'a wgpu::Sampler],
    ) {
        assert_eq!(texture_views.len(), self.max_textures);
        assert_eq!(samplers.len(), self.max_textures);
        match self.mode {
            MaterialBindingMode::BindingArray => {
                entries.push(wgpu::BindGroupEntry {
                    binding: first_binding,
                    resource: wgpu::BindingResource::TextureViewArray(texture_views),
                });
                entries.push(wgpu::BindGroupEntry {
                    binding: first_binding + 1,
                    resource: wgpu::BindingResource::SamplerArray(samplers),
                });
            }
            MaterialBindingMode::Expanded => {
                for (index, view) in texture_views.iter().enumerate() {
                    entries.push(wgpu::BindGroupEntry {
                        binding: first_binding + index as u32,
                        resource: wgpu::BindingResource::TextureView(view),
                    });
                }
                for (index, sampler) in samplers.iter().enumerate() {
                    entries.push(wgpu::BindGroupEntry {
                        binding: first_binding + self.max_textures as u32 + index as u32,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    });
                }
            }
        }
    }
}

/// Material workflow discriminant.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialWorkflow {
    Metallic = 0,
    Specular = 1,
}

/// Feature flags for [`GpuMaterial::flags`].
///
/// Each flag toggles a warp-uniform branch in the generated WGSL; disabled
/// features cost zero instructions via constant-condition elimination.
pub const FLAG_DOUBLE_SIDED: u32 = 1 << 0;
pub const FLAG_ALPHA_BLEND: u32 = 1 << 1;
pub const FLAG_ALPHA_TEST: u32 = 1 << 2;
pub const FLAG_HAS_NORMAL_MAP: u32 = 1 << 3;
pub const FLAG_HAS_CLEAR_COAT: u32 = 1 << 4;
pub const FLAG_HAS_SUBSURFACE: u32 = 1 << 5;
pub const FLAG_HAS_ANISOTROPY: u32 = 1 << 6;
pub const FLAG_HAS_CUSTOM_SHADER: u32 = 1 << 7;
pub const FLAG_TRANSPARENT_ONLY: u32 = 1 << 8;
pub const FLAG_FORWARD_SHADING: u32 = 1 << 9;

/// Material class shader archetypes.
pub const MATERIAL_CLASS_DEFAULT: u32 = 0;
pub const MATERIAL_CLASS_CLEAR_COAT: u32 = 1;
pub const MATERIAL_CLASS_SUBSURFACE: u32 = 2;
pub const MATERIAL_CLASS_ANISOTROPIC: u32 = 3;
pub const MATERIAL_CLASS_SKIN: u32 = 4;
/// Two-sided foliage: light from behind passes through, tinted by the
/// colour; `class_params[0]` is the share of light that passes
/// ([`crate::MaterialRow::foliage`]).
pub const MATERIAL_CLASS_FOLIAGE: u32 = 5;
/// The first class a host registers its own templates at
/// (`RadiantTemplateRegistry::register_str`); the built-ins are below it.
pub const FIRST_CUSTOM_MATERIAL_CLASS: u32 = 6;
pub const MATERIAL_CLASS_CUSTOM: u32 = 0xFFFF;

/// GPU material data. 96 bytes.
///
/// All texture indices reference the global material texture table. Depending
/// on device capabilities, that table is either descriptor-indexed or expanded
/// into ordinary texture/sampler bindings.
/// If a texture is not present, the index is u32::MAX.
///
/// # WGSL equivalent
/// ```wgsl
/// struct GpuMaterial {
///     base_color:           vec4<f32>,
///     emissive:             vec4<f32>,
///     roughness_metallic:   vec4<f32>,   // x=roughness, y=metallic, z=ior, w=specular
///     tex_base_color:       u32,
///     tex_normal:           u32,
///     tex_roughness:        u32,
///     tex_emissive:         u32,
///     tex_occlusion:        u32,
///     workflow:             u32,
///     flags:                u32,
///     material_class:       u32,
///     class_params:         vec4<f32>,
/// }
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuMaterial {
    /// Base color (RGBA linear)
    pub base_color: [f32; 4],
    /// Emissive color (RGB) + unused (w)
    pub emissive: [f32; 4],
    /// x=roughness, y=metallic/specular_strength, z=IOR, w=specular_tint
    pub roughness_metallic: [f32; 4],
    /// Bindless texture indices
    pub tex_base_color: u32,
    pub tex_normal: u32,
    pub tex_roughness: u32,
    pub tex_emissive: u32,
    pub tex_occlusion: u32,
    /// MaterialWorkflow discriminant
    pub workflow: u32,
    /// Feature flags (see `FLAG_*` constants)
    pub flags: u32,
    /// Material class selector (see `MATERIAL_CLASS_*` constants)
    pub material_class: u32,
    /// Class-specific parameters interpreted by the active Radiant template.
    /// The default PBR template ignores these; custom templates can use them
    /// for any purpose (e.g. clear-coat strength, subsurface colour, anisotropy direction).
    pub class_params: [f32; 4],
}

impl GpuMaterial {
    /// Index used to indicate "no texture bound"
    pub const NO_TEXTURE: u32 = u32::MAX;
}

/// Borrowed material-texture state for passes that sample Helio's texture table.
/// The G-buffer's single-row fallback reserves metadata `params.w = -1` to
/// select texture-store IDs directly from `GpuMaterial`, with identity UVs.
/// Explicit metadata tables must not use that reserved marker.
#[derive(Clone, Copy)]
pub struct MaterialTextureBindings<'a> {
    pub material_textures: &'a wgpu::Buffer,
    pub texture_views: &'a [&'a wgpu::TextureView],
    pub samplers: &'a [&'a wgpu::Sampler],
    pub version: u64,
}

#[cfg(test)]
mod material_binding_tests {
    use super::{
        MaterialBindingConfig, MaterialBindingMode, BINDLESS_MATERIAL_FEATURES,
        EXPANDED_MATERIAL_TEXTURE_RESERVE, MAX_MATERIAL_TEXTURES,
    };

    fn limits(sampled_textures: u32, samplers: u32, bindings: u32) -> wgpu::Limits {
        wgpu::Limits {
            max_sampled_textures_per_shader_stage: sampled_textures,
            max_samplers_per_shader_stage: samplers,
            max_bindings_per_bind_group: bindings,
            ..wgpu::Limits::default()
        }
    }

    #[test]
    fn missing_or_partial_binding_array_features_select_expanded_bindings() {
        for features in [
            wgpu::Features::empty(),
            wgpu::Features::TEXTURE_BINDING_ARRAY,
            wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING,
        ] {
            let config = MaterialBindingConfig::from_capabilities(features, limits(16, 16, 64));
            assert_eq!(config.mode, MaterialBindingMode::Expanded);
            assert_eq!(
                config.max_textures,
                (16 - EXPANDED_MATERIAL_TEXTURE_RESERVE).min(MAX_MATERIAL_TEXTURES)
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn complete_binding_array_feature_pair_selects_descriptor_indexing() {
        let config = MaterialBindingConfig::from_capabilities(
            BINDLESS_MATERIAL_FEATURES,
            limits(256, 256, 8),
        );
        assert_eq!(config.mode, MaterialBindingMode::BindingArray);
        assert_eq!(config.max_textures, MAX_MATERIAL_TEXTURES.min(256));
    }

    #[test]
    fn expanded_binding_count_is_bounded_by_texture_sampler_pairs() {
        let config =
            MaterialBindingConfig::from_capabilities(wgpu::Features::empty(), limits(64, 64, 22));
        assert_eq!(config.mode, MaterialBindingMode::Expanded);
        assert_eq!(config.max_textures, 10);

        let mut entries = Vec::new();
        config.append_layout_entries(&mut entries, 2, wgpu::ShaderStages::FRAGMENT);
        assert_eq!(entries.len(), 20);
        assert_eq!(entries.first().map(|entry| entry.binding), Some(2));
        assert_eq!(entries.last().map(|entry| entry.binding), Some(21));
        assert!(entries.iter().all(|entry| entry.count.is_none()));
    }
}
