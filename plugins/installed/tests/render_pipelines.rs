//! The shipped render pipelines as the editor and the game see them: the
//! `pulsar.render_pipelines` plugin's files in every project's
//! `plugins/pulsar.render_pipelines/` folder, registered by the engine before
//! a renderer asks, made for the project's plugins, and the migration of a
//! level that names Helio's old default (or none, from before pipelines were
//! explicit) to them.
//!
//! The GPU part builds `Deferred.rendergraph` the way a viewport does and
//! requires the passes the deferred preset builds with every plugin's
//! features: the sky, the voxel terrain and the lights are in the frame.

use std::sync::{Arc, Mutex};

use engine_backend::subsystems::render::helio_renderer::{pipelines, render_features};
use helio::{DebugDrawState, IntoGraph, PassBuildContext, RendererConfig};
use helio_core::GpuCameraUniforms;
use helio_presets::Preset;
use pulsar_scenedb::gpu::{EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore};

// Every installed plugin is linked, as the editor links them.
use installed_plugins as _;

const DEFERRED: &str = "plugins/pulsar.render_pipelines/Deferred.rendergraph";

/// The process's project and plugin set are global: one test at a time.
static PROJECT: Mutex<()> = Mutex::new(());

fn open_project(toml: &str) -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("Pulsar.toml"), toml).unwrap();
    engine_state::EngineContext::new().set_global();
    engine_state::set_project_path(project.path().display().to_string());
    project
}

fn node_kinds(text: &str) -> Vec<String> {
    let asset = helio_graph_asset::GraphAsset::from_json(text).expect("the file parses");
    asset
        .nodes
        .iter()
        .chain(
            asset
                .sections
                .iter()
                .flat_map(|section| section.nodes.iter()),
        )
        .map(|node| node.kind.clone())
        .collect()
}

#[test]
fn the_shipped_pipelines_are_the_plugins_files_and_old_levels_run_them() {
    let _one = PROJECT.lock().unwrap_or_else(|e| e.into_inner());
    let project = open_project("# a test project\n");

    // The folder is there, made for the installed plugins: the voxel
    // terrain's and the sky's nodes are in the deferred graph.
    let file = project.path().join(DEFERRED);
    let text = engine_fs::virtual_fs::read_to_string(&file).expect("the plugin serves the file");
    let kinds = node_kinds(&text);
    for kind in [
        "helio.pass.voxel_chunks",
        "helio.pass.atmosphere_composite",
        "helio.pass.deferred_light",
        helio_presets::features::ANTIALIASING,
    ] {
        assert!(kinds.iter().any(|k| k == kind), "{kind} is in the file");
    }

    // The engine registered it before anyone asked, by its project path.
    assert!(matches!(
        pipelines::pipeline_state(Some(DEFERRED)),
        pipelines::PipelineState::Assigned { .. }
    ));
    let listed: Vec<String> = pipelines::list_pipelines()
        .into_iter()
        .map(|pipeline| pipeline.id)
        .collect();
    for stem in ["Deferred", "DeferredFxaa", "Forward", "HLFS", "HLFS_FXAA"] {
        let id = format!("plugins/pulsar.render_pipelines/{stem}.rendergraph");
        assert!(listed.contains(&id), "{id} in {listed:?}");
    }

    // A level from before pipelines were explicit, and one that names the old
    // default by id, both run the file.
    for level in [
        r#"{"version":"2.1"}"#,
        r#"{"world_settings":{"pipeline_version":1,"render_pipeline":"helio.deferred"}}"#,
    ] {
        assert_eq!(
            pipelines::render_pipeline_of_level(level).as_deref(),
            Some(DEFERRED),
            "{level}"
        );
    }
    assert_eq!(
        pipelines::migrate_pipeline_id("helio.hlfs".into()),
        "plugins/pulsar.render_pipelines/HLFS.rendergraph"
    );
    // A project's own graph keeps its id.
    assert_eq!(
        pipelines::migrate_pipeline_id("graphs/mine.rendergraph".into()),
        "graphs/mine.rendergraph"
    );
}

#[test]
fn without_a_kind_the_shipped_graph_has_none_of_its_nodes_and_still_runs() {
    let _one = PROJECT.lock().unwrap_or_else(|e| e.into_inner());
    let project = open_project("[plugins]\n\"pulsar.voxel\" = false\n");
    let text = engine_fs::virtual_fs::read_to_string(&project.path().join(DEFERRED))
        .expect("the plugin serves the file");
    assert!(!node_kinds(&text)
        .iter()
        .any(|k| k == "helio.pass.voxel_chunks"));
    assert!(matches!(
        pipelines::pipeline_state(Some(DEFERRED)),
        pipelines::PipelineState::Assigned { .. }
    ));
    pulsar_plugins::deactivate();
}

#[test]
fn with_the_plugin_disabled_an_old_level_says_its_pipeline_is_missing() {
    let _one = PROJECT.lock().unwrap_or_else(|e| e.into_inner());
    let _project = open_project("[plugins]\n\"pulsar.render_pipelines\" = false\n");
    assert_eq!(
        pipelines::render_pipeline_of_level(r#"{"version":"2.1"}"#).as_deref(),
        Some(pipelines::LEGACY_DEFAULT_PIPELINE)
    );
    assert_eq!(
        pipelines::pipeline_state(Some(pipelines::LEGACY_DEFAULT_PIPELINE)),
        pipelines::PipelineState::NotFound(pipelines::LEGACY_DEFAULT_PIPELINE.into())
    );
    pulsar_plugins::deactivate();
}

fn device() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: helio::required_wgpu_features(adapter.features()),
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        experimental_features: helio::required_experimental_features(adapter.features()),
        ..Default::default()
    }))
    .ok()?;
    Some((Arc::new(device), Arc::new(queue)))
}

