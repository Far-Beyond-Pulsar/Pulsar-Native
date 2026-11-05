//! The materials' public contract (`pulsar.materials`): the one crate
//! another kind reads a material concept from.
//!
//! The GPU material row and its table ([`MaterialRow`], [`MATERIALS`]), the
//! material texture table ([`MATERIAL_TEXTURES`], [`MaterialTextureBindings`],
//! the binding mode a device gets: [`MaterialBindingConfig`]) and its sampler
//! setting ([`MaterialSampler`]), the texture uploads and UV transforms
//! ([`texture`]), the PBR/BRDF evaluation and specular anti-aliasing WGSL,
//! and the Radiant material-template system ([`radiant`], [`templates`],
//! [`MaterialTemplates`]). The plugin's feature (`helio-pass-materials`)
//! publishes the texture table; the drawers of every kind read it and the
//! rows through this crate.
//!
//! Holds no scene storage of its own: persistent material authoring is a
//! SceneDB component storing a [`MaterialRow`] under [`MATERIALS`] (for
//! standalone hosts, [`host_rows`] with the `host-rows` feature).

pub mod asset;
#[cfg(feature = "host-rows")]
pub mod host_rows;
pub mod material;
pub mod radiant;
pub mod row;
pub mod shader;
pub mod table;
pub mod templates;
pub mod texture;
pub mod texture_mips;
pub mod vg_alpha;

pub use material::{
    GpuMaterial, MaterialBindingConfig, MaterialBindingMode, MaterialTextureBindings,
    MaterialWorkflow, BINDLESS_MATERIAL_FEATURES, EXPANDED_MATERIAL_TEXTURE_RESERVE,
    FIRST_CUSTOM_MATERIAL_CLASS, FLAG_ALPHA_BLEND, FLAG_ALPHA_TEST, FLAG_DOUBLE_SIDED,
    FLAG_FORWARD_SHADING, FLAG_HAS_ANISOTROPY, FLAG_HAS_CLEAR_COAT, FLAG_HAS_CUSTOM_SHADER,
    FLAG_HAS_NORMAL_MAP, FLAG_HAS_SUBSURFACE, FLAG_TRANSPARENT_ONLY, MATERIAL_CLASS_ANISOTROPIC,
    MATERIAL_CLASS_CLEAR_COAT, MATERIAL_CLASS_CUSTOM, MATERIAL_CLASS_DEFAULT,
    MATERIAL_CLASS_FOLIAGE, MATERIAL_CLASS_SKIN, MATERIAL_CLASS_SUBSURFACE, MAX_MATERIAL_TEXTURES,
};
pub use radiant::{
    graph_source, register_graph_source, RadiantGraphRegistry, RadiantShaderCache,
    RadiantShaderKey, RadiantTemplate, RadiantTemplateRegistry, SharedTemplateRegistry,
};
pub use row::{MaterialRow, MATERIALS};
pub use shader::{
    apply_webgpu_decal_bindings, apply_webgpu_material_bindings, MATERIAL_ROW_SNIPPET,
    MATERIAL_SNIPPETS, MATERIAL_SURFACE_SNIPPET, PBR_EVAL, PBR_EVAL_SNIPPET, PBR_MARKER,
    SPECULAR_AA, SPECULAR_AA_MARKER, SPECULAR_AA_SNIPPET,
};
pub use table::{MaterialStandIn, MaterialTextureTable, MATERIAL_TEXTURE_DATA_WORDS};
pub use texture::{TextureSamplerDesc, TextureTransform, TextureUpload};

/// The material templates surfaces compile from (a renderer config's
/// `features`): the opaque registry the G-buffer drawer reads and the
/// transparent one. Shared handles: a host registers templates into them
/// whenever it likes (`radiant_demo`). Without the setting the passes use
/// the built-in templates.
#[derive(Clone)]
pub struct MaterialTemplates {
    pub opaque: SharedTemplateRegistry,
    pub transparent: SharedTemplateRegistry,
}

impl Default for MaterialTemplates {
    fn default() -> Self {
        let registry = || {
            std::sync::Arc::new(std::sync::RwLock::new(
                radiant::RadiantTemplateRegistry::new(),
            ))
        };
        Self {
            opaque: registry(),
            transparent: registry(),
        }
    }
}

/// The sampler material texture slots are read with (a renderer config's
/// `features`, read when the graph builds: set it with
/// `Renderer::set_feature`); the default sampler without.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialSampler(pub wgpu::SamplerDescriptor<'static>);

/// Graph key of the material texture bindings (`MaterialTextureBindings`)
/// published every frame for the passes that sample material textures.
pub const MATERIAL_TEXTURES: &str = "material_textures";

/// [`MATERIAL_TEXTURES`] as a typed key.
#[inline]
pub const fn material_textures<T>() -> helio_core::ResourceKey<T> {
    helio_core::ResourceKey::new(MATERIAL_TEXTURES)
}
