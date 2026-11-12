//! The exporter of the default render graph: `DefaultRenderGraph.rendergraph`
//! as a project ships it, generated, never hand-written.
//!
//! It builds the node registry as the editor does (`graph_node_registry`: the
//! core nodes, Helio's features and every installed plugin's nodes, with the
//! pins a probe found on a real device), makes the wired, sectioned, laid-out
//! default deferred graph from it (`deferred_graph_asset`), validates it
//! (routing included; any error fails the export, warnings are printed),
//! compiles it on the GPU and checks its pass list against the
//! deferred preset's, writes it through `engine_fs`, then reads the
//! file back through the render graph editor's loader and checks it again.
//!
//! Ignored: it writes a file. Re-run it whenever a plugin adds nodes:
//!
//! ```text
//! PULSAR_EXPORT_DIR=<project root> cargo test -p installed_plugins \
//!     --test export_default_graph -- --ignored --nocapture
//! ```
//!
//! `PULSAR_EXPORT_NAME` (default `DefaultRenderGraph`) names the file. The graph's
//! anti-aliasing node follows the project's TSR setting; `PULSAR_EXPORT_AA`
//! (`fxaa`, the default, or `tsr`) picks the config it is compiled and checked
//! with.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use engine_backend::subsystems::render::helio_renderer::{pipelines, render_features};
use helio::{DebugDrawState, PassBuildContext, RendererConfig};
use helio_core::GpuCameraUniforms;
use helio_graph_asset::{GraphAsset, Severity};
use helio_presets::Preset;
use pulsar_scenedb::gpu::{EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore};

// Every installed plugin, linked for its registrations.
use installed_plugins as _;

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
    let gpu_store = SceneGpuStore::new(
        &gpu_context,
        SceneGpuConfig {
            classes: Vec::new(),
            tombstone_headroom: 0,
            max_cells_metadata: 0,
        },
    );
    GpuMirrorHandle::new(Arc::new(gpu_store), Arc::clone(queue))
}

fn buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::UNIFORM
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Every node of the asset, main graph and sections.
fn all_nodes(asset: &GraphAsset) -> impl Iterator<Item = &helio_graph_asset::NodeInstance> {
    asset.nodes.iter().chain(
        asset
            .sections
            .iter()
            .flat_map(|section| section.nodes.iter()),
    )
}

/// The pass names a graph builds, in order.
fn pass_names(graph: &helio_core::RenderGraph) -> Vec<String> {
    graph
        .build_order_declarations()
        .into_iter()
        .map(|(pass, _)| pass.pass.to_string())
        .collect()
}

