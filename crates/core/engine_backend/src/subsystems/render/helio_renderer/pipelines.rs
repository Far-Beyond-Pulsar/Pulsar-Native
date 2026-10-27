//! The render pipelines the engine can run.
//!
//! One process-wide [`RenderPipelineRegistry`] holds Helio's shipped graphs
//! (registered on first use) and whatever else registers: a plugin that
//! compiles a project's render-graph assets adds one entry per asset with
//! [`register_project_pipeline`]. The level editor lists them in World
//! Settings and stores the chosen id in the level; the viewport renderer and
//! the game both resolve that id here, so a pipeline authored once runs in
//! both.

use std::sync::{Arc, OnceLock, RwLock};

use helio::{IntoGraph, PipelineFactory, PipelineInfo, PipelineOrigin, RenderPipelineRegistry};

/// Project-relative suffix of a render-graph asset.
pub const RENDER_GRAPH_EXTENSION: &str = "rendergraph";

fn registry() -> &'static RwLock<RenderPipelineRegistry> {
    static REGISTRY: OnceLock<RwLock<RenderPipelineRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = RenderPipelineRegistry::new();
        helio_default_passes::pipelines::register_builtin_pipelines(&mut registry);
        RwLock::new(registry)
    })
}

/// Every offered pipeline: built-ins first, then project pipelines.
pub fn list_pipelines() -> Vec<PipelineInfo> {
    registry()
        .read()
        .map(|r| r.list())
        .unwrap_or_default()
        .into_iter()
        // A preview's pipeline is the editor's own, not a choice for a level.
        .filter(|info| !info.id.starts_with(PREVIEW_PREFIX))
        .collect()
}

/// Id prefix of pipelines made for a render graph editor's preview.
const PREVIEW_PREFIX: &str = "preview:";

/// Registers `asset` as a pipeline for a preview viewport and returns its id,
/// for [`crate::subsystems::render::HelioEditorMailbox::set_render_pipeline`].
/// Each call makes a new id (so the renderer rebuilds), and `replacing` retires
/// the previous one. Previews are not listed with [`list_pipelines`].
pub fn register_preview_pipeline(
    asset: helio_graph_asset::GraphAsset,
    replacing: Option<&str>,
) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = format!(
        "{PREVIEW_PREFIX}{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let asset = Arc::new(asset);
    let nodes = Arc::new(graph_node_registry());
    let factory: PipelineFactory = Arc::new(move || {
        Box::new(helio_graph_asset::GraphAssetPipeline::new(
            Arc::clone(&asset),
            Arc::clone(&nodes),
            helio_default_passes::build_default_graph_external_with_context,
        )) as Box<dyn IntoGraph>
    });
    if let Ok(mut registry) = registry().write() {
        if let Some(old) = replacing {
            registry.remove(old);
        }
        registry.register(
            PipelineInfo {
                id: id.clone(),
                label: "Preview".to_string(),
                description: String::new(),
                origin: PipelineOrigin::Project,
            },
            factory,
        );
    }
    id
}

/// The id the engine runs for `requested`: the request if it is registered,
/// else the registry default. `None` only if nothing is registered.
pub fn resolve_pipeline(requested: Option<&str>) -> Option<String> {
    registry()
        .read()
        .ok()
        .and_then(|r| r.resolve(requested).map(str::to_owned))
}

/// A new instance of the pipeline `requested` resolves to.
pub fn create_pipeline(requested: Option<&str>) -> Option<Box<dyn IntoGraph>> {
    registry().read().ok().and_then(|r| r.create(requested))
}

/// Offers a project-authored pipeline under `id` (its asset path).
/// Re-registering the same id replaces it, so a saved asset takes effect on
/// the next renderer rebuild.
pub fn register_project_pipeline(
    id: impl Into<String>,
    label: impl Into<String>,
    description: impl Into<String>,
    factory: PipelineFactory,
) {
    if let Ok(mut registry) = registry().write() {
        registry.register(
            PipelineInfo {
                id: id.into(),
                label: label.into(),
                description: description.into(),
                origin: PipelineOrigin::Project,
            },
            factory,
        );
    }
}

