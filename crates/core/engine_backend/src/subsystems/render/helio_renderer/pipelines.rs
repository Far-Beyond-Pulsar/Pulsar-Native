//! The render pipelines the engine can run.
//!
//! One process-wide [`RenderPipelineRegistry`] holds the pipelines providers
//! offer (Helio's shipped graphs, registered on first use) and whatever else
//! registers: the project's render-graph assets are compiled and added one
//! entry per asset with [`refresh_project_pipelines`]. A level names the
//! pipeline it runs (World Settings' asset picker stores the id); the
//! viewport renderer and the game both resolve that id here, so a pipeline
//! authored once runs in both. Every one builds over Helio's default
//! features and the ones plugins add ([`super::render_features`]).
//!
//! Pipelines are not implicit: there is no default. A level that names none,
//! or one that is not registered or does not compile, runs no pipeline, and
//! [`pipeline_state`] says why ([`PipelineState::reason`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use helio::{IntoGraph, PipelineFactory, PipelineInfo, PipelineOrigin, RenderPipelineRegistry};

/// Project-relative suffix of a render-graph asset.
pub const RENDER_GRAPH_EXTENSION: &str = "rendergraph";

fn registry() -> &'static RwLock<RenderPipelineRegistry> {
    // Static, not `EngineContext`: read by the standalone game and headless tools,
    // where no context is set; `RenderPipelineRegistry` has no `Default` either.
    static REGISTRY: OnceLock<RwLock<RenderPipelineRegistry>> = OnceLock::new();
    // The plugin set the built-ins were made for.
    static SYNCED: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);
    let cell = REGISTRY.get_or_init(|| RwLock::new(RenderPipelineRegistry::new()));
    let generation = pulsar_plugins::generation();
    let mut synced = SYNCED.lock().unwrap_or_else(|e| e.into_inner());
    if *synced != Some(generation) {
        // The built-ins bake in the plugins' features, so a project that
        // enables another set of plugins gets them made again; project and
        // preview pipelines stay.
        if let Ok(mut registry) = cell.write() {
            registry.retain(|info| info.origin != PipelineOrigin::BuiltIn);
            helio_presets::pipelines::register_builtin_pipelines(
                &mut registry,
                &super::render_features::plugin_features(),
            );
        }
        *synced = Some(generation);
    }
    cell
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

/// What the engine runs for a requested pipeline id. The request is all there
/// is: nothing is chosen for a level that names none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipelineState {
    /// A registered pipeline. `revision` changes each time its asset is
    /// registered again (a saved `.rendergraph`), so a renderer built from an
    /// older one rebuilds.
    Assigned { id: String, revision: u64 },
    /// The level names no pipeline.
    Unassigned,
    /// The level names a pipeline nobody registered.
    NotFound(String),
    /// The level names a render graph that does not compile.
    Invalid { id: String, reason: String },
}

impl PipelineState {
    /// Why no pipeline runs, for the error screen and the logs; `None` when one does.
    pub fn reason(&self) -> Option<String> {
        match self {
            Self::Assigned { .. } => None,
            Self::Unassigned => Some("No render pipeline assigned".to_string()),
            Self::NotFound(id) => Some(format!("Render pipeline `{id}` was not found")),
            Self::Invalid { id, reason } => {
                Some(format!("Render pipeline `{id}` is invalid: {reason}"))
            }
        }
    }

    /// What Helio's status screen shows for a pipeline that is named but not
    /// there, or not valid; `None` when one is assigned or none is named (the
    /// renderer's own "no pipeline" screen).
    pub fn failure(&self) -> Option<(String, helio::PipelineFailure)> {
        match self {
            Self::NotFound(id) => Some((
                id.clone(),
                helio::PipelineFailure::new(format!("Render pipeline `{id}` was not found"))
                    .with_errors([
                        "The level names a pipeline no provider offers and no project render graph has."
                            .to_string(),
                    ]),
            )),
            Self::Invalid { id, reason } => Some((
                id.clone(),
                helio::PipelineFailure::new(format!("Render pipeline `{id}` is invalid"))
                    .with_errors([reason.clone()]),
            )),
            Self::Assigned { .. } | Self::Unassigned => None,
        }
    }

