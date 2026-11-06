//! Fog (`pulsar.fog`): the component crate of the plugin that owns
//! participating media.
//!
//! - The components ([`component`]): [`GlobalFogComponent`] (a medium
//!   filling the world) and [`LocalFogVolumeComponent`] (one confined to its
//!   owner's box), with the medium they share ([`MediumProps`]).
//! - Their rows ([`GlobalFogSourceRow`], [`LocalFogSourceRow`]): the media
//!   rows SceneDB derives from the authored values; the plugin's row join
//!   places a local volume at its owner (`helio_pass_volumetric_fog::scene_rows`).
//! - The registrations of the plugin's Helio feature
//!   (`helio_pass_volumetric_fog::FogFeature`: the volumetric medium, the
//!   frame's participating media, and its composite) and its scene rows
//!   with the engine.
//!
//! Everything registers through `inventory` once the crate is linked: hosts link it
//! without naming it (a game built for a project links the runtime modules
//! of the plugins its `Pulsar.toml` enables, the editor every installed
//! plugin through `installed_plugins`; `pulsar_plugins`) and know no fog type.

// The plugin's descriptor (`../pulsar-plugin.toml`): linking this crate
// installs the plugin; a project enables it or not (`pulsar_plugins`).
pulsar_plugins::installed!("../pulsar-plugin.toml");

pub mod component;
pub mod ground;
pub use ground::{haze_for_air, haze_on_ground};

pub use component::*;

use pulsar_scenedb::gpu::{GpuMirrorHandle, SceneGpuStore};
use pulsar_scenedb_derive::SceneStore;

/// Where the global media's source rows live.
pub const GLOBAL_FOG_SOURCES_BUFFER: &str = "global_fog_sources";
/// Where the local volumes' source rows live.
pub const LOCAL_FOG_SOURCES_BUFFER: &str = "local_fog_sources";

/// A global fog medium (`GlobalFogRow`, bit for bit; `enabled` 0 when the
/// authored component is disabled).
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "global_fog_sources")]
pub struct GlobalFogSourceRow {
    #[gpu]
    pub medium: [f32; 16],
    /// The medium's `planet` (its last four words).
    #[gpu]
    pub planet: [f32; 4],
}

/// A local fog volume: its local size (`xyz`, full extent before the owner's
/// scale), then the pass row's medium and edge fade (`x`).
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "local_fog_sources")]
pub struct LocalFogSourceRow {
    #[gpu]
    pub size: [f32; 4],
    #[gpu]
    pub medium: [f32; 16],
    /// The medium's `planet` (its last four words).
    #[gpu]
    pub planet: [f32; 4],
    #[gpu]
    pub edge_fade: [f32; 4],
}

/// `bytes`, which must be exactly a `T`, as a `T`.
fn read<T: bytemuck::Pod>(bytes: &[u8]) -> T {
    bytemuck::pod_read_unaligned(bytes)
}

impl GlobalFogSourceRow {
    pub fn of(fog: &GlobalFogComponent) -> Self {
        let mut medium = fog.medium.to_medium();
        medium.enabled = u32::from(fog.enabled);
        read(bytemuck::bytes_of(&medium))
    }
}

impl LocalFogSourceRow {
    pub fn of(fog: &LocalFogVolumeComponent) -> Self {
        let mut medium = fog.medium.to_medium();
        medium.enabled = u32::from(fog.enabled);
        let words: [f32; 20] = bytemuck::cast(medium);
        Self {
            size: [fog.size[0], fog.size[1], fog.size[2], 0.0],
            medium: read(bytemuck::cast_slice(&words[..16])),
            planet: read(bytemuck::cast_slice(&words[16..])),
            edge_fade: [fog.edge_fade.max(0.0), 0.0, 0.0, 0.0],
        }
    }
}