fn node_registry_cell() -> &'static RwLock<helio_graph_asset::NodeRegistry> {
    static NODES: OnceLock<RwLock<helio_graph_asset::NodeRegistry>> = OnceLock::new();
    NODES.get_or_init(|| RwLock::new(helio_default_passes::graphs::base_registry()))
}

/// Builds the default graph once on a headless device and returns the pins its
/// passes declare, and the SceneDB buffers the mirror registers.
fn probe_on_headless_device() -> Result<helio_graph_asset::Probe, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .map_err(|error| format!("no GPU adapter: {error}"))?;
    let features = helio::required_wgpu_features(adapter.features());
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: features,
        required_limits: helio::required_wgpu_limits(adapter.limits()),
        // Ray queries are experimental in wgpu; Helio asks for them with the features.
        experimental_features: helio::required_experimental_features(features),
        ..Default::default()
    }))
    .map_err(|error| format!("no GPU device: {error}"))?;
    probe_on(Arc::new(device), Arc::new(queue))
}

/// The editor's own device and queue, if one was offered before the probe ran.
static PROBE_DEVICE: OnceLock<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> = OnceLock::new();

/// Offers the editor's shared device for the pin probe, so it does not create a
/// second one. Only the first offer, made before the probe starts, is used.
pub fn offer_probe_device(device: wgpu::Device, queue: wgpu::Queue) {
    let _ = PROBE_DEVICE.set((Arc::new(device), Arc::new(queue)));
}

/// Builds the default graph on `device` (without locking it) and returns the
/// pins its passes declare, and the SceneDB buffers the mirror registers.
fn probe_on(
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
) -> Result<helio_graph_asset::Probe, String> {
    let mut scene = pulsar_scenedb::SceneDb::new();
    let scene_db = crate::scene::ensure_gpu_mirror(&mut scene, device.clone(), queue.clone());

    let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage,
            mapped_at_creation: false,
        })
    };
    let camera = buffer(
        "Probe Camera",
        (std::mem::size_of::<helio_core::GpuCameraUniforms>() * 2) as u64,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    );
    let debug_camera = buffer(
        "Probe Debug Camera",
        64,
        wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    );
    let cull_stats = buffer(
        "Probe Cull Stats",
        64,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
    );

    let config = helio::RendererConfig::new(64, 64, wgpu::TextureFormat::Bgra8UnormSrgb);
    let mut env = helio_graph_asset::BuildEnv::new(
        &device,
        &queue,
        config,
        Arc::new(std::sync::Mutex::new(helio::DebugDrawState::default())),
        &camera,
        &debug_camera,
        &cull_stats,
        false,
        scene_db,
    );
    // The registry as it is now: its pin table is shared, so the nodes it holds
    // get their pins when the result is applied.
    let registry = node_registry_cell()
        .read()
        .map(|r| r.clone())
        .map_err(|_| "node registry is poisoned".to_string())?;
    helio_default_passes::graphs::probe_pins(&registry, &mut env)
}

/// The probe, on the shared device when one was offered, else a headless one.
fn probe_run() -> Result<helio_graph_asset::Probe, String> {
    match PROBE_DEVICE.get() {
        Some((device, queue)) => probe_on(device.clone(), queue.clone()),
        None => probe_on_headless_device(),
    }
}

/// What the passes declare, found once per process by building the default graph
/// on a headless device (a second or two; see [`start_graph_pin_probe`]).
/// `None` when no GPU is available: stage nodes then have flow pins only.
static PROBE_ERROR: OnceLock<String> = OnceLock::new();

/// Why [`graph_pin_probe`] found nothing, once it has tried and failed.
pub fn graph_pin_probe_error() -> Option<&'static str> {
    PROBE_ERROR.get().map(String::as_str)
}

