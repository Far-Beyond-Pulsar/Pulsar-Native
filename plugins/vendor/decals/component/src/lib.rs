//! Decals (`pulsar.decals`): the component crate of the plugin that owns
//! decals.
//!
//! - The component ([`component`]): `DecalComponent`, an image or a tint
//!   projected onto the surfaces inside a box at its owner, and its script
//!   method `fade_by`.
//! - Its row ([`DecalSourceRow`]): the decal pass row SceneDB derives from
//!   the authored value, with the albedo image's texture store slot; the
//!   plugin's join builds its world-to-decal transform from the owner
//!   (`helio_pass_decal::scene_rows`).
//! - The registrations of the plugin's Helio feature
//!   (`helio_pass_decal::DecalFeature`, a G-buffer drawer), its scene rows
//!   and how a decal takes in a loaded image, with the engine.
//!
//! Everything registers through `inventory` once the crate is linked: hosts link it
//! without naming it (a game built for a project links the runtime modules
//! of the plugins its `Pulsar.toml` enables, the editor every installed
//! plugin through `installed_plugins`; `pulsar_plugins`) and know no decal type.

// The plugin's descriptor (`../pulsar-plugin.toml`): linking this crate
// installs the plugin; a project enables it or not (`pulsar_plugins`).
pulsar_plugins::installed!("../pulsar-plugin.toml");

pub mod component;

pub use component::*;

use pulsar_scenedb::gpu::{GpuMirrorHandle, SceneGpuStore};
use pulsar_scenedb_derive::SceneStore;

/// Where the decals' source rows live.
pub const DECAL_SOURCES_BUFFER: &str = "decal_sources";

/// A decal: its box's local size (`xyz`, full extent before the owner's
/// scale), then the decal pass row with a zero transform, which the join
/// replaces with the world-to-decal transform of the owner-placed box. A
/// disabled decal has a zero size, which the join skips.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "decal_sources")]
pub struct DecalSourceRow {
    #[gpu]
    pub size: [f32; 4],
    /// The pass row's transform: zero here, the join's to write.
    #[gpu]
    pub transform: [f32; 16],
    /// The rest of the pass row: colour and opacity, texture slots, blend,
    /// type, fade, age and normal adaptation.
    #[gpu]
    pub params: [f32; 16],
}

impl DecalSourceRow {
    /// The row of `decal` with its albedo texture at `albedo_slot`
    /// (`u32::MAX`: none).
    pub fn of(decal: &DecalComponent, albedo_slot: u32) -> Self {
        if !decal.enabled {
            return bytemuck::Zeroable::zeroed();
        }
        let size = [decal.size[0], decal.size[1], decal.size[2], 0.0];
        bytemuck::pod_read_unaligned(
            &[
                bytemuck::bytes_of(&size),
                bytemuck::bytes_of(&decal.to_row(albedo_slot)),
            ]
            .concat(),
        )
    }
}

// A decal's image is registered in the scene's texture store as its row is
// derived, so the row carries its bindless slot. The world loaded the image
// (a texture asset) and bound it into the decal; nothing reads the file here.
fn decal_dispatch(mirror: &GpuMirrorHandle, row: u32, data: *const (), is_new_insert: bool) {
    // SAFETY: SceneDB reaches this only through `DecalComponent`'s own
    // `ComponentId`, with a pointer to a live value.
    let decal = unsafe { &*(data as *const DecalComponent) };
    let albedo_slot = helio_component::texture_assets::texture_slot(
        &decal.albedo_image,
        &decal.albedo_texture,
        mirror,
        "decal",
    );
    pulsar_scenedb::gpu::write_derived_row(
        mirror,
        row,
        &DecalSourceRow::of(decal, albedo_slot),
        is_new_insert,
    );
}

fn decal_clear(mirror: &GpuMirrorHandle, row: u32) {
    pulsar_scenedb::gpu::clear_derived_row::<DecalSourceRow>(mirror, row);
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuMirrorRegistration {
        component_id: pulsar_scenedb::component_id::<DecalComponent>,
        dispatch: decal_dispatch,
    }
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuClearRegistration {
        component_id: pulsar_scenedb::component_id::<DecalComponent>,
        clear: decal_clear,
    }
}

fn register_texture_binding() {
    pulsar_scenedb::register_binding::<DecalComponent>(
        helio_component::texture_assets::TEXTURE_ASSET_KIND,
        bind_decal_texture,
    );
}

inventory::submit! {
    helio_component::texture_assets::TextureBindingRegistration {
        origin: pulsar_plugins::origin!(),
        register: register_texture_binding,
    }
}

fn register_columns(store: &mut SceneGpuStore, device: &std::sync::Arc<wgpu::Device>) {
    DecalSourceRow::register_gpu_columns_growable(store, 64, device);
}

fn derivation(
    device: &wgpu::Device,
    keys: helio_scene_join::RowJoinKeys,
) -> Box<dyn helio_scene_join::SceneDerivation> {
    Box::new(helio_pass_decal::scene_rows::scene_rows(
        device,
        keys,
        helio_scene_join::BufferKey::of(DECAL_SOURCES_BUFFER),
    ))
}

inventory::submit! {
    engine_backend::scene::helio_bridge::SceneRowsRegistration {
        origin: pulsar_plugins::origin!(),
        register_columns,
        derivation,
    }
}

/// The decals' feature: the decal drawer after the G-buffer's.
fn render_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new().with(helio_pass_decal::DecalFeature)
}

inventory::submit! {
    engine_backend::subsystems::render::RenderFeatureRegistration {
        origin: pulsar_plugins::origin!(),
        features: render_features,
        configure: |config| config,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_source_row_is_its_size_and_the_pass_row() {
        assert_eq!(std::mem::size_of::<DecalSourceRow>(), 36 * 4);
        assert_eq!(
            std::mem::size_of::<DecalSourceRow>() as u64,
            helio_pass_decal::scene_rows::DECAL_SOURCE_ROW_BYTES
        );
        assert_eq!(std::mem::size_of::<helio_pass_decal::DecalRow>(), 32 * 4);
    }

    #[test]
    fn a_disabled_decal_derives_an_inert_row() {
        let mut decal = DecalComponent::default();
        assert_eq!(
            DecalSourceRow::of(&decal, u32::MAX).size,
            [1.0, 1.0, 1.0, 0.0]
        );
        decal.enabled = false;
        let row = DecalSourceRow::of(&decal, u32::MAX);
        assert!(bytemuck::bytes_of(&row).iter().all(|byte| *byte == 0));
    }

    #[test]
    fn registers_its_feature() {
        let features =
            engine_backend::subsystems::render::helio_renderer::render_features::plugin_features();
        assert!(
            features.ids().contains(&"helio.decals"),
            "features: {:?}",
            features.ids()
        );
    }
}
