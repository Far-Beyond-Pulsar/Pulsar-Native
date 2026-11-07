use helio_light_host::LightComponent;
use std::sync::Arc;

use glam::Vec3;
use helio::{required_wgpu_limits, Camera, RendererBuilder, RendererConfig};
use helio_mats::{
    MaterialBindingConfig, MaterialBindingMode, BINDLESS_MATERIAL_FEATURES,
    EXPANDED_MATERIAL_TEXTURE_RESERVE, MAX_MATERIAL_TEXTURES,
};

const PORTABLE_SAMPLED_TEXTURE_LIMIT: u32 = 16;

#[test]
fn default_graph_renders_on_a_native_non_bindless_device_at_the_16_texture_boundary() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let Some(adapter) = request_test_adapter(&instance).await else {
            eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: limited native default graph");
            return;
        };
        if !adapter
            .features()
            .contains(wgpu::Features::INDIRECT_FIRST_INSTANCE)
        {
            eprintln!("GPU_VALIDATION_SKIPPED_MISSING_INDIRECT_FIRST_INSTANCE: limited native default graph");
            return;
        }

        let mut limits = required_wgpu_limits(adapter.limits());
        limits.max_sampled_textures_per_shader_stage = PORTABLE_SAMPLED_TEXTURE_LIMIT;
        limits.max_samplers_per_shader_stage = limits
            .max_samplers_per_shader_stage
            .min(PORTABLE_SAMPLED_TEXTURE_LIMIT);
        run_default_graph(
            adapter,
            wgpu::Features::INDIRECT_FIRST_INSTANCE,
            limits,
            MaterialBindingMode::Expanded,
            PORTABLE_SAMPLED_TEXTURE_LIMIT as usize - EXPANDED_MATERIAL_TEXTURE_RESERVE,
            "Limited Native",
        )
        .await;
    });
}

#[test]
fn default_graph_retains_the_bindless_material_tier_when_supported() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let Some(adapter) = request_test_adapter(&instance).await else {
            eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: bindless native default graph");
            return;
        };
        let required_features =
            wgpu::Features::INDIRECT_FIRST_INSTANCE | BINDLESS_MATERIAL_FEATURES;
        if !adapter.features().contains(required_features) {
            eprintln!("GPU_VALIDATION_SKIPPED_NO_BINDLESS_TIER: bindless native default graph");
            return;
        }
        let limits = required_wgpu_limits(adapter.limits());
        let expected_max = MAX_MATERIAL_TEXTURES
            .min(limits.max_sampled_textures_per_shader_stage as usize)
            .min(limits.max_samplers_per_shader_stage as usize);
        run_default_graph(
            adapter,
            required_features,
            limits,
            MaterialBindingMode::BindingArray,
            expected_max,
            "Bindless Native",
        )
        .await;
    });
}

#[test]
fn budgeted_shadows_render_hundreds_of_requests_across_frames() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let adapter = request_test_adapter(&instance)
            .await
            .expect("GPU required for shadow integration");
        let features = wgpu::Features::INDIRECT_FIRST_INSTANCE | BINDLESS_MATERIAL_FEATURES;
        let limits = required_wgpu_limits(adapter.limits());
        let expected = MAX_MATERIAL_TEXTURES
            .min(limits.max_sampled_textures_per_shader_stage as usize)
            .min(limits.max_samplers_per_shader_stage as usize);
        run_default_graph(
            adapter,
            features,
            limits,
            MaterialBindingMode::BindingArray,
            expected,
            "Budgeted shadows",
        )
        .await;
    });
}