    /// The registered id that runs.
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Assigned { id, .. } => Some(id),
            _ => None,
        }
    }
}

fn revisions() -> &'static Mutex<HashMap<String, u64>> {
    // Static, like the registry it describes.
    static REVISIONS: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    REVISIONS.get_or_init(Default::default)
}

fn bump_revision(id: &str) {
    if let Ok(mut revisions) = revisions().lock() {
        *revisions.entry(id.to_owned()).or_insert(0) += 1;
    }
}

fn revision_of(id: &str) -> u64 {
    revisions()
        .lock()
        .ok()
        .and_then(|revisions| revisions.get(id).copied())
        .unwrap_or(0)
}

/// Project render graphs that were found but are not offered, by id, with why.
fn failures() -> &'static Mutex<HashMap<String, String>> {
    static FAILURES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    FAILURES.get_or_init(Default::default)
}

/// What runs for `requested`; see [`PipelineState`].
pub fn pipeline_state(requested: Option<&str>) -> PipelineState {
    let Some(id) = requested.filter(|id| !id.is_empty()) else {
        return PipelineState::Unassigned;
    };
    let registered = registry().read().map_or(false, |r| r.contains(id));
    if registered {
        return PipelineState::Assigned {
            id: id.to_owned(),
            revision: revision_of(id),
        };
    }
    match failures().lock().ok().and_then(|f| f.get(id).cloned()) {
        Some(reason) => PipelineState::Invalid {
            id: id.to_owned(),
            reason,
        },
        None => PipelineState::NotFound(id.to_owned()),
    }
}

/// A new instance of the pipeline `requested` names; `None` when it names none
/// or one that is not registered (there is no default to fall back to).
pub fn create_pipeline(requested: Option<&str>) -> Option<Box<dyn IntoGraph>> {
    let id = requested.filter(|id| !id.is_empty())?;
    registry()
        .read()
        .ok()
        .filter(|r| r.contains(id))
        .and_then(|r| r.create(Some(id)))
}

/// Says once per build which pipeline its routing findings belong to. Helio
/// logs each finding as it builds the graph (`helio::routing`, an error when no
/// pass writes the final output, a black frame, a warning for the rest) but
/// does not know pipeline ids; this adds the id and the counts.
pub fn log_routing(pipeline: &str, issues: &[helio::RoutingIssue]) {
    let errors = issues
        .iter()
        .filter(|issue| issue.severity == helio::RoutingSeverity::Error)
        .count();
    let warnings = issues.len() - errors;
    if errors > 0 {
        tracing::error!(
            "render pipeline `{pipeline}`: {errors} routing error(s), {warnings} warning(s); the final output may stay black (see `helio::routing`)"
        );
    } else if warnings > 0 {
        tracing::warn!(
            "render pipeline `{pipeline}`: {warnings} routing warning(s) (see `helio::routing`)"
        );
    }
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
    let id = id.into();
    bump_revision(&id);
    register_entry(id, label, description, factory);
}

fn register_entry(
    id: String,
    label: impl Into<String>,
    description: impl Into<String>,
    factory: PipelineFactory,
) {
    if let Ok(mut registry) = registry().write() {
        registry.register(
            PipelineInfo {
                id,
                label: label.into(),
                description: description.into(),
                origin: PipelineOrigin::Project,
            },
            factory,
        );
    }
}

/// The node registry of one plugin set, with the pins a probe found for it.
struct NodeSet {
    /// [`pulsar_plugins::generation`] the registry was made for.
    generation: Option<u64>,
    registry: helio_graph_asset::NodeRegistry,
    /// Whether the probe of `generation` has been applied.
    pins_applied: bool,
}