macro_rules! derived_row {
    ($authored:ty, $row:ty, $dispatch:ident, $clear:ident) => {
        fn $dispatch(mirror: &GpuMirrorHandle, row: u32, data: *const (), is_new_insert: bool) {
            // SAFETY: SceneDB reaches this only through the authored
            // component's own `ComponentId`, with a pointer to a live value.
            let authored = unsafe { &*(data as *const $authored) };
            pulsar_scenedb::gpu::write_derived_row(
                mirror,
                row,
                &<$row>::of(authored),
                is_new_insert,
            );
        }

        fn $clear(mirror: &GpuMirrorHandle, row: u32) {
            pulsar_scenedb::gpu::clear_derived_row::<$row>(mirror, row);
        }

        pulsar_scenedb::pulsar_reflection::inventory::submit! {
            pulsar_scenedb::gpu::GpuMirrorRegistration {
                component_id: pulsar_scenedb::component_id::<$authored>,
                dispatch: $dispatch,
            }
        }

        pulsar_scenedb::pulsar_reflection::inventory::submit! {
            pulsar_scenedb::gpu::GpuClearRegistration {
                component_id: pulsar_scenedb::component_id::<$authored>,
                clear: $clear,
            }
        }
    };
}

derived_row!(
    GlobalFogComponent,
    GlobalFogSourceRow,
    global_fog_dispatch,
    global_fog_clear
);
derived_row!(
    LocalFogVolumeComponent,
    LocalFogSourceRow,
    local_fog_dispatch,
    local_fog_clear
);

fn register_columns(store: &mut SceneGpuStore, device: &std::sync::Arc<wgpu::Device>) {
    GlobalFogSourceRow::register_gpu_columns_growable(store, 4, device);
    LocalFogSourceRow::register_gpu_columns_growable(store, 16, device);
}

fn derivation(
    device: &wgpu::Device,
    keys: helio_scene_join::RowJoinKeys,
) -> Box<dyn helio_scene_join::SceneDerivation> {
    Box::new(helio_pass_volumetric_fog::scene_rows::scene_rows(
        device,
        keys,
        helio_pass_volumetric_fog::FogRowSources {
            global_fog: helio_scene_join::BufferKey::of(GLOBAL_FOG_SOURCES_BUFFER),
            local_fog: helio_scene_join::BufferKey::of(LOCAL_FOG_SOURCES_BUFFER),
        },
    ))
}

inventory::submit! {
    engine_backend::scene::helio_bridge::SceneRowsRegistration {
        origin: pulsar_plugins::origin!(),
        register_columns,
        derivation,
    }
}

/// Fog's feature: the volumetric medium and its composite.
fn render_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new().with(helio_pass_volumetric_fog::FogFeature)
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
    fn rows_have_the_documented_word_counts() {
        assert_eq!(std::mem::size_of::<GlobalFogSourceRow>(), 20 * 4);
        assert_eq!(std::mem::size_of::<LocalFogSourceRow>(), 28 * 4);
        assert_eq!(
            std::mem::size_of::<helio_pass_volumetric_fog::GlobalFogRow>(),
            20 * 4
        );
        assert_eq!(
            std::mem::size_of::<helio_pass_volumetric_fog::LocalFogVolumeRow>(),
            32 * 4
        );
        assert_eq!(
            std::mem::size_of::<GlobalFogSourceRow>() as u64,
            helio_pass_volumetric_fog::scene_rows::GLOBAL_FOG_SOURCE_ROW_BYTES
        );
        assert_eq!(
            std::mem::size_of::<LocalFogSourceRow>() as u64,
            helio_pass_volumetric_fog::scene_rows::LOCAL_FOG_SOURCE_ROW_BYTES
        );
    }

    #[test]
    fn a_disabled_medium_derives_an_inert_row() {
        let enabled = |row: &GlobalFogSourceRow| row.medium[0].to_bits();
        let mut fog = GlobalFogComponent::default();
        assert_eq!(enabled(&GlobalFogSourceRow::of(&fog)), 1);
        fog.enabled = false;
        assert_eq!(enabled(&GlobalFogSourceRow::of(&fog)), 0);
    }

    #[test]
    fn registers_its_feature() {
        let features =
            engine_backend::subsystems::render::helio_renderer::render_features::plugin_features();
        assert!(
            features.ids().contains(&"helio.fog"),
            "features: {:?}",
            features.ids()
        );
    }
}