/// The pass names a pipeline builds, in order.
fn passes(
    pipeline: Box<dyn IntoGraph>,
    device: &Arc<wgpu::Device>,
    queue: &Arc<wgpu::Queue>,
) -> Vec<String> {
    let buffer = |label: &str, size: u64| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::UNIFORM
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    let camera = buffer(
        "Scene Camera Buffer",
        (std::mem::size_of::<GpuCameraUniforms>() * 2) as u64,
    );
    let debug_camera = buffer("Debug Camera Buffer", 64);
    let cull_stats = buffer("Cull Stats Buffer", 64);
    let store = SceneGpuStore::new(
        &EngineGpuContext::new(Arc::clone(device), Arc::clone(queue)),
        SceneGpuConfig {
            classes: Vec::new(),
            tombstone_headroom: 0,
            max_cells_metadata: 0,
        },
    );
    let graph = pipeline
        .into_graph(PassBuildContext {
            device,
            queue,
            config: render_features::configure(RendererConfig::new(
                256,
                256,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            )),
            debug_state: Arc::new(std::sync::Mutex::new(DebugDrawState::default())),
            camera_buffer: &camera,
            debug_camera_buffer: &debug_camera,
            cull_stats_buffer: &cull_stats,
            owns_device: false,
            scene_db: GpuMirrorHandle::new(Arc::new(store), Arc::clone(queue)),
        })
        .expect("the pipeline builds");
    graph
        .build_order_declarations()
        .into_iter()
        .map(|(pass, _)| pass.pass.to_string())
        .collect()
}

/// The shipped file builds exactly what the deferred preset builds with every
/// plugin's features (what `helio.deferred` was), the sky and the terrain
/// included.
#[test]
fn the_deferred_file_builds_the_deferred_preset_with_every_plugin() {
    let _one = PROJECT.lock().unwrap_or_else(|e| e.into_inner());
    let _project = open_project("# a test project\n");
    let Some((device, queue)) = device() else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: shipped deferred pipeline");
        return;
    };
    let from_file = passes(
        pipelines::create_pipeline(Some(DEFERRED)).expect("registered"),
        &device,
        &queue,
    );
    let from_preset = passes(
        Box::new(Preset::Deferred.with_features(&render_features::plugin_features())),
        &device,
        &queue,
    );
    assert_eq!(from_file, from_preset);
    for pass in ["AtmosphereComposite", "DeferredLight"] {
        assert!(
            from_file.iter().any(|p| p.contains(pass)),
            "{pass} in {from_file:?}"
        );
    }
}

/// Every backend this machine has an adapter for.
fn devices_by_backend() -> Vec<(wgpu::Backend, Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
    let mut found = Vec::new();
    for backends in [
        wgpu::Backends::VULKAN,
        wgpu::Backends::DX12,
        wgpu::Backends::METAL,
    ] {
        let mut descriptor = helio_core::wgpu_instance_descriptor();
        descriptor.backends = backends;
        let instance = wgpu::Instance::new(descriptor);
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
            continue;
        };
        let backend = adapter.get_info().backend;
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                required_features: helio::required_wgpu_features(adapter.features()),
                required_limits: helio::required_wgpu_limits(adapter.limits()),
                experimental_features: helio::required_experimental_features(adapter.features()),
                ..Default::default()
            }))
        else {
            continue;
        };
        found.push((backend, Arc::new(device), Arc::new(queue)));
    }
    found
}

/// The editor's viewport renderer builds and draws on every backend: first
/// with no pipeline (Helio's status screen, which draws its text with the
/// debug overlay), then the shipped deferred pipeline. The editor runs on
/// D3D12 on Windows, whose shader compiler (DXC) rejects what Vulkan's path
/// accepts: the status screen's text shader once ended a non-void function in
/// `discard`, the editor's first renderer build panicked and every viewport
/// stayed black (Pulsar-Native, 2026-11-11), while the Vulkan examples and the
/// headless tests drew.
#[test]
fn the_editor_renderer_builds_and_draws_on_every_backend() {
    let _one = PROJECT.lock().unwrap_or_else(|e| e.into_inner());
    let _project = open_project("# a test project\n");
    let devices = devices_by_backend();
    if devices.is_empty() {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: editor renderer backends");
        return;
    }
    const SIZE: u32 = 128;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    for (backend, device, queue) in devices {
        let scene = Arc::new(parking_lot::RwLock::new(engine_backend::scene::new_scene()));
        let mut renderer =
            engine_backend::services::gpu_renderer::GpuRendererBuilder::new(SIZE, SIZE)
                .scene_db(scene)
                .build();
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("backend test target"),
            size: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
        for pipeline in [None, Some(DEFERRED.to_string())] {
            if let Some(mailbox) = renderer.editor_mailbox() {
                mailbox.set_render_pipeline(pipeline.clone());
            }
            assert!(
                renderer.prepare_for_render(&device, &queue, SIZE, SIZE, format),
                "{backend:?}: the renderer builds with {pipeline:?}"
            );
            assert!(renderer.is_initialized(), "{backend:?}: {pipeline:?}");
            renderer.render_frame_to_surface(&device, &queue, &view, SIZE, SIZE, format);
        }
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        assert!(
            pollster::block_on(errors.pop()).is_none(),
            "{backend:?}: the GPU rejected what the renderer built"
        );
    }
}