/// The node registry, made again when the project's plugin set changes: the
/// core nodes, Helio's, and every enabled plugin's features' nodes
/// ([`super::render_features::plugin_features`]), with the nodes of disabled
/// plugins recorded as absent so a graph naming one says why it is missing.
fn node_set() -> std::sync::MutexGuard<'static, NodeSet> {
    // Static for the same reason as `REGISTRY`: no `EngineContext` outside the editor.
    static NODES: OnceLock<std::sync::Mutex<NodeSet>> = OnceLock::new();
    let cell = NODES.get_or_init(|| {
        std::sync::Mutex::new(NodeSet {
            generation: None,
            registry: Default::default(),
            pins_applied: false,
        })
    });
    let mut set = cell.lock().unwrap_or_else(|e| e.into_inner());
    let generation = pulsar_plugins::generation();
    if set.generation != Some(generation) {
        let mut registry =
            helio_presets::graphs::registry_with(&super::render_features::plugin_features());
        for (kind, plugin) in super::render_features::disabled_feature_nodes() {
            registry.mark_absent(kind, format!("plugin {plugin} disabled"));
        }
        set.registry = registry;
        set.generation = Some(generation);
        set.pins_applied = false;
    }
    set
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
// Static: a set-once offer with no `Default`, read by a background probe thread.
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

    let mut env = helio_graph_asset::build_env(&helio::PassBuildContext {
        device: &device,
        queue: &queue,
        // With the plugins' settings, so a node that builds nothing without its
        // feature's settings (the voxel terrain's) is probed as the pass it adds.
        config: super::render_features::configure(helio::RendererConfig::new(
            64,
            64,
            wgpu::TextureFormat::Bgra8UnormSrgb,
        )),
        debug_state: Arc::new(std::sync::Mutex::new(helio::DebugDrawState::default())),
        camera_buffer: &camera,
        debug_camera_buffer: &debug_camera,
        cull_stats_buffer: &cull_stats,
        owns_device: false,
        scene_db,
    });
    // The registry as it is now: its pin table is shared, so the nodes it holds
    // get their pins when the result is applied.
    let registry = node_set().registry.clone();
    helio_presets::graphs::probe_pins(&registry, &mut env)
}

/// The probe, on the shared device when one was offered, else a headless one.
fn probe_run() -> Result<helio_graph_asset::Probe, String> {
    match PROBE_DEVICE.get() {
        Some((device, queue)) => probe_on(device.clone(), queue.clone()),
        None => probe_on_headless_device(),
    }
}

/// Why [`graph_pin_probe`] found nothing, once it has tried and failed.
pub fn graph_pin_probe_error() -> Option<String> {
    probe_cache().1.clone()
}

/// The probe of one plugin set: what its passes declare, or why there is none.
type ProbeResult = (u64, Option<Arc<helio_graph_asset::Probe>>, Option<String>);

// Static: the process-wide probe, made again when the plugin set changes (the
// nodes of a disabled plugin are not probed, an enabled one's are).
static PROBE: std::sync::Mutex<Option<ProbeResult>> = std::sync::Mutex::new(None);

/// The probe for the current plugin set, run when there is none yet (a second
/// or two; see [`start_graph_pin_probe`]).
fn probe_cache() -> (Option<Arc<helio_graph_asset::Probe>>, Option<String>, u64) {
    let mut cached = PROBE.lock().unwrap_or_else(|e| e.into_inner());
    let generation = pulsar_plugins::generation();
    if cached.as_ref().map(|(g, ..)| *g) != Some(generation) {
        let result = std::panic::catch_unwind(probe_run)
            .unwrap_or_else(|_| Err("building the passes panicked".to_string()));
        *cached = Some(match result {
            Ok(probe) => (generation, Some(Arc::new(probe)), None),
            Err(error) => {
                tracing::warn!("Render graph nodes have no resource pins: {error}");
                (generation, None, Some(error))
            }
        });
    }
    let (generation, probe, error) = cached.clone().expect("just set");
    (probe, error, generation)
}

/// What the passes declare, found by building the default graph on a device.
/// `None` when no GPU is available: stage nodes then have flow pins only.
pub fn graph_pin_probe() -> Option<Arc<helio_graph_asset::Probe>> {
    probe_cache().0
}