async fn run_default_graph(
    adapter: wgpu::Adapter,
    required_features: wgpu::Features,
    required_limits: wgpu::Limits,
    expected_mode: MaterialBindingMode,
    expected_max_textures: usize,
    label: &'static str,
) {
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some(label),
            required_features,
            required_limits,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            ..Default::default()
        })
        .await
        .expect("the selected native material capability tier must create a device");
    let device = Arc::new(device);
    let queue = Arc::new(queue);
    let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    // The material binding tier is chosen from the device, as the renderer
    // does at construction.
    let binding = MaterialBindingConfig::for_device(&device);
    assert_eq!(binding.mode, expected_mode);
    assert_eq!(binding.max_textures, expected_max_textures);

    let mut scene_db = scene_db_with_gpu_mirror(&device, &queue);
    if label == "Budgeted shadows" {
        for i in 0..200 {
            let entity = scene_db.world.spawn();
            let light = helio_light_api::GpuLight {
                position_range: [(i % 20) as f32 * 0.2 - 2.0, 1.0, (i / 20) as f32 * 0.2, 3.0],
                direction_outer: [0.0, -1.0, 0.0, 0.7],
                shadow_index: 0,
                light_type: 2,
                ..Default::default()
            };
            scene_db.world.insert(
                entity,
                LightComponent::from(helio_light_api::LightRow::from(light)),
            );
        }
    }
    let config = RendererConfig::new(32, 32, wgpu::TextureFormat::Rgba8Unorm);
    let mut renderer = RendererBuilder::new(
        config.clone(),
        scene_db
            .world
            .gpu_mirror()
            .cloned()
            .expect("mirror attached above"),
    )
    .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
    .with_external_device()
    .build(
        Arc::clone(&device),
        Arc::clone(&queue),
        config.width,
        config.height,
        config.surface_format,
    );
    let construction_error = validation_scope.pop().await;
    assert!(
        construction_error.is_none(),
        "graph construction: {construction_error:?}"
    );
    let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    scene_db.world.flush_gpu_mirror(&queue);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Limited Native Render Target"),
        size: wgpu::Extent3d {
            width: config.width,
            height: config.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: config.surface_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    let camera = Camera::perspective_look_at(
        Vec3::new(0.0, 1.0, 3.0),
        Vec3::ZERO,
        Vec3::Y,
        60.0_f32.to_radians(),
        1.0,
        0.1,
        100.0,
    );

    renderer
        .render(&camera, &target_view)
        .expect("the complete selected-tier default graph must render");

    if label == "Budgeted shadows" {
        for _ in 0..16 {
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            renderer
                .render(&camera, &target_view)
                .expect("budgeted shadow frame");
            let (updates, texels) = renderer
                .find_pass::<helio_pass_shadow::ShadowPass>()
                .unwrap()
                .last_update_work();
            let budget = config
                .features
                .get_or_default::<helio_light_api::ShadowSettings>()
                .budget;
            assert!(updates <= budget.updates_per_frame);
            assert!(texels <= budget.update_texels_per_frame);
        }
    }

    if label == "Budgeted shadows" {
        let residency = renderer
            .find_pass::<helio_pass_shadow_matrix::ShadowMatrixPass>()
            .unwrap()
            .residency();
        assert!(
            residency
                .residents
                .iter()
                .filter(|r| r.owner != 0 && r.tiles.iter().any(|t| t.size > 0))
                .count()
                > 42
        );
    }
    let resized_target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Limited Native Resized Render Target"),
        size: wgpu::Extent3d {
            width: 48,
            height: 24,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: config.surface_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    renderer.set_render_size(48, 24);
    renderer
        .render(&camera, &resized_target.create_view(&Default::default()))
        .expect("the complete selected-tier default graph must render after resize");
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let validation_error = validation_scope.pop().await;
    assert!(
        validation_error.is_none(),
        "selected-tier default graph validation failed: {validation_error:?}"
    );
}

/// An empty SceneDB with a GPU mirror and the columns the default graph
/// reads, the way every frontend now hands scene data to the renderer.
fn scene_db_with_gpu_mirror(
    device: &Arc<wgpu::Device>,
    queue: &Arc<wgpu::Queue>,
) -> pulsar_scenedb::SceneDb {
    let mut scene_db = pulsar_scenedb::SceneDb::new();
    let ctx = pulsar_scenedb::gpu::EngineGpuContext::new(device.clone(), queue.clone());
    let mut store = pulsar_scenedb::gpu::SceneGpuStore::new(
        &ctx,
        pulsar_scenedb::gpu::SceneGpuConfig {
            classes: Vec::new(),
            tombstone_headroom: 0,
            max_cells_metadata: 0,
        },
    );
    helio_mesh_host::MeshComponent::register_gpu_columns_growable(&mut store, 64, device);
    helio_mats::host_rows::MaterialComponent::register_gpu_columns_growable(&mut store, 64, device);
    helio_mesh_host::StaticObjectComponent::register_gpu_columns_growable(&mut store, 64, device);
    helio_light_host::LightComponent::register_gpu_columns_growable(
        &mut store,
        helio_light_api::MAX_LIGHTS,
        device,
    );
    let mirror = pulsar_scenedb::gpu::GpuMirrorHandle::new(Arc::new(store), queue.clone());
    scene_db.world.attach_gpu_mirror(mirror);
    scene_db
}

async fn request_test_adapter(instance: &wgpu::Instance) -> Option<wgpu::Adapter> {
    for force_fallback_adapter in [false, true] {
        if let Ok(adapter) = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter,
                apply_limit_buckets: false,
            })
            .await
        {
            return Some(adapter);
        }
    }
    None
}
