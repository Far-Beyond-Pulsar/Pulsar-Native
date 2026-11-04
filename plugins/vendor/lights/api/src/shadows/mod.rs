//! The shadow views' contract: the per-face matrices the views publish
//! ([`SHADOW_MATRICES`], [`GpuShadowMatrix`]), the atlas budget and its
//! residency ([`ShadowSettings`], [`ShadowBudget`], [`ResidencyTable`]),
//! the filtering quality, and the per-caster dirty flags what casts into
//! the views reads ([`LightDirtyFlags`]). The views are the lights' shadow
//! views feature; the casters are any kind's drawers.

mod budget;
mod gpu_types;
pub use budget::*;
pub use gpu_types::*;

/// Graph key of the dynamic casters' shadow atlas (`Depth32Float`, a
/// `D2Array`): what casts into the views draws it, every reader samples it
/// through the views' rows (`//!use shadow_atlas`).
pub const SHADOW_ATLAS: &str = "shadow_atlas";
/// Graph key of the static casters' cached atlas (same form).
pub const STATIC_SHADOW_ATLAS: &str = "static_shadow_atlas";
/// Graph key of the casters' coloured transmittance (`Rgba16Float`
/// `D2Array`: rgb the absorption, a the depth of a translucent caster).
pub const SHADOW_TRANSMITTANCE: &str = "shadow_transmittance";
/// Graph key of the atlas's comparison sampler.
pub const SHADOW_SAMPLER: &str = "shadow_sampler";
/// Graph key of the casters' per-face counts.
pub const SHADOW_CASTER_COUNTS: &str = "shadow_caster_counts";

/// Graph key of the shadow views' matrices
/// ([`ShadowMatricesFrameData`]).
pub const SHADOW_MATRICES: &str = "shadow_matrices";

/// [`SHADOW_MATRICES`] as a typed key.
#[inline]
pub const fn shadow_matrices<T>() -> helio_core::ResourceKey<T> {
    helio_core::ResourceKey::new(SHADOW_MATRICES)
}

/// The shadow views' per-caster dirty flags: The shadow views set a
/// caster's flag when its light's matrices change, and whatever draws into
/// the shadow casters reads them to know which faces to redraw (the meshes'
/// caster invalidation, which also dirties faces its own moved casters
/// touch). One per graph: the node that builds the matrix pass shares it
/// (`BuildEnv::shared`), the casters' nodes take it from there.
pub struct LightDirtyFlags(pub std::sync::Arc<wgpu::Buffer>);

impl LightDirtyFlags {
    pub fn new(device: &wgpu::Device) -> Self {
        Self(std::sync::Arc::new(device.create_buffer(
            &wgpu::BufferDescriptor {
                label: Some("Shadow Dirty Flags"),
                size: MAX_SHADOW_CASTERS as u64 * 4,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        )))
    }
}