pub fn graph_pin_probe() -> Option<&'static helio_graph_asset::Probe> {
    static PROBE: OnceLock<Option<helio_graph_asset::Probe>> = OnceLock::new();
    PROBE
        .get_or_init(|| {
            match std::panic::catch_unwind(probe_run)
                .unwrap_or_else(|_| Err("building the passes panicked".to_string()))
            {
                Ok(probe) => Some(probe),
                Err(error) => {
                    tracing::warn!("Render graph nodes have no resource pins: {error}");
                    let _ = PROBE_ERROR.set(error);
                    None
                }
            }
        })
        .as_ref()
}

/// Starts [`graph_pin_probe`] on a background thread, so the pins are ready
/// when an editor first asks for the node registry.
pub fn start_graph_pin_probe() {
    std::thread::Builder::new()
        .name("render-graph-pin-probe".into())
        .spawn(|| {
            graph_pin_probe();
        })
        .ok();
}

/// The node types render-graph assets can use: the core resource nodes,
/// Helio's stages with the pins their passes declare (waits for the probe),
/// and whatever [`register_graph_node`] added. A snapshot, so the editor and
/// the compiler see one consistent set.
pub fn graph_node_registry() -> helio_graph_asset::NodeRegistry {
    if let Some(probe) = graph_pin_probe() {
        static APPLIED: std::sync::Once = std::sync::Once::new();
        APPLIED.call_once(|| {
            if let Ok(mut registry) = node_registry_cell().write() {
                helio_default_passes::graphs::apply_probe(&mut registry, probe);
            }
        });
    }
    node_registry_cell()
        .read()
        .map(|r| r.clone())
        .unwrap_or_default()
}

/// The default deferred pipeline as a graph, wired from the probed pins: the
/// template for a new render graph.
pub fn default_graph_template() -> helio_graph_asset::GraphAsset {
    let registry = graph_node_registry();
    helio_default_passes::graphs::deferred_graph_asset(&registry)
}
/// Offers a node type to render-graph assets. A plugin whose pass crate
/// supplies a node registers it here; the graphs already registered as
/// pipelines pick it up the next time the project's graphs are loaded.
pub fn register_graph_node(descriptor: helio_graph_asset::NodeDescriptor) {
    if let Ok(mut registry) = node_registry_cell().write() {
        registry.register(descriptor);
    }
}

/// What loading a project's render graphs found.
#[derive(Debug, Default)]
pub struct ProjectGraphReport {
    /// Asset paths registered as pipelines.
    pub registered: Vec<String>,
    /// Asset paths that could not be used, with why.
    pub failed: Vec<(String, String)>,
}

/// Registers every `.rendergraph` under `project_root` as a project pipeline
/// whose id is its project-relative path, replacing the previous set. A graph
/// with errors is reported in `failed` and not offered; one with only warnings
/// is offered. Call when a project opens and when a `.rendergraph` changes.
pub fn refresh_project_pipelines(project_root: &std::path::Path) -> ProjectGraphReport {
    clear_project_pipelines();
    let nodes = Arc::new(graph_node_registry());
    let mut report = ProjectGraphReport::default();
    for relative in engine_fs::virtual_fs::find_by_extension(project_root, RENDER_GRAPH_EXTENSION) {
        let id = relative.to_string_lossy().replace('\\', "/");
        let loaded = engine_fs::virtual_fs::read_file(&project_root.join(&relative))
            .map_err(|e| format!("{e:#}"))
            .and_then(|bytes| {
                helio_graph_asset::GraphAsset::from_json(&String::from_utf8_lossy(&bytes))
            });
        let asset = match loaded {
            Ok(asset) => asset,
            Err(error) => {
                report.failed.push((id, error));
                continue;
            }
        };
        let validation = helio_graph_asset::validate(&asset, &nodes);
        if let Some(error) = validation.errors().next() {
            report.failed.push((id, error.to_string()));
            continue;
        }
        let label = if asset.name.trim().is_empty() {
            relative
                .file_stem()
                .map_or_else(|| id.clone(), |stem| stem.to_string_lossy().into_owned())
        } else {
            asset.name.clone()
        };
        let description = if asset.description.is_empty() {
            format!("Render graph {id}")
        } else {
            asset.description.clone()
        };
        let asset = Arc::new(asset);
        let nodes = Arc::clone(&nodes);
        register_project_pipeline(
            id.clone(),
            label,
            description,
            Arc::new(move || {
                Box::new(helio_graph_asset::GraphAssetPipeline::new(
                    Arc::clone(&asset),
                    Arc::clone(&nodes),
                    helio_default_passes::build_default_graph_external_with_context,
                )) as Box<dyn IntoGraph>
            }),
        );
        report.registered.push(id);
    }
    report
}

