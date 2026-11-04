//! PBR material mapping from SolidRS to Helio material assets.

use helio_mats::MaterialWorkflow;
use helio_mats::{GpuMaterial, TextureTransform};
use solid_rs::scene::{AlphaMode, Material as SolidMaterial, TextureRef as SolidTextureRef};

use crate::texture_loader::TextureSemantic;
use crate::Result;

const MATERIAL_WORKFLOW_EPSILON: f32 = 1.0e-6;

#[derive(Debug, Clone, Copy)]
pub struct ConvertedTextureRef {
    pub texture_index: usize,
    pub uv_channel: u32,
    pub transform: TextureTransform,
}

#[derive(Debug, Clone, Default)]
pub struct ConvertedMaterialTextures {
    pub base_color: Option<ConvertedTextureRef>,
    pub normal: Option<ConvertedTextureRef>,
    pub roughness_metallic: Option<ConvertedTextureRef>,
    pub emissive: Option<ConvertedTextureRef>,
    pub occlusion: Option<ConvertedTextureRef>,
    pub specular_color: Option<ConvertedTextureRef>,
    pub specular_weight: Option<ConvertedTextureRef>,
    pub normal_scale: f32,
    pub occlusion_strength: f32,
    pub alpha_cutoff: f32,
}

#[derive(Debug, Clone)]
pub struct ConvertedMaterial {
    pub gpu: GpuMaterial,
    pub textures: ConvertedMaterialTextures,
}

fn uses_explicit_specular_ior_workflow(material: &SolidMaterial) -> bool {
    let specular_color_is_default = (material.specular_color.x - 1.0).abs()
        <= MATERIAL_WORKFLOW_EPSILON
        && (material.specular_color.y - 1.0).abs() <= MATERIAL_WORKFLOW_EPSILON
        && (material.specular_color.z - 1.0).abs() <= MATERIAL_WORKFLOW_EPSILON;
    let specular_weight_is_default =
        (material.specular_weight - 1.0).abs() <= MATERIAL_WORKFLOW_EPSILON;
    let ior_is_default = (material.ior - 1.5).abs() <= MATERIAL_WORKFLOW_EPSILON;

    !specular_color_is_default
        || material.specular_color_texture.is_some()
        || !specular_weight_is_default
        || material.specular_weight_texture.is_some()
        || !ior_is_default
}

fn convert_texture_ref(
    texture: &SolidTextureRef,
    semantic: TextureSemantic,
) -> ConvertedTextureRef {
    if texture.uv_channel > 1 {
        log::warn!(
            "Texture semantic {:?} uses UV channel {}, but the current Helio wrapper only carries UV0/UV1; falling back to UV1.",
            semantic,
            texture.uv_channel
        );
    }

    let transform =
        texture
            .transform
            .as_ref()
            .map_or_else(TextureTransform::default, |transform| TextureTransform {
                offset: [transform.offset.x, transform.offset.y],
                scale: [transform.scale.x, transform.scale.y],
                rotation_radians: transform.rotation,
            });

    ConvertedTextureRef {
        texture_index: texture.texture_index,
        uv_channel: texture.uv_channel.min(1) as u32,
        transform,
    }
}

pub fn convert_material<F>(
    material: &SolidMaterial,
    mut resolve_texture: F,
) -> Result<ConvertedMaterial>
where
    F: FnMut(ConvertedTextureRef, TextureSemantic) -> Result<ConvertedTextureRef>,
{
    let mut workflow = MaterialWorkflow::Metallic as u32;
    let mut metallic = material.metallic_factor;
    let mut roughness = material.roughness_factor;
    let mut ior = 1.5;
    let mut specular = 0.5;

    if uses_explicit_specular_ior_workflow(material) {
        workflow = MaterialWorkflow::Specular as u32;
        metallic = material.specular_weight;
        roughness = material.roughness_factor;
        ior = material.ior;
        specular = material.specular_weight;
    }

    let mut flags = 0u32;
    match material.alpha_mode {
        AlphaMode::Opaque => {}
        AlphaMode::Mask => {
            flags |= 1 << 2;
        }
        AlphaMode::Blend => {
            flags |= 1 << 1;
        }
    }
    if material.double_sided {
        flags |= 1;
    }

    let convert_slot = |slot: &Option<SolidTextureRef>,
                        semantic: TextureSemantic,
                        resolve_texture: &mut F|
     -> Result<Option<ConvertedTextureRef>> {
        slot.as_ref()
            .map(|texture| resolve_texture(convert_texture_ref(texture, semantic), semantic))
            .transpose()
    };

    let textures = ConvertedMaterialTextures {
        base_color: convert_slot(
            &material.base_color_texture,
            TextureSemantic::BaseColor,
            &mut resolve_texture,
        )?,
        normal: convert_slot(
            &material.normal_texture,
            TextureSemantic::Normal,
            &mut resolve_texture,
        )?,
        roughness_metallic: convert_slot(
            &material.metallic_roughness_texture,
            TextureSemantic::MetallicRoughness,
            &mut resolve_texture,
        )?,
        emissive: convert_slot(
            &material.emissive_texture,
            TextureSemantic::Emissive,
            &mut resolve_texture,
        )?,
        occlusion: convert_slot(
            &material.occlusion_texture,
            TextureSemantic::Occlusion,
            &mut resolve_texture,
        )?,
        specular_color: convert_slot(
            &material.specular_color_texture,
            TextureSemantic::SpecularColor,
            &mut resolve_texture,
        )?,
        specular_weight: convert_slot(
            &material.specular_weight_texture,
            TextureSemantic::SpecularWeight,
            &mut resolve_texture,
        )?,
        normal_scale: material.normal_scale,
        occlusion_strength: material.occlusion_strength,
        alpha_cutoff: material.alpha_cutoff,
    };

    Ok(ConvertedMaterial {
        gpu: GpuMaterial {
            base_color: [
                material.base_color_factor.x,
                material.base_color_factor.y,
                material.base_color_factor.z,
                material.base_color_factor.w,
            ],
            emissive: [
                material.emissive_factor.x,
                material.emissive_factor.y,
                material.emissive_factor.z,
                1.0,
            ],
            roughness_metallic: [roughness, metallic, ior, specular],
            tex_base_color: GpuMaterial::NO_TEXTURE,
            tex_normal: GpuMaterial::NO_TEXTURE,
            tex_roughness: GpuMaterial::NO_TEXTURE,
            tex_emissive: GpuMaterial::NO_TEXTURE,
            tex_occlusion: GpuMaterial::NO_TEXTURE,
            workflow,
            flags,
            material_class: 0,
            class_params: [0.0; 4],
        },
        textures,
    })
}