/// Starts [`graph_pin_probe`] on a background thread, so the pins are ready
/// when an editor first asks for the node registry.
pub fn start_graph_pin_probe() {
    editor_task_queue::spawn_thread(
        editor_task_queue::ThreadSpec::new(
            "render-graph-pin-probe",
            editor_task_queue::ThreadRole::Other,
        )
        .owner("engine_backend"),
        || {
            graph_pin_probe();
        },
    )
    .ok();
}

/// The node types render-graph assets can use: the core resource nodes,
/// Helio's stages and every enabled plugin's nodes with the pins their passes
/// declare (waits for the probe). A snapshot, so the editor and the compiler
/// see one consistent set. A plugin that is disabled has no nodes here; a
/// graph that names one is reported as missing it, with the plugin.
pub fn graph_node_registry() -> helio_graph_asset::NodeRegistry {
    let (probe, _, probed_for) = probe_cache();
    let mut set = node_set();
    if let Some(probe) = probe {
        if !set.pins_applied && set.generation == Some(probed_for) {
            helio_presets::graphs::apply_probe(&mut set.registry, &probe);
            set.pins_applied = true;
        }
    }
    set.registry.clone()
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
    let mut failed_now: HashMap<String, String> = HashMap::new();
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
                failed_now.insert(id.clone(), error.clone());
                report.failed.push((id, error));
                continue;
            }
        };
        let validation = helio_graph_asset::validate(&asset, &nodes);
        if let Some(error) = validation.errors().next() {
            failed_now.insert(id.clone(), error.to_string());
            report.failed.push((id, error.to_string()));
            continue;
        }
        // A saved graph that did not change keeps its revision, so the
        // renderers running it are not rebuilt because another graph saved.
        let content = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            asset.to_json().hash(&mut hasher);
            hasher.finish()
        };
        let changed = content_hashes().lock().map_or(true, |mut hashes| {
            hashes.insert(id.clone(), content) != Some(content)
        });
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
        register_entry(
            id.clone(),
            label,
            description,
            Arc::new(move || {
                Box::new(helio_graph_asset::GraphAssetPipeline::new(
                    Arc::clone(&asset),
                    Arc::clone(&nodes),
                )) as Box<dyn IntoGraph>
            }),
        );
        if changed {
            bump_revision(&id);
        }
        report.registered.push(id);
    }
    if let Ok(mut failures) = failures().lock() {
        *failures = failed_now;
    }
    // A graph whose file is gone forgets its hash, so a new file of that path is new.
    if let Ok(mut hashes) = content_hashes().lock() {
        hashes.retain(|id, _| report.registered.contains(id));
    }
    report
}

/// Hash of each registered project graph's text, to tell a saved change from a reload.
fn content_hashes() -> &'static Mutex<HashMap<String, u64>> {
    static HASHES: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    HASHES.get_or_init(Default::default)
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
    // Static: `set_level_pipeline` is called by the standalone game, which has no
    // editor `EngineContext` to hang it on.
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

/// The pipeline a level saved before pipelines were explicit ran: the engine's
/// old default. A level without `world_settings.pipeline_version` (or with 0)
/// that names none is migrated to it, so it keeps rendering; the editor writes
/// the version on save, after which `None` means no pipeline.
pub const LEGACY_DEFAULT_PIPELINE: &str = "helio.deferred";

/// The version of the `render_pipeline` setting's meaning: 0 (absent) is the
/// implicit default, 1 is explicit (`None` is no pipeline).
pub const PIPELINE_VERSION: u32 = 1;

/// The pipeline a level's saved setting means, migrated: see
/// [`LEGACY_DEFAULT_PIPELINE`] and [`PIPELINE_VERSION`].
pub fn migrated_render_pipeline(saved_version: u32, saved: Option<String>) -> Option<String> {
    let saved = saved.filter(|id| !id.is_empty());
    if saved.is_none() && saved_version < PIPELINE_VERSION {
        Some(LEGACY_DEFAULT_PIPELINE.to_string())
    } else {
        saved
    }
}

