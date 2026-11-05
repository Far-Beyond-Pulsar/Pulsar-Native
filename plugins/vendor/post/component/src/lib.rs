//! Post-process volumes (`pulsar.post`): the component crate of the plugin
//! that owns authored post-process settings.
//!
//! - The components: [`PostProcessVolumeComponent`] (settings that blend in
//!   inside a box, or everywhere when unbound) and
//!   [`CameraPostProcessComponent`] (one render view's baseline), with the
//!   settings they share ([`PostProcessSettingsProps`]).
//! - Their rows ([`PostProcessVolumeSourceRow`],
//!   [`CameraPostProcessSourceRow`]): the pass rows SceneDB derives from the
//!   authored values; the plugin's row join places a volume at its owner
//!   (`helio_pass_post_volumes::scene_rows`).
//! - The registrations of the plugin's Helio feature
//!   (`helio_pass_post_volumes::PostVolumesFeature`: the blend that
//!   publishes the frame's post settings) and its scene rows with the engine.
//!
//! Everything registers through `inventory` once the crate is linked: hosts link it
//! without naming it (a game built for a project links the runtime modules
//! of the plugins its `Pulsar.toml` enables, the editor every installed
//! plugin through `installed_plugins`; `pulsar_plugins`) and know no post-process type.

// The plugin's descriptor (`../pulsar-plugin.toml`): linking this crate
// installs the plugin; a project enables it or not (`pulsar_plugins`).
pulsar_plugins::installed!("../pulsar-plugin.toml");

pub mod camera;
pub mod volume;

pub use camera::*;
pub use volume::*;

use pulsar_scenedb::gpu::{GpuMirrorHandle, SceneGpuStore};
use pulsar_scenedb_derive::SceneStore;

/// Where the volumes' source rows live.
pub const POST_PROCESS_VOLUME_SOURCES_BUFFER: &str = "post_process_volume_sources";
/// Where the camera baselines' source rows live.
pub const CAMERA_POST_PROCESS_SOURCES_BUFFER: &str = "camera_postprocess_sources";

/// A post-process volume: its local size, then the pass row after its
/// bounds: priority, blend radius, blend weight, unbound flag (`params`, as
/// bits), override mask, settings and lens blocks.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "post_process_volume_sources")]
pub struct PostProcessVolumeSourceRow {
    #[gpu]
    pub size: [f32; 4],
    #[gpu]
    pub params: [u32; 4],
    #[gpu]
    pub mask: [u32; 4],
    #[gpu]
    pub settings: [u32; 116],
    #[gpu]
    pub lens: [f32; 16],
}

/// A camera's post-process baseline (`CameraPostProcessRow`, bit for bit:
/// view id and enabled flag in `header`; `enabled` 0 when the authored
/// component is disabled).
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "camera_postprocess_sources")]
pub struct CameraPostProcessSourceRow {
    #[gpu]
    pub header: [u32; 4],
    #[gpu]
    pub settings: [u32; 116],
    #[gpu]
    pub lens: [f32; 16],
}

/// `bytes`, which must be exactly a `T`, as a `T`.
fn read<T: bytemuck::Pod>(bytes: &[u8]) -> T {
    bytemuck::pod_read_unaligned(bytes)
}

impl PostProcessVolumeSourceRow {
    pub fn of(volume: &PostProcessVolumeComponent) -> Self {
        let size = [volume.size[0], volume.size[1], volume.size[2], 0.0];
        let mut row: Self = bytemuck::Zeroable::zeroed();
        if volume.enabled {
            let pass = helio_pass_post_volumes::PostProcessVolumeRow::from(
                volume.local_descriptor().to_gpu(),
            );
            // The pass row after its two bound vec4s.
            let after_bounds = &bytemuck::bytes_of(&pass)[32..];
            row = read(&[bytemuck::bytes_of(&size), after_bounds].concat());
        }
        row.size = size;
        row
    }
}