#[test]
#[ignore = "writes DefaultRenderGraph.rendergraph into PULSAR_EXPORT_DIR; see the module docs"]
fn export_the_default_render_graph() {
    let dir = PathBuf::from(
        std::env::var_os("PULSAR_EXPORT_DIR").expect("set PULSAR_EXPORT_DIR to the project root"),
    );
    let name = std::env::var("PULSAR_EXPORT_NAME").unwrap_or_else(|_| "DefaultRenderGraph".into());
    let tsr = match std::env::var("PULSAR_EXPORT_AA").as_deref() {
        Ok("tsr") => true,
        Ok("fxaa") | Err(_) => false,
        Ok(other) => panic!("PULSAR_EXPORT_AA is `fxaa` or `tsr`, not `{other}`"),
    };

    let (device, queue) = device().expect("the exporter needs a GPU adapter");
    // The probe runs on a device of its own (headless) unless one is offered;
    // the editor offers its shared one, the exporter lets it make its own.
    let registry = pipelines::graph_node_registry();
    assert!(
        registry.has_generated_pins(),
        "the pin probe found nothing ({:?}): the graph would be unwired",
        pipelines::graph_pin_probe_error()
    );

    let mut config = render_features::configure(RendererConfig::new(
        1920,
        1080,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ));
    if tsr {
        config = config.with_tsr_quality(helio::TsrQuality::Quality);
    }

    // The graph, wired, sectioned and laid out. Its anti-aliasing node
    // follows the project's TSR setting; `PULSAR_EXPORT_AA` picks the config
    // the export is compiled and checked with.
    let mut asset = helio_presets::graphs::deferred_graph_asset(&registry);
    asset.name = "Default Render Graph".into();
    asset.description = "The default deferred pipeline: every pass of the engine's deferred preset and of the installed plugins, wired and sectioned.".into();

    // Validate: routing included. Errors fail the export; warnings are listed.
    let validation = helio_graph_asset::validate(&asset, &registry);
    let mut warnings = 0;
    for diagnostic in &validation.diagnostics {
        match diagnostic.severity {
            Severity::Warning => {
                warnings += 1;
                println!("warning: {diagnostic}");
            }
            Severity::Error => println!("ERROR: {diagnostic}"),
        }
    }
    assert!(
        !validation.has_errors(),
        "the default graph has validation errors"
    );

    // Compile on the GPU, and compare with the preset the engine builds.
    let camera = buffer(
        &device,
        "Scene Camera Buffer",
        (std::mem::size_of::<GpuCameraUniforms>() * 2) as u64,
    );
    let debug_camera = buffer(&device, "Debug Camera Buffer", 64);
    let cull_stats = buffer(&device, "Cull Stats Buffer", 64);
    let scene_db = mirror(&device, &queue);
    let context = || PassBuildContext {
        device: &device,
        queue: &queue,
        config: config.clone(),
        debug_state: Arc::new(Mutex::new(DebugDrawState::default())),
        camera_buffer: &camera,
        debug_camera_buffer: &debug_camera,
        cull_stats_buffer: &cull_stats,
        owns_device: false,
        scene_db: scene_db.clone(),
    };
    let mut env = helio_graph_asset::build_env(&context());
    let graph = helio_graph_asset::compile(&asset, &registry, &mut env).unwrap_or_else(|errors| {
        panic!(
            "the default graph does not compile: {}",
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        )
    });
    let compiled = pass_names(&graph);
    let preset = pass_names(
        &Preset::Deferred
            .with_features(&render_features::plugin_features())
            .build(context()),
    );
    assert_eq!(
        compiled, preset,
        "the default graph and the deferred preset build different pass lists"
    );

    // Write it, then read it back as the render graph editor does.
    let path = dir.join(format!("{name}.rendergraph"));
    let json = asset.to_json();
    engine_fs::virtual_fs::write_file(&path, json.as_bytes()).expect("write the graph");
    let text = engine_fs::virtual_fs::read_to_string(&path).expect("read the graph back");
    let document =
        render_bps_plugin::document::decode(&text, &registry).expect("the editor loads it");
    let reloaded = document.buildable_asset(&registry);
    let revalidation = helio_graph_asset::validate(&reloaded, &registry);
    assert!(
        !revalidation.has_errors(),
        "the reloaded graph has errors: {:?}",
        revalidation
            .errors()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );
    let mut env = helio_graph_asset::build_env(&context());
    let rebuilt = helio_graph_asset::compile(&reloaded, &registry, &mut env)
        .unwrap_or_else(|errors| panic!("the reloaded graph does not compile: {errors:?}"));
    assert_eq!(
        pass_names(&rebuilt),
        compiled,
        "the file builds another pipeline"
    );

    // The report.
    let base = helio_presets::graphs::base_registry();
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for node in all_nodes(&asset) {
        *kinds.entry(node.kind.as_str()).or_default() += 1;
    }
    let plugin_kinds: Vec<_> = kinds
        .iter()
        .filter(|(kind, _)| base.get(kind).is_none() && registry.get(kind).is_some())
        .collect();
    println!("wrote {} ({} bytes)", path.display(), json.len());
    println!(
        "{} main-graph nodes, {} sections, {} nodes in all, {} connections in the main graph, {warnings} warning(s)",
        asset.nodes.len(),
        asset.sections.len(),
        all_nodes(&asset).count(),
        asset.connections.len()
    );
    for section in &asset.sections {
        let passes = section
            .nodes
            .iter()
            .filter(|node| node.kind.starts_with("helio.pass."))
            .count();
        println!(
            "section `{}`: {} nodes, {passes} passes",
            section.name,
            section.nodes.len()
        );
    }
    println!("{} passes build: {compiled:?}", compiled.len());
    println!("node kinds not in Helio's base registry (plugins): {plugin_kinds:?}");
}