/// The `world_settings.render_pipeline` of a level file's JSON text, migrated.
/// A level without world settings migrates as legacy.
pub fn render_pipeline_of_level(json: &str) -> Option<String> {
    let level: serde_json::Value = serde_json::from_str(json).ok()?;
    let settings = level.get("world_settings");
    let version = settings
        .and_then(|s| s.get("pipeline_version"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    let saved = settings
        .and_then(|s| s.get("render_pipeline"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    migrated_render_pipeline(version, saved)
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

    fn dummy_factory() -> PipelineFactory {
        Arc::new(|| {
            Box::new(
                |_ctx: helio::PassBuildContext<'_>| -> helio_core::RenderGraph {
                    unreachable!("not built in this test")
                },
            ) as Box<dyn IntoGraph>
        })
    }

    #[test]
    fn no_pipeline_runs_unless_the_level_names_a_registered_one() {
        assert_eq!(pipeline_state(None), PipelineState::Unassigned);
        assert_eq!(pipeline_state(Some("")), PipelineState::Unassigned);
        assert_eq!(
            pipeline_state(None).reason().as_deref(),
            Some("No render pipeline assigned")
        );
        assert_eq!(
            pipeline_state(Some("no/such.rendergraph")),
            PipelineState::NotFound("no/such.rendergraph".into())
        );
        assert!(create_pipeline(None).is_none());
        assert!(create_pipeline(Some("no/such.rendergraph")).is_none());
        // The shipped pipelines stay registered by id, offered by their provider.
        assert!(list_pipelines()
            .iter()
            .any(|p| p.id == LEGACY_DEFAULT_PIPELINE && p.origin == PipelineOrigin::BuiltIn));
        assert!(matches!(
            pipeline_state(Some(LEGACY_DEFAULT_PIPELINE)),
            PipelineState::Assigned { .. }
        ));
    }

    #[test]
    fn project_pipelines_list_after_built_ins_and_clear() {
        register_project_pipeline("a/x.rendergraph", "X", "", dummy_factory());
        let listed = list_pipelines();
        assert_eq!(
            listed.last().map(|p| p.id.as_str()),
            Some("a/x.rendergraph")
        );
        assert!(matches!(
            pipeline_state(Some("a/x.rendergraph")),
            PipelineState::Assigned { .. }
        ));
        clear_project_pipelines();
        assert!(!list_pipelines().iter().any(|p| p.id == "a/x.rendergraph"));
    }

    #[test]
    fn registering_a_pipeline_again_changes_its_state_so_renderers_rebuild() {
        register_project_pipeline("rev/x.rendergraph", "X", "", dummy_factory());
        let first = pipeline_state(Some("rev/x.rendergraph"));
        register_project_pipeline("rev/x.rendergraph", "X", "", dummy_factory());
        assert_ne!(first, pipeline_state(Some("rev/x.rendergraph")));
        clear_project_pipelines();
    }

    #[test]
    fn legacy_levels_keep_the_pipeline_they_ran_and_new_levels_may_have_none() {
        // Saved before pipelines were explicit: no key, or null, meant the default.
        for legacy in [
            r#"{"world_settings":{}}"#,
            r#"{"world_settings":{"render_pipeline":null}}"#,
            r#"{}"#,
        ] {
            assert_eq!(
                render_pipeline_of_level(legacy).as_deref(),
                Some(LEGACY_DEFAULT_PIPELINE),
                "{legacy}"
            );
        }
        // Saved since: null is no pipeline.
        assert_eq!(
            render_pipeline_of_level(
                r#"{"world_settings":{"pipeline_version":1,"render_pipeline":null}}"#
            ),
            None
        );
        assert_eq!(
            render_pipeline_of_level(
                r#"{"world_settings":{"pipeline_version":1,"render_pipeline":"graphs/a.rendergraph"}}"#
            )
            .as_deref(),
            Some("graphs/a.rendergraph")
        );
        assert_eq!(render_pipeline_of_level("not json"), None);
    }
}