impl CameraPostProcessSourceRow {
    pub fn of(camera: &CameraPostProcessComponent) -> Self {
        let mut row = helio_pass_post_volumes::CameraPostProcessRow::new(
            camera.view_id,
            &camera.settings.to_settings(),
        );
        row.enabled = u32::from(camera.enabled);
        read(bytemuck::bytes_of(&row))
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
    PostProcessVolumeComponent,
    PostProcessVolumeSourceRow,
    volume_dispatch,
    volume_clear
);
derived_row!(
    CameraPostProcessComponent,
    CameraPostProcessSourceRow,
    camera_dispatch,
    camera_clear
);

fn register_columns(store: &mut SceneGpuStore, device: &std::sync::Arc<wgpu::Device>) {
    PostProcessVolumeSourceRow::register_gpu_columns_growable(store, 16, device);
    CameraPostProcessSourceRow::register_gpu_columns_growable(store, 4, device);
}

fn derivation(
    device: &wgpu::Device,
    keys: helio_scene_join::RowJoinKeys,
) -> Box<dyn helio_scene_join::SceneDerivation> {
    Box::new(helio_pass_post_volumes::scene_rows::scene_rows(
        device,
        keys,
        helio_pass_post_volumes::PostRowSources {
            post_process_volumes: helio_scene_join::BufferKey::of(
                POST_PROCESS_VOLUME_SOURCES_BUFFER,
            ),
            camera_post_process: helio_scene_join::BufferKey::of(
                CAMERA_POST_PROCESS_SOURCES_BUFFER,
            ),
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

/// The volumes' feature: the blend that resolves the frame's post settings.
fn render_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new().with(helio_pass_post_volumes::PostVolumesFeature)
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
        assert_eq!(std::mem::size_of::<PostProcessVolumeSourceRow>(), 144 * 4);
        assert_eq!(std::mem::size_of::<CameraPostProcessSourceRow>(), 136 * 4);
        assert_eq!(
            std::mem::size_of::<helio_pass_post_volumes::PostProcessVolumeRow>(),
            148 * 4
        );
        assert_eq!(
            std::mem::size_of::<helio_pass_post_volumes::CameraPostProcessRow>(),
            136 * 4
        );
        assert_eq!(
            std::mem::size_of::<PostProcessVolumeSourceRow>() as u64,
            helio_pass_post_volumes::scene_rows::POST_PROCESS_VOLUME_SOURCE_ROW_BYTES
        );
        assert_eq!(
            std::mem::size_of::<CameraPostProcessSourceRow>() as u64,
            helio_pass_post_volumes::scene_rows::CAMERA_POST_PROCESS_SOURCE_ROW_BYTES
        );
    }

    #[test]
    fn disabled_components_derive_inert_rows() {
        let mut camera = CameraPostProcessComponent::default();
        camera.view_id = 7;
        let row = CameraPostProcessSourceRow::of(&camera);
        assert_eq!((row.header[0], row.header[1]), (7, 1));
        camera.enabled = false;
        assert_eq!(CameraPostProcessSourceRow::of(&camera).header[1], 0);

        let mut volume = PostProcessVolumeComponent::default();
        volume.enabled = false;
        let row = PostProcessVolumeSourceRow::of(&volume);
        assert!(bytemuck::bytes_of(&row)[16..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn a_post_process_volume_row_is_the_pass_row_after_its_bounds() {
        let volume = PostProcessVolumeComponent::default();
        let row = PostProcessVolumeSourceRow::of(&volume);
        let pass =
            helio_pass_post_volumes::PostProcessVolumeRow::from(volume.local_descriptor().to_gpu());
        assert_eq!(
            &bytemuck::bytes_of(&row)[16..],
            &bytemuck::bytes_of(&pass)[32..]
        );
        assert_eq!(row.size[..3], volume.size);
    }

    #[test]
    fn registers_its_feature() {
        let features =
            engine_backend::subsystems::render::helio_renderer::render_features::plugin_features();
        assert!(
            features.ids().contains(&"helio.post_volumes"),
            "features: {:?}",
            features.ids()
        );
    }
}
