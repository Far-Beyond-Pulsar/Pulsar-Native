//! The voxel terrain joins Helio's presets as an engine's feature
//! (`Preset::with_features`): every preset with a G-buffer draws it, foliage
//! grows on its ground, TSR integrates its rotating sun samples, and the
//! forward preset (no G-buffer) leaves it out.
//!
//! Needs a device: the graphs build their passes for real. Without one the
//! test skips.
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use helio::{DebugDrawState, PassBuildContext, RendererConfig};
use helio_core::{GpuCameraUniforms, PassDeclaration};
use helio_pass_voxel_planet::chunks::pass::ChunkPass;
use helio_presets::Preset;
use pulsar_scenedb::gpu::{EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore};

fn device() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let adapter = instance.request_adapter(&Default::default()).await.ok()?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: helio::required_wgpu_features(adapter.features()),
                required_limits: helio::required_wgpu_limits(adapter.limits()),
                experimental_features: helio::required_experimental_features(adapter.features()),
                ..Default::default()
            })
            .await
            .ok()?;
        Some((Arc::new(device), Arc::new(queue)))
    })
}

fn mirror(device: &Arc<wgpu::Device>, queue: &Arc<wgpu::Queue>) -> GpuMirrorHandle {
    let gpu_context = EngineGpuContext::new(Arc::clone(device), Arc::clone(queue));
    let mut gpu_store = SceneGpuStore::new(
        &gpu_context,
        SceneGpuConfig {
            classes: Vec::new(),
            tombstone_headroom: 0,
            max_cells_metadata: 0,
        },
    );
    helio_components::AtmosphereComponent::register_gpu_columns_growable(&mut gpu_store, 4, device);
    helio_components::MeshComponent::register_gpu_columns_growable(&mut gpu_store, 16, device);
    helio_components::MaterialComponent::register_gpu_columns_growable(&mut gpu_store, 16, device);
    helio_components::StaticObjectComponent::register_gpu_columns_growable(
        &mut gpu_store,
        16,
        device,
    );
    helio_components::LightComponent::register_gpu_columns_growable(&mut gpu_store, 16, device);
    GpuMirrorHandle::new(Arc::new(gpu_store), Arc::clone(queue))
}

fn uniform_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::UNIFORM
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

struct Fixtures {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    camera: wgpu::Buffer,
    debug_camera: wgpu::Buffer,
    cull_stats: wgpu::Buffer,
    scene_db: GpuMirrorHandle,
    debug_state: Arc<Mutex<DebugDrawState>>,
    config: RendererConfig,
}

impl Fixtures {
    fn new() -> Option<Self> {
        let (device, queue) = device()?;
        Some(Self {
            camera: uniform_buffer(
                &device,
                "Scene Camera Buffer",
                (std::mem::size_of::<GpuCameraUniforms>() * 2) as u64,
            ),
            debug_camera: uniform_buffer(&device, "Debug Camera Buffer", 64),
            cull_stats: uniform_buffer(&device, "Cull Stats Buffer", 64),
            scene_db: mirror(&device, &queue),
            debug_state: Arc::new(Mutex::new(DebugDrawState::default())),
            config: RendererConfig::new(320, 180, wgpu::TextureFormat::Rgba8Unorm),
            device,
            queue,
        })
    }

    fn context(&self) -> PassBuildContext<'_> {
        PassBuildContext {
            device: &self.device,
            queue: &self.queue,
            config: self.config.clone(),
            debug_state: Arc::clone(&self.debug_state),
            camera_buffer: &self.camera,
            debug_camera_buffer: &self.debug_camera,
            cull_stats_buffer: &self.cull_stats,
            owns_device: false,
            scene_db: self.scene_db.clone(),
        }
    }

    /// `preset` with the voxel and media features (as the engine adds them), built.
    fn build(&self, preset: Preset) -> helio_core::RenderGraph {
        preset
            .with_features(
                &helio_feature_api::FeatureSet::new()
                    .with(helio_pass_voxel_planet::VoxelFeature)
                    .with(helio_pass_foliage_gbuffer::FoliageFeature)
                    .with(helio_pass_water_sim::WaterFeature)
                    .with(helio_pass_volumetric_fog::FogFeature)
                    .with(helio_pass_post_volumes::PostVolumesFeature),
            )
            .build(self.context())
    }
}

fn passes(graph: &helio_core::RenderGraph) -> Vec<PassDeclaration> {
    (0..graph.pass_count())
        .filter_map(|index| graph.pass_declaration(index))
        .collect()
}

#[test]
fn voxel_terrain_joins_every_preset_with_a_gbuffer() {
    let Some(mut fixtures) = Fixtures::new() else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: voxel terrain node");
        return;
    };
    fixtures.config = fixtures
        .config
        .clone()
        .with_tsr_quality(helio::TsrQuality::Quality)
        .with_feature(helio_pass_voxel_planet::VoxelSettings::default());
    for preset in [Preset::Deferred, Preset::DeferredFxaa, Preset::Hlfs] {
        let graph = fixtures.build(preset);
        let chunks = graph
            .find_pass::<ChunkPass>()
            .unwrap_or_else(|| panic!("{}: the terrain's pass", preset.name()));
        let place = graph
            .find_pass::<helio_pass_foliage_place::FoliagePlacePass>()
            .expect("foliage placement");
        assert!(
            Arc::ptr_eq(place.ground(), &chunks.foliage_ground()),
            "{}: foliage does not grow on the terrain's ground",
            preset.name()
        );
    }
    // Under TSR the terrain's rotating sun samples are integrated.
    let graph = fixtures.build(Preset::Deferred);
    let tsr = passes(&graph)
        .into_iter()
        .find(|pass| pass.type_name == "helio_pass_tsr::TsrPass")
        .expect("TSR");
    let reads: HashSet<&str> = tsr.reads.iter().map(|read| read.name).collect();
    assert!(reads.contains(helio_pass_tsr::STOCHASTIC_MASK));
    // The forward preset has no G-buffer to draw into.
    assert!(fixtures
        .build(Preset::Forward)
        .find_pass::<ChunkPass>()
        .is_none());
    // Without the voxel feature, no preset knows the terrain.
    assert!(Preset::Deferred
        .build(fixtures.context())
        .find_pass::<ChunkPass>()
        .is_none());
}