/// Forgets every project pipeline (a project closed or is reloading).
pub fn clear_project_pipelines() {
    if let Ok(mut registry) = registry().write() {
        // Project graphs go; a preview's pipeline belongs to an open editor.
        registry.retain(|info| {
            info.origin != PipelineOrigin::Project || info.id.starts_with(PREVIEW_PREFIX)
        });
    }
}

fn level_pipeline_cell() -> &'static RwLock<Option<String>> {
    static LEVEL: OnceLock<RwLock<Option<String>>> = OnceLock::new();
    LEVEL.get_or_init(Default::default)
}

/// The pipeline the loaded level asks for. A game sets it when it loads a
/// level (the editor reaches the same renderer through its mailbox instead);
/// renderers built afterwards read it with [`level_pipeline`].
pub fn set_level_pipeline(id: Option<String>) {
    if let Ok(mut slot) = level_pipeline_cell().write() {
        *slot = id;
    }
}

pub fn level_pipeline() -> Option<String> {
    level_pipeline_cell()
        .read()
        .ok()
        .and_then(|slot| slot.clone())
}

/// The `world_settings.render_pipeline` of a level file's JSON text, if it
/// names one. A level without world settings, or without the key, has none.
pub fn render_pipeline_of_level(json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()?
        .get("world_settings")?
        .get("render_pipeline")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// Shared slot a UI thread writes and the render thread reads: the pipeline
/// the level asks for. A change rebuilds the renderer at the next frame
/// boundary, like the project's graph settings.
#[derive(Clone, Default)]
pub struct RequestedPipeline(Arc<std::sync::Mutex<Option<String>>>);

impl RequestedPipeline {
    pub fn set(&self, id: Option<String>) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = id;
        }
    }

    pub fn get(&self) -> Option<String> {
        self.0.lock().ok().and_then(|slot| slot.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_ids_fall_back_to_the_default() {
        let default = resolve_pipeline(None).expect("built-ins are registered");
        assert_eq!(default, helio_default_passes::pipelines::DEFAULT);
        assert_eq!(resolve_pipeline(Some("no/such.rendergraph")), Some(default));
    }

    #[test]
    fn project_pipelines_list_after_built_ins_and_clear() {
        let factory: PipelineFactory = Arc::new(|| {
            Box::new(
                |_ctx: helio::PassBuildContext<'_>| -> helio_core::RenderGraph {
                    unreachable!("not built in this test")
                },
            ) as Box<dyn IntoGraph>
        });
        register_project_pipeline("a/x.rendergraph", "X", "", factory);
        let listed = list_pipelines();
        assert_eq!(
            listed.last().map(|p| p.id.as_str()),
            Some("a/x.rendergraph")
        );
        assert_eq!(
            resolve_pipeline(Some("a/x.rendergraph")).as_deref(),
            Some("a/x.rendergraph")
        );
        clear_project_pipelines();
        assert!(!list_pipelines().iter().any(|p| p.id == "a/x.rendergraph"));
    }
}

#[cfg(test)]
mod level_tests {
    use super::render_pipeline_of_level;

    #[test]
    fn reads_the_pipeline_a_level_names() {
        let level = r#"{"world_settings":{"render_pipeline":"graphs/a.rendergraph"}}"#;
        assert_eq!(
            render_pipeline_of_level(level).as_deref(),
            Some("graphs/a.rendergraph")
        );
        assert_eq!(render_pipeline_of_level(r#"{"world_settings":{}}"#), None);
        assert_eq!(
            render_pipeline_of_level(r#"{"world_settings":{"render_pipeline":""}}"#),
            None
        );
        assert_eq!(render_pipeline_of_level("not json"), None);
    }
}
