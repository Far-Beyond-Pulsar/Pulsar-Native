//! Standalone virtual-geometry stress scene built from the colocated scan FBXs.
//!
//! From `crates/renderer/helio`, run `cargo vg-stress` (alias for `cargo run -p examples --features vg-stress --bin vg_stress --release`).
//! Controls: WASD move, Space/Shift rise/fall, left click grabs the mouse,
//! Escape releases it (or exits when released).
//! Fog left Helio with its plugin (Pulsar `plugins/vendor/fog`); the museum
//! no longer has a medium.
//!
//! The sky (the atmosphere and its clouds) and GI left Helio with their
//! plugins (Pulsar `plugins/vendor/sky`, `plugins/vendor/gi`): this scene
//! has no sky and no GI probes; the background colour and the ambient
//! light stand in.

#![recursion_limit = "512"]

mod v3_demo_common;
mod vg_stress_bench;
mod vg_stress_capture;
mod vg_stress_cook;
mod vg_stress_loading;
/// Self-provisioning of the scan meshes (shared with `build.rs`).
mod vg_stress_prep;
mod vg_stress_data {
    pub mod dims;
    pub mod layout;
    pub mod materials;
    pub mod textures;
}

use helio::{
    Camera, HelioGraphicsConfig, HelioGraphicsContext, Renderer, RendererBuilder, RendererConfig,
};
use helio_core::{
    BufferHandle, BufferKey, SceneDerivation, SceneDerivationContext, SceneDerivationOutput,
};
use helio_vg_format::{
    GpuVgInstance, VgAssetReadyRow, INST_CONE_CULL, INST_ENABLED, INST_LOD_SUPPRESS,
    INST_SHADOW_CASTER, VG_ASSET_READY_FLAG_READY,
};
use helio_vg_stream::{VgRuntime, VgRuntimeSettings};
use pulsar_scenedb::{Entity, SceneDb};
use v3_demo_common::{
    directional_light, make_material, new_scene_db_with_gpu_mirror, point_light, scene_db_handle,
    spawn_light, spawn_material, spawn_mesh, spawn_object, spot_light,
};

use winit::{
    application::ApplicationHandler,
    event::*,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window, WindowId},
};

use bytemuck::Zeroable;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};
use vg_stress_data::{
    layout::{LightKind, Placement},
    materials::Mat,
};

const VG_INSTANCES: BufferKey = BufferKey::of("vg_instances");
const VG_INSTANCE_COUNT: BufferKey = BufferKey::of("vg_instance_count");
const VG_ASSET_READY: BufferKey = BufferKey::of("vg_asset_ready");
const CLASSIC_SUPPRESSED: BufferKey = BufferKey::of("classic_suppressed");
const SCENE_OBJECTS_STRUCTURE: BufferKey = BufferKey::of("scene_objects_structure");
const VG_FLAGS: u32 = INST_ENABLED | INST_SHADOW_CASTER | INST_LOD_SUPPRESS | INST_CONE_CULL;
/// The demo presents the scanned assets together on one metre-based stress
/// grid. Normalize their imported height to this scene height while
/// leaving the source mesh and its cached VG data unchanged.
const SCAN_SCENE_HEIGHT_METERS: f32 = 2.0;
/// True for the prepared Stanford scans (`vg_stress_prep::scans::MODELS`), which
/// are normalised to [`SCAN_SCENE_HEIGHT_METERS`] on load.
fn is_scan_asset(path: &std::path::Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|stem| {
            vg_stress_prep::scans::MODELS
                .iter()
                .any(|m| m.name.eq_ignore_ascii_case(stem))
        })
}

#[derive(Clone, Copy)]
struct VgTarget {
    vertex_offset: u32,
    vertex_count: u32,
    object_row: u32,
}

#[derive(Default)]
struct VgSceneTargets {
    targets: Vec<VgTarget>,
    object_rows: u32,
    /// Instances the last derivation made visible (ready assets only).
    visible: usize,
    /// Bumped whenever `targets` changes, so the derivation can skip frames
    /// on which neither the instances nor the asset readiness moved.
    generation: u64,
    /// Times the derivation rebuilt its rows / was called (bench counters).
    derive_rebuilds: u64,
    derive_calls: u64,
    /// Bumped whenever an object row is added (the role of the scene join's
    /// `scene_objects_structure` marker: BVH refit and object-batch placement-only
    /// runs rely on it to tell a move from a structural change).
    structure: u64,
    /// Bytes of the derivation's GPU buffers (instances, count, ready, classic).
    gpu_bytes: u64,
}

/// Which instances are drawn through virtual geometry this frame: exactly the
/// targets whose asset reports ready (root pages resident). Everything else
/// draws nothing, because VG parts own no classic geometry; `classic[row]` is
/// set for the object rows VG now owns. Pure, so the gating is unit tested.
fn select_visible(
    targets: &[VgTarget],
    ready: &[VgAssetReadyRow],
    classic: &mut [u32],
) -> Vec<GpuVgInstance> {
    let slots: HashMap<(u32, u32), u32> = ready
        .iter()
        .filter(|row| row.flags & VG_ASSET_READY_FLAG_READY != 0)
        .map(|row| ((row.vertex_offset, row.vertex_count), row.asset_slot))
        .collect();
    let mut instances = Vec::new();
    for target in targets {
        let Some(&asset_slot) = slots.get(&(target.vertex_offset, target.vertex_count)) else {
            continue;
        };
        instances.push(GpuVgInstance {
            asset_slot,
            flags: VG_FLAGS,
            lod_error_scale: 1.0,
            object_row: target.object_row,
        });
        if let Some(row) = classic.get_mut(target.object_row as usize) {
            *row = 1;
        }
    }
    instances
}

struct SceneVgDerivation {
    runtime: VgRuntime,
    enabled: bool,
    targets: Arc<Mutex<VgSceneTargets>>,
    instances: wgpu::Buffer,
    count: wgpu::Buffer,
    ready: wgpu::Buffer,
    classic: wgpu::Buffer,
    structure_marker: wgpu::Buffer,
    instance_capacity: u32,
    object_rows: u32,
    ready_capacity: u32,
    content_generation: u64,
    buffer_epoch: u64,
    /// (targets generation, ready version) of the last rebuild.
    last_inputs: Option<(u64, u64)>,
}

impl SceneVgDerivation {
    fn new(
        device: &wgpu::Device,
        runtime: VgRuntime,
        targets: Arc<Mutex<VgSceneTargets>>,
        enabled: bool,
    ) -> Self {
        let instance_capacity = 1;
        let object_rows = 1;
        let make = |label: &'static str, bytes: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes.max(16),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Self {
            runtime,
            enabled,
            targets,
            instances: make("VG Stress Instances", instance_capacity as u64 * 16),
            count: make("VG Stress Instance Count", 16),
            ready: make("VG Stress Asset Ready", 16),
            classic: make("VG Stress Classic Suppression", object_rows as u64 * 4),
            structure_marker: make("VG Stress Structure Marker", 16),
            instance_capacity,
            object_rows,
            ready_capacity: 1,
            content_generation: 1,
            buffer_epoch: 1,
            last_inputs: None,
        }
    }

    fn handle(buffer: &wgpu::Buffer, row_bytes: u64, generation: u64, epoch: u64) -> BufferHandle {
        BufferHandle {
            buffer: buffer.clone(),
            epoch,
            row_bytes,
            content_generation: generation,
        }
    }
}

impl SceneDerivation for SceneVgDerivation {
    fn name(&self) -> &'static str {
        "VgStressScene"
    }

    fn derive(
        &mut self,
        ctx: &SceneDerivationContext<'_>,
        _encoder: &mut wgpu::CommandEncoder,
    ) -> SceneDerivationOutput {
        if self.enabled {
            self.runtime.service(ctx.device, ctx.queue);
        }
        let mut targets = self.targets.lock().expect("VG scene targets");
        targets.derive_calls += 1;
        let structure = targets.structure.max(1);
        let instance_count = targets.targets.len().max(1) as u32;
        let object_rows = targets.object_rows.max(1);
        let make = |label, bytes: u64| {
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes.max(16),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let (ready_rows, version) = self.runtime.asset_ready_rows(1);
        let mut grew = instance_count > self.instance_capacity || object_rows > self.object_rows;
        if ready_rows.len() > self.ready_capacity as usize {
            self.ready_capacity = ready_rows.len().next_power_of_two() as u32;
            self.ready = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("VG Stress Asset Ready"),
                size: self.ready_capacity as u64 * 16,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.buffer_epoch += 1;
            grew = true;
        }
        if instance_count > self.instance_capacity {
            self.instance_capacity = instance_count.next_power_of_two();
            self.instances = make("VG Stress Instances", self.instance_capacity as u64 * 16);
            self.buffer_epoch += 1;
        }
        if object_rows > self.object_rows {
            self.object_rows = object_rows.next_power_of_two();
            self.classic = make("VG Stress Classic Suppression", self.object_rows as u64 * 4);
            self.buffer_epoch += 1;
        }

        // Instances only change when an instance is added or an asset's
        // readiness changes: most frames rewrite and re-publish nothing.
        let inputs = (targets.generation, version);
        if grew || self.last_inputs != Some(inputs) {
            self.last_inputs = Some(inputs);
            targets.derive_rebuilds += 1;
            targets.gpu_bytes = u64::from(self.instance_capacity) * 16
                + u64::from(self.object_rows) * 4
                + u64::from(self.ready_capacity) * 16
                + 32;
            let mut classic = vec![0u32; self.object_rows as usize];
            let visible: &[VgTarget] = if self.enabled { &targets.targets } else { &[] };
            let mut instances = select_visible(visible, &ready_rows, &mut classic);
            let count = [instances.len() as u32, 0, 0, 0];
            targets.visible = instances.len();
            instances.resize(self.instance_capacity as usize, GpuVgInstance::zeroed());
            ctx.queue
                .write_buffer(&self.instances, 0, bytemuck::cast_slice(&instances));
            ctx.queue
                .write_buffer(&self.count, 0, bytemuck::cast_slice(&count));
            ctx.queue
                .write_buffer(&self.ready, 0, bytemuck::cast_slice(&ready_rows));
            ctx.queue
                .write_buffer(&self.classic, 0, bytemuck::cast_slice(&classic));
            self.content_generation = self.content_generation.wrapping_add(1);
        }

        SceneDerivationOutput {
            buffers: vec![
                (
                    VG_INSTANCES,
                    Self::handle(
                        &self.instances,
                        16,
                        self.content_generation,
                        self.buffer_epoch,
                    ),
                ),
                (
                    VG_INSTANCE_COUNT,
                    Self::handle(&self.count, 16, self.content_generation, self.buffer_epoch),
                ),
                (
                    VG_ASSET_READY,
                    Self::handle(&self.ready, 16, self.content_generation, self.buffer_epoch),
                ),
                (
                    CLASSIC_SUPPRESSED,
                    Self::handle(&self.classic, 4, self.content_generation, self.buffer_epoch),
                ),
                (
                    SCENE_OBJECTS_STRUCTURE,
                    BufferHandle {
                        buffer: self.structure_marker.clone(),
                        epoch: 1,
                        row_bytes: 16,
                        content_generation: structure,
                    },
                ),
            ],
            ..Default::default()
        }
    }
}

/// Print what the scene contains and what it will cost, before loading.
fn print_scene_report(
    layout: &vg_stress_data::layout::MuseumLayout,
    config: &vg_stress_prep::catalog::Config,
    root: &std::path::Path,
) {
    let (kit, statues, trees, total) = layout.triangle_report();
    let fmt = |t: u64| format!("{:.1}M", t as f64 / 1e6);
    println!(
        "[Museum] density {:.2} (VG_STRESS_DENSITY / --density: low, normal, high, insane or a number); trees: {}",
        config.density,
        if config.trees.is_empty() {
            "none".to_string()
        } else {
            config
                .trees
                .iter()
                .map(|t| t.id)
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    println!(
        "[Museum] expected source triangles: {} total = architecture {} + statues {} ({} statue instances) + trees {} ({} tree instances)",
        fmt(total),
        fmt(kit),
        fmt(statues),
        layout.statues.len(),
        fmt(trees),
        layout.tree_count()
    );
    let masked: u64 = layout
        .placements
        .iter()
        .filter(|p| p.asset.contains('#'))
        .map(|p| {
            let (path, variant) = vg_stress_loading::split_key(&p.asset);
            vg_stress_prep::catalog::TREES
                .iter()
                .find(|t| path.contains(t.id))
                .and_then(|t| t.variants.iter().find(|v| Some(v.mesh) == variant))
                .map_or(0, |v| v.masked_triangles)
        })
        .sum();
    println!(
        "[Museum] of the tree triangles {} are alpha-masked cards/leaves (alpha-tested, cooked into VG like the rest)",
        fmt(masked)
    );
    let assets = root.join("assets/vg_stress");
    let mats = Mat::all(config);
    let jobs = vg_stress_data::textures::jobs(&assets, &mats, config);
    let tex = vg_stress_data::textures::estimate_bytes(&jobs);
    let download: u64 = vg_stress_prep::catalog::MARBLES
        .iter()
        .map(|m| vg_stress_prep::polyhaven::pinned_bytes(m.id))
        .chain(
            config
                .trees
                .iter()
                .map(|t| vg_stress_prep::polyhaven::pinned_bytes(t.id)),
        )
        .sum();
    println!(
        "[Museum] textures: {} uploads, about {:.2} GiB of GPU memory at quality {:?} (VG_STRESS_TEXTURES=low|balanced|max); Poly Haven downloads about {:.2} GB",
        jobs.len(),
        tex as f64 / (1u64 << 30) as f64,
        config.quality,
        download as f64 / 1e9
    );
}

/// `VG_STRESS_TAU` / the `-` and `=` keys: the VG screen-space error budget in
/// pixels. Larger is coarser and faster.
const TAU_RANGE: (f32, f32) = (0.1, 64.0);

/// The one sampler every SceneDB material texture slot shares. Helio's default
/// (`SamplerDescriptor::default()`) is ClampToEdge + Nearest, which smears
/// every tiled texture (UV > 1) into streaks of its edge texels and drops the
/// mips; the kit's marble UVs tile every 2 m, so this must be Repeat + trilinear.
fn material_sampler() -> wgpu::SamplerDescriptor<'static> {
    wgpu::SamplerDescriptor {
        label: Some("VG museum repeat trilinear sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: 16,
        ..Default::default()
    }
}

fn parse_tau(text: Option<&str>) -> f32 {
    text.and_then(|t| t.trim().parse::<f32>().ok())
        .filter(|t| t.is_finite() && *t > 0.0)
        .map_or(1.0, |t| t.clamp(TAU_RANGE.0, TAU_RANGE.1))
}

fn prepare_requested() -> bool {
    std::env::args().skip(1).any(|a| a == "--prepare")
        || std::env::var("VG_STRESS_PREPARE").is_ok_and(|v| !matches!(v.trim(), "" | "0" | "false"))
}

/// Headless provisioning: download/convert the sources if needed, then cook
/// every VG cache, with no window or GPU. After this `cargo vg-stress` starts
/// with everything ready. (`build.rs` cannot do this: cooking needs the VG
/// builder and importer crates, which are not build dependencies.)
fn prepare() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = vg_stress_prep::catalog::Config::from_env_and_args();
    let layout =
        vg_stress_data::layout::museum_layout_with(&vg_stress_data::dims::Dims::load(), &config);
    print_scene_report(&layout, &config, &root);
    let tau = parse_tau(std::env::var("VG_STRESS_TAU").ok().as_deref());
    let loader = vg_stress_loading::start(root, &layout.placements, true, config, tau, true);
    let (mut vg_assets, mut errors) = (0usize, 0usize);
    let started = std::time::Instant::now();
    while let Ok(event) = loader.events.recv() {
        use vg_stress_loading::Event;
        match event {
            Event::VgAsset { .. } => vg_assets += 1,
            Event::Status(status) => println!("[Museum] {status}"),
            Event::Error(error) => {
                errors += 1;
                eprintln!("[Museum] ERROR {error}");
            }
            Event::Finished => break,
            Event::Part(_) | Event::Texture { .. } | Event::Progress(_) => {}
        }
    }
    println!(
        "[Museum prepare] {vg_assets} VG assets ready, {errors} errors, {}",
        vg_stress_cook::format_duration(started.elapsed().as_secs_f64())
    );
    std::process::exit(if errors == 0 { 0 } else { 1 });
}

fn main() {
    env_logger::init();
    if prepare_requested() {
        return prepare();
    }
    let event_loop = EventLoop::new().expect("Failed to create event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("Event loop error");
}

struct App {
    state: Option<AppState>,
}

struct AppState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface_format: wgpu::TextureFormat,
    renderer: Renderer,
    last_frame: std::time::Instant,

    cam_pos: glam::Vec3,
    cam_yaw: f32,
    cam_pitch: f32,
    keys: HashSet<KeyCode>,
    cursor_grabbed: bool,
    mouse_delta: (f32, f32),

    scene_db: SceneDb,
    /// The scene's meshes for ray queries (the GI probes trace them).
    rays: v3_demo_common::SceneRays,
    runtime: VgRuntime,
    loader: Option<vg_stress_loading::Loader>,
    placements: HashMap<String, Vec<Placement>>,
    materials: HashMap<Mat, Entity>,
    targets: Arc<Mutex<VgSceneTargets>>,
    loading_errors: usize,
    loaded_objects: usize,
    started: std::time::Instant,
    frames: u64,
    render_errors: u64,
    smoke_seconds: Option<f32>,
    profile: bool,
    frame_samples: Vec<f32>,
    debug_views: Vec<helio_core::DebugViewDescriptor>,
    debug_index: usize,
    scene_triangles: u64,
    loading_status: String,
    last_title: std::time::Instant,
    capture: Option<vg_stress_capture::Capture>,
    post_stack: Option<vg_stress_capture::PostStack>,
    post_level: u8,
    sun_light: Option<Entity>,
    adapter_name: String,
    tau: f32,
    vg_expected: usize,
    vg_loaded: usize,
    /// Instancing audit counters (printed when loading finishes).
    classic_meshes: usize,
    vg_meshes: HashMap<u128, (Entity, u32, u32)>,
    classic_objects: usize,
    vg_objects: usize,
    classic_batches: HashSet<(u32, u32)>,
    placeholder_ordinal: u32,
    bench: Option<vg_stress_bench::BenchState>,
}

impl App {
    fn new() -> Self {
        Self { state: None }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let capture_options = vg_stress_capture::Options::from_env_and_args();
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Helio – VG Museum | starting")
                        .with_visible(
                            std::env::var_os("HELIO_VG_SMOKE_SECONDS").is_none()
                                && capture_options.is_none(),
                        )
                        .with_inner_size(winit::dpi::LogicalSize::new(1280u32, 720u32)),
                )
                .expect("window"),
        );

        let mut size = window.inner_size();
        if let Some(options) = &capture_options {
            size = winit::dpi::PhysicalSize::new(options.size.0, options.size.1);
        }
        let mut graphics = pollster::block_on(HelioGraphicsContext::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            HelioGraphicsConfig {
                vr_support: capture_options.is_none(),
                application_name: "Helio VG Museum".into(),
                ..Default::default()
            },
        ))
        .expect("Helio graphics context");
        let adapter = graphics.adapter.clone();
        let device = graphics.device.clone();
        let queue = graphics.queue.clone();
        device.on_uncaptured_error(std::sync::Arc::new(|e: wgpu::Error| {
            panic!("[GPU UNCAPTURED ERROR] {:?}", e);
        }));
        let info = adapter.get_info();
        println!(
            "[WGPU] Backend: {:?}, Device: {}, Driver: {}",
            info.backend, info.name, info.driver
        );
        let format = if capture_options.is_some() {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            graphics.surface_config.format
        };

        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        let runtime = VgRuntime::default();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let tau = parse_tau(std::env::var("VG_STRESS_TAU").ok().as_deref());
        let mode = match std::env::var("VG_STRESS_MODE")
            .ok()
            .as_deref()
            .map(str::trim)
        {
            Some("hw") => helio_vg_format::VgMode::ForceHwRaster,
            Some("nonrt") => helio_vg_format::VgMode::ForceNonRtFallback,
            Some("off") => helio_vg_format::VgMode::Off,
            _ => helio_vg_format::VgMode::Auto,
        };
        let tier3 = std::env::var_os("VG_STRESS_TIER3").is_some_and(|v| v != "0");
        let page_budget_mb = std::env::var("VG_STRESS_PAGE_MB")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or_else(|| runtime.settings().page_budget_mb);
        runtime.set_settings(VgRuntimeSettings {
            tau_px: tau,
            mode,
            tier3,
            page_budget_mb,
            ..runtime.settings()
        });
        println!("[Museum] VG mode {mode:?} (VG_STRESS_MODE=auto|hw|nonrt|off)");
        println!(
            "[Museum] Tier 3 dense tiles {} (VG_STRESS_TIER3=1 enables)",
            if tier3 { "ON" } else { "off" }
        );
        println!(
            "[Museum] VG error tolerance tau = {tau} px (VG_STRESS_TAU; - and = keys change it live)"
        );
        let museum_config = vg_stress_prep::catalog::Config::from_env_and_args();
        let mut layout = vg_stress_data::layout::museum_layout_with(
            &vg_stress_data::dims::Dims::load(),
            &museum_config,
        );
        // Instance benchmark: ONE asset, N placements, nothing else (VG_STRESS_INSTANCES).
        let bench_cfg = vg_stress_bench::BenchConfig::from_env_and_args();
        if let Some(b) = &bench_cfg {
            layout.placements = b.placements();
            layout.lights.clear();
            println!(
                "[Bench] {} instances of {:?}, grid {:.0} m, seed {}",
                b.instances,
                b.asset,
                b.extent(),
                b.seed
            );
        }
        print_scene_report(&layout, &museum_config, &root);
        let textures = Arc::new(RwLock::new(pulsar_scenedb::gpu::TextureStore::new(1024)));
        let mirror = scene_db_handle(&scene_db)
            .with_texture_store(textures)
            .expect("museum texture store");
        scene_db.world.attach_gpu_mirror(mirror);
        let targets = Arc::new(Mutex::new(VgSceneTargets::default()));
        let mut materials = HashMap::new();
        for material in Mat::all(&museum_config) {
            let surface = material.surface();
            let mut row = make_material(
                surface.base_color,
                surface.roughness,
                surface.metallic,
                surface.emissive_color,
                surface.emissive_intensity,
            );
            if material.tree_part().is_some_and(|(_, slot)| slot.masked) {
                // The catalogue's leaf materials are cutouts, not blended glass.
                row.flags |= helio_mats::FLAG_ALPHA_TEST | helio_mats::FLAG_DOUBLE_SIDED;
            }
            let entity = spawn_material(&mut scene_db.world, row);
            materials.insert(material, entity);
        }
        let sun_light = spawn_light(
            &mut scene_db.world,
            directional_light([0.35, -0.8, -0.3], [1.0, 0.94, 0.82], 3.0),
        );
        for light in &layout.lights {
            let direction = glam::Vec3::from_array(light.direction)
                .normalize_or_zero()
                .to_array();
            let gpu = match light.kind {
                LightKind::Point => {
                    point_light(light.position, light.color, light.intensity, light.range)
                }
                LightKind::Spot => spot_light(
                    light.position,
                    direction,
                    light.color,
                    light.intensity,
                    light.range,
                    light.inner_deg.to_radians(),
                    light.outer_deg.to_radians(),
                ),
                LightKind::Directional => {
                    directional_light(direction, light.color, light.intensity)
                }
            };
            spawn_light(&mut scene_db.world, gpu);
        }
        let mut placements: HashMap<String, Vec<Placement>> = HashMap::new();
        for placement in &layout.placements {
            placements
                .entry(placement.asset.clone())
                .or_default()
                .push(placement.clone());
        }
        let vg_supported = helio_pass_vg::virtual_geometry_supported(&device);
        if !vg_supported {
            log::warn!(
                "This device does not support Helio's virtual-geometry passes; rendering scans through the classic mesh path."
            );
        }
        let mut config = RendererConfig::new(size.width, size.height, format);
        if vg_supported {
            config = helio_pass_vg::with_virtual_geometry(config, &runtime);
        }
        let derivation =
            SceneVgDerivation::new(&device, runtime.clone(), targets.clone(), vg_supported);
        let builder = RendererBuilder::new(config, scene_db_handle(&scene_db))
            .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
            .with_scene_derivation(Box::new(derivation));
        let mut renderer = if capture_options.is_none() {
            builder
                .build_with_graphics_context(&mut graphics)
                .expect("Helio renderer")
        } else {
            builder.build(
                device.clone(),
                queue.clone(),
                size.width,
                size.height,
                format,
            )
        };
        renderer.set_material_sampler(&material_sampler());
        renderer.set_feature_live(helio::AmbientLight {
            color: [1.0, 0.95, 0.85],
            intensity: 0.05,
        });
        renderer.set_clear_color([0.018, 0.022, 0.035, 1.0]);
        let mut debug_views = renderer.available_debug_views();
        debug_views.push(helio_core::DebugViewDescriptor {
            name: "Normal",
            debug_mode: 0,
            description: "Lit museum scene",
        });
        debug_views.sort_by_key(|v| v.debug_mode);
        debug_views.dedup_by_key(|v| v.debug_mode);
        println!(
            "[Museum controls] 0 normal, F3 VG triangles, F4 VG clusters, F5 LOD level, F6 triangle size, F7 raster tier, F8 page residency, F9 error vs tau, [ / ] every debug view (VG_DEBUG_VIEW=<id or key> picks one at startup), F1 list views"
        );
        for view in &debug_views {
            println!(
                "  {}: {} – {}",
                view.debug_mode, view.name, view.description
            );
        }

        // No source parsing or cooking takes place on the window thread.
        let vg_expected = layout
            .placements
            .iter()
            .filter(|p| p.virtual_geometry)
            .map(|p| p.asset.as_str())
            .collect::<HashSet<_>>()
            .len();
        let capture = capture_options.map(|o| {
            let shots = bench_cfg
                .as_ref()
                .map_or_else(|| vg_stress_capture::load_shots(&root), |b| b.shots());
            vg_stress_capture::Capture::new(o, shots)
        });
        let adapter_name = info.name.clone();
        let loader = vg_stress_loading::start(
            root,
            &layout.placements,
            vg_supported,
            museum_config,
            tau,
            false,
        );
        println!(
            "[Museum] {} placements, {} exhibits; loading in background",
            layout.placements.len(),
            layout.statues.len()
        );
        let rays =
            v3_demo_common::SceneRays::new(&device, &queue).with_virtual_geometry(runtime.clone());
        let surface = graphics.surface;
        self.state = Some(AppState {
            window,
            surface,
            device,
            queue,
            surface_format: format,
            renderer,
            last_frame: std::time::Instant::now(),
            cam_pos: glam::Vec3::new(0.0, 3.0, 34.0),
            cam_yaw: 0.0,
            cam_pitch: 0.04,
            keys: HashSet::new(),
            cursor_grabbed: false,
            mouse_delta: (0.0, 0.0),
            scene_db,
            rays,
            runtime,
            loader: Some(loader),
            placements,
            materials,
            targets,
            loading_errors: 0,
            loaded_objects: 0,
            started: std::time::Instant::now(),
            frames: 0,
            render_errors: 0,
            smoke_seconds: std::env::var("HELIO_VG_SMOKE_SECONDS")
                .ok()
                .and_then(|s| s.parse().ok()),
            profile: std::env::var_os("HELIO_VG_PROFILE").is_some(),
            frame_samples: Vec::with_capacity(1024),
            debug_index: helio_vg_format::debug_view::from_env()
                .and_then(|view| debug_views.iter().position(|v| v.debug_mode == view.id()))
                .unwrap_or(0),
            debug_views,
            scene_triangles: 0,
            loading_status: "Loading museum".into(),
            last_title: std::time::Instant::now(),
            capture,
            post_stack: None,
            post_level: 0,
            sun_light: Some(sun_light),
            adapter_name,
            tau,
            vg_expected,
            vg_loaded: 0,
            classic_meshes: 0,
            vg_meshes: HashMap::new(),
            classic_objects: 0,
            vg_objects: 0,
            classic_batches: HashSet::new(),
            placeholder_ordinal: 0,
            bench: bench_cfg.map(vg_stress_bench::BenchState::new),
        });
        if let Some(state) = &mut self.state {
            if state.capture.is_none() {
                vg_stress_capture::set_interactive_post(state, 1);
            }
            if state.bench.is_some() {
                state.bench_ground();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        physical_key: PhysicalKey::Code(KeyCode::Escape),
                        ..
                    },
                ..
            } => {
                if state.cursor_grabbed {
                    state.cursor_grabbed = false;
                    let _ = state.window.set_cursor_grab(CursorGrabMode::None);
                    state.window.set_cursor_visible(true);
                } else {
                    event_loop.exit();
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ks,
                        physical_key: PhysicalKey::Code(key),
                        repeat,
                        ..
                    },
                ..
            } => match ks {
                ElementState::Pressed => {
                    if !repeat && state.debug_key(key) {
                        return;
                    }
                    state.keys.insert(key);
                }
                ElementState::Released => {
                    state.keys.remove(&key);
                }
            },
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if !state.cursor_grabbed {
                    let ok = state
                        .window
                        .set_cursor_grab(CursorGrabMode::Confined)
                        .or_else(|_| state.window.set_cursor_grab(CursorGrabMode::Locked))
                        .is_ok();
                    if ok {
                        state.window.set_cursor_visible(false);
                        state.cursor_grabbed = true;
                    }
                }
            }
            WindowEvent::Resized(s) if s.width > 0 && s.height > 0 => {
                state.surface.configure(
                    &state.device,
                    &wgpu::SurfaceConfiguration {
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        format: state.surface_format,
                        width: s.width,
                        height: s.height,
                        present_mode: wgpu::PresentMode::Fifo,
                        alpha_mode: wgpu::CompositeAlphaMode::Auto,
                        view_formats: vec![],
                        desired_maximum_frame_latency: 2,
                        color_space: wgpu::SurfaceColorSpace::Auto,
                    },
                );
                state.renderer.set_render_size(s.width, s.height);
            }
            WindowEvent::RedrawRequested => {
                if state.capture.is_some() {
                    return;
                }
                let now = std::time::Instant::now();
                let dt = (now - state.last_frame).as_secs_f32();
                state.last_frame = now;
                state.render(dt);
                if state.smoke_complete() {
                    event_loop.exit();
                    return;
                }
                state.window.request_redraw();
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: winit::event::DeviceId, event: DeviceEvent) {
        let Some(state) = &mut self.state else { return };
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if state.cursor_grabbed {
                state.mouse_delta.0 += dx as f32;
                state.mouse_delta.1 += dy as f32;
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(s) = &mut self.state {
            if let Some(mut capture) = s.capture.take() {
                event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
                let done = capture.step(s);
                s.capture = Some(capture);
                if done {
                    event_loop.exit();
                }
                return;
            }
            if s.smoke_seconds.is_some() {
                // Hidden windows do not receive normal redraw events on Windows.
                event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
                let now = std::time::Instant::now();
                let dt = (now - s.last_frame).as_secs_f32();
                s.last_frame = now;
                if !s.profile && s.frames > 0 && s.frames % 30 == 0 {
                    s.debug_key(KeyCode::BracketRight);
                }
                s.render(dt);
                if s.smoke_complete() {
                    event_loop.exit();
                }
                return;
            }
            s.window.request_redraw();
        }
    }
}

impl AppState {
    fn smoke_complete(&self) -> bool {
        if !self
            .smoke_seconds
            .is_some_and(|seconds| self.started.elapsed().as_secs_f32() >= seconds)
        {
            return false;
        }
        println!(
            "[Museum smoke] {} frames, {} objects, {} VG assets, {} loading errors, {} render errors, loader active={}",
            self.frames,
            self.loaded_objects,
            self.vg_meshes
                .keys()
                .filter(|&&id| self.runtime.has_asset(id))
                .count(),
            self.loading_errors,
            self.render_errors,
            self.loader.is_some()
        );
        assert!(
            self.frames > 1
                && self.loaded_objects > 0
                && self.render_errors == 0
                && self.loading_errors == 0,
            "museum startup smoke failed"
        );
        true
    }

    fn debug_key(&mut self, key: KeyCode) -> bool {
        let mode = match key {
            KeyCode::Digit0 => Some(0),
            KeyCode::F3 => Some(20),
            KeyCode::F4 => Some(21),
            KeyCode::F5 => Some(helio_vg_format::debug_view::VgDebugView::LodLevel.id()),
            KeyCode::F6 => Some(helio_vg_format::debug_view::VgDebugView::TriangleSize.id()),
            KeyCode::F7 => Some(helio_vg_format::debug_view::VgDebugView::RasterTier.id()),
            KeyCode::F8 => Some(helio_vg_format::debug_view::VgDebugView::PageResidency.id()),
            KeyCode::F9 => Some(helio_vg_format::debug_view::VgDebugView::ErrorVsTau.id()),
            KeyCode::Digit1 => Some(4),
            KeyCode::Digit2 => Some(5),
            KeyCode::Digit3 => Some(6),
            KeyCode::Digit4 => Some(7),
            KeyCode::Digit5 => Some(8),
            KeyCode::Digit6 => Some(9),
            KeyCode::Digit7 => Some(10),
            KeyCode::Digit8 => Some(22),
            KeyCode::Digit9 => Some(23),
            KeyCode::BracketRight | KeyCode::PageDown => {
                self.debug_index = (self.debug_index + 1) % self.debug_views.len();
                None
            }
            KeyCode::BracketLeft | KeyCode::PageUp => {
                self.debug_index =
                    (self.debug_index + self.debug_views.len() - 1) % self.debug_views.len();
                None
            }
            KeyCode::KeyP => {
                let next = (self.post_level + 1) % 3;
                vg_stress_capture::set_interactive_post(self, next);
                self.update_title();
                return true;
            }
            KeyCode::Minus | KeyCode::Equal => {
                self.adjust_tau(if key == KeyCode::Equal { 1.25 } else { 0.8 });
                return true;
            }
            KeyCode::F1 => {
                for view in &self.debug_views {
                    println!("{}: {} – {}", view.debug_mode, view.name, view.description);
                }
                return true;
            }
            _ => return false,
        };
        if let Some(mode) = mode {
            let mode = if self.debug_views[self.debug_index].debug_mode == mode {
                0
            } else {
                mode
            };
            if let Some(index) = self.debug_views.iter().position(|v| v.debug_mode == mode) {
                self.debug_index = index;
            } else {
                log::warn!("Debug mode {mode} is not available on this device");
                return true;
            }
        }
        let view = self.debug_views[self.debug_index];
        self.renderer.set_debug_mode(view.debug_mode);
        println!("[Museum view] {}: {}", view.name, view.description);
        self.update_title();
        true
    }

    fn update_title(&mut self) {
        if self.profile {
            self.frame_samples.sort_by(f32::total_cmp);
            let n = self.frame_samples.len();
            let percentile = |p: usize| {
                if n == 0 {
                    0.0
                } else {
                    self.frame_samples[((n - 1) * p) / 100]
                }
            };
            let timing = self.renderer.timing_snapshot();
            let mut passes = timing.passes.clone();
            passes.sort_by(|a, b| b.gpu_ms.unwrap_or(0.0).total_cmp(&a.gpu_ms.unwrap_or(0.0)));
            let top: Vec<_> = passes
                .iter()
                .take(6)
                .map(|p| format!("{}={:.2}ms", p.name, p.gpu_ms.unwrap_or(0.0)))
                .collect();
            let pool = self
                .runtime
                .with_store(|store| {
                    let s = store.stats();
                    format!(
                        "pool pages={}/{} (roots {}) uploads={} evict={} fail={} resident={:.0}MiB cpu_pages={}",
                        s.resident_pages, s.slot_limit, s.root_pages_resident, s.uploads, s.evictions, s.failures,
                        s.resident_bytes as f64 / 1048576.0, s.cpu_tracked_pages
                    )
                })
                .unwrap_or_default();
            eprintln!(
                "[Museum profile] t={:.2} frames={} loader={} p50={:.2} p99={:.2} max={:.2} gpu={:?} lag={:?} {} | {}",
                self.started.elapsed().as_secs_f32(),
                self.frames,
                self.loader.is_some(),
                percentile(50),
                percentile(99),
                self.frame_samples.last().copied().unwrap_or(0.0),
                timing.total_gpu_ms,
                timing.gpu_lag_frames,
                top.join(" "),
                pool
            );
            self.frame_samples.clear();
        }
        let vg_triangles = self
            .renderer
            .find_pass::<helio_pass_vg_cull::VgCullEarlyPass>()
            .and_then(|pass| pass.selected_triangle_count());
        // The cull's existing async stats readback: no extra GPU stall.
        let vg_stats = self
            .renderer
            .find_pass::<helio_pass_vg_cull::VgCullEarlyPass>()
            .and_then(|pass| pass.stats_snapshot())
            .map(|stats| stats.summary())
            .unwrap_or_default();
        self.window.set_title(&format!(
            "Helio – VG Museum | post {} (P) | {} | tau {:.2}px (-/=) | Scene tris: {} | VG selected: {} | VG assets {}/{} | {} | {}{}",
            ["off", "standard", "cinematic"][self.post_level as usize],
            self.debug_views[self.debug_index].name,
            self.tau,
            self.scene_triangles,
            vg_triangles
                .map(|n| n.to_string())
                .unwrap_or_else(|| "pending".into()),
            self.vg_loaded,
            self.vg_expected,
            vg_stats,
            self.loading_status,
            if self.loading_errors == 0 {
                String::new()
            } else {
                format!(" ({} errors)", self.loading_errors)
            }
        ));
        self.last_title = std::time::Instant::now();
    }

    /// Benchmark scene: a ground plane, plus `VG_STRESS_BENCH_CLASSIC` copies of one
    /// small classic mesh (the hardware-instancing check) beside the grid.
    fn bench_ground(&mut self) {
        let Some(cfg) = self.bench.as_ref().map(|b| b.cfg.clone()) else {
            return;
        };
        let ext = cfg.extent() * 0.75 + 200.0;
        let material = self.materials[&Mat::StatueLimestone];
        let (ground, _, _) = self.upload_mesh(v3_demo_common::plane_mesh([0.0, 0.0, 0.0], ext));
        spawn_object(
            &mut self.scene_db.world,
            ground,
            material,
            glam::Mat4::IDENTITY,
            ext * 1.5,
        )
        .expect("bench ground");
        if cfg.classic > 0 {
            let classic_material = self.materials[&Mat::StatueGreenStone];
            let (quad, _, _) = self.upload_mesh(v3_demo_common::plane_mesh([0.0, 0.0, 0.0], 0.8));
            let side = (f64::from(cfg.classic).sqrt().ceil() as u32).max(1);
            let origin = glam::Vec3::new(-(side as f32), 0.5, cfg.extent() * 0.5 + 10.0);
            for i in 0..cfg.classic {
                let t = vg_stress_bench::classic_transform(i, side, 2.0, origin);
                spawn_object(&mut self.scene_db.world, quad, classic_material, t, 1.2)
                    .expect("bench classic");
            }
            if let Some(b) = &mut self.bench {
                b.classic_objects = cfg.classic;
            }
            self.classic_objects += cfg.classic as usize;
        }
        if let Ok(mut t) = self.targets.lock() {
            t.structure += 1;
        }
    }

    /// Upload one classic mesh; returns its entity and interned vertex range.
    fn upload_mesh(&mut self, upload: helio_mesh_api::MeshUpload) -> (Entity, u32, u32) {
        let mesh = spawn_mesh(&mut self.scene_db.world, upload);
        let mirror = scene_db_handle(&self.scene_db);
        let vertices =
            helio_mesh_host::MeshComponent::vertices_gpu_handle(mirror.store(), mesh.index())
                .expect("uploaded mesh vertex range");
        (mesh, vertices.offset, vertices.count)
    }

    /// One object row per placement of `asset`'s part: instances share the
    /// mesh (or VG asset) and differ only by transform row.
    fn place_instances(
        &mut self,
        asset: &str,
        meta: &vg_stress_loading::PartMeta,
        mesh: Entity,
        range: (u32, u32),
        vg: bool,
    ) {
        let node = glam::Mat4::from_cols_array(&meta.transform);
        let spawn_started = std::time::Instant::now();
        let private_before = vg_stress_bench::private_bytes();
        let objects_before = self.loaded_objects;
        let mut targets = self.targets.lock().expect("VG scene targets");
        for placement in &self.placements[asset] {
            self.scene_triangles += meta.source_triangles;
            let material = placement
                .slots
                .iter()
                .find(|(slot, _)| {
                    slot == &meta.slot || meta.slot.strip_suffix(".001") == Some(slot.as_str())
                })
                .map(|(_, material)| *material)
                .unwrap_or_else(|| Mat::for_kit_slot(&meta.slot));
            let [pitch, yaw, roll] = placement.rotation.map(f32::to_radians);
            let transform = glam::Mat4::from_scale_rotation_translation(
                glam::Vec3::from_array(placement.scale),
                glam::Quat::from_euler(glam::EulerRot::YXZ, yaw, pitch, roll),
                glam::Vec3::from_array(placement.position.map(|v| v as f32)),
            ) * node;
            let scale = transform
                .x_axis
                .truncate()
                .length()
                .max(transform.y_axis.truncate().length())
                .max(transform.z_axis.truncate().length());
            let object = spawn_object(
                &mut self.scene_db.world,
                mesh,
                self.materials[&material],
                transform,
                meta.radius * scale,
            )
            .expect("museum object mesh ranges");
            targets.object_rows = targets.object_rows.max(object.index() + 1);
            targets.structure += 1;
            if let Some(bench) = &mut self.bench {
                bench.entities.push(object);
            }
            if vg {
                targets.targets.push(VgTarget {
                    vertex_offset: range.0,
                    vertex_count: range.1,
                    object_row: object.index(),
                });
                targets.generation += 1;
                self.vg_objects += 1;
            } else {
                self.classic_objects += 1;
                self.classic_batches
                    .insert((mesh.index(), self.materials[&material].index()));
            }
            self.loaded_objects += 1;
        }
        if let Some(bench) = &mut self.bench {
            if vg {
                bench.spawn_ms += spawn_started.elapsed().as_secs_f64() * 1000.0;
                bench.spawned_objects += (self.loaded_objects - objects_before) as u64;
                bench.private_before_spawn = private_before;
                bench.private_after_spawn = vg_stress_bench::private_bytes();
            }
        }
    }

    fn adjust_tau(&mut self, factor: f32) {
        self.tau = (self.tau * factor).clamp(TAU_RANGE.0, TAU_RANGE.1);
        self.runtime.set_settings(VgRuntimeSettings {
            tau_px: self.tau,
            ..self.runtime.settings()
        });
        println!("[Museum] VG tau = {:.2} px", self.tau);
        self.update_title();
    }

    /// What is shared and what is drawn, for the instancing audit.
    fn print_instancing_report(&self) {
        let vg_assets = self.vg_meshes.len();
        println!(
            "[Museum instancing] {} classic kit meshes + {} VG assets (one placeholder row and one .vgeo each) \
             = {} distinct geometries for {} object rows ({} VG, {} classic); classic (mesh, material) batches at spawn: {}",
            self.classic_meshes,
            vg_assets,
            self.classic_meshes + vg_assets,
            self.loaded_objects,
            self.vg_objects,
            self.classic_objects,
            self.classic_batches.len(),
        );
    }

    fn poll_loader(&mut self) {
        use std::sync::mpsc::TryRecvError;
        use vg_stress_loading::Event;
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_millis(3) {
            let Some(loader) = &self.loader else { break };
            let event = match loader.events.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.loader = None;
                    self.loading_errors += 1;
                    self.loading_status = "Loader stopped; see log".into();
                    break;
                }
            };
            match event {
                Event::Part(part) => {
                    // Classic geometry: an architecture kit piece (one mesh).
                    let Some(upload) = part.lods.into_iter().next() else {
                        break;
                    };
                    let (mesh, offset, count) = self.upload_mesh(upload);
                    self.classic_meshes += 1;
                    self.place_instances(&part.asset, &part.meta, mesh, (offset, count), false);
                    break;
                }
                Event::VgAsset { asset, parts } => {
                    // The asset is fully cooked: only now do its instances exist.
                    for part in parts {
                        let Some(words) = part.meta.content else {
                            continue;
                        };
                        let content = vg_stress_loading::content_id(words);
                        let (mesh, offset, count) = match self.vg_meshes.get(&content) {
                            Some(&found) => found,
                            None => {
                                // VG parts own no classic geometry. The mesh row is
                                // a zero-area placeholder whose interned vertex
                                // range is the identity of the VG asset.
                                self.placeholder_ordinal += 1;
                                let (mesh, offset, count) = self.upload_mesh(
                                    vg_stress_cook::placeholder_mesh(self.placeholder_ordinal),
                                );
                                self.runtime.request_asset(
                                    content,
                                    offset,
                                    count,
                                    part.vgeo.clone(),
                                );
                                self.vg_meshes.insert(content, (mesh, offset, count));
                                (mesh, offset, count)
                            }
                        };
                        self.place_instances(&asset, &part.meta, mesh, (offset, count), true);
                    }
                    self.vg_loaded += 1;
                    break;
                }
                Event::Progress(line) => self.loading_status = line,
                Event::Texture {
                    bindings,
                    srgb,
                    width,
                    height,
                    mips,
                } => {
                    let mirror = scene_db_handle(&self.scene_db);
                    let textures = mirror.texture_store().expect("museum texture store");
                    let descriptor = wgpu::TextureDescriptor {
                        label: Some("Museum texture"),
                        size: wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: mips.len() as u32,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: if srgb {
                            wgpu::TextureFormat::Rgba8UnormSrgb
                        } else {
                            wgpu::TextureFormat::Rgba8Unorm
                        },
                        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    };
                    let levels: Vec<&[u8]> = mips.iter().map(Vec::as_slice).collect();
                    match textures.write().expect("texture store").register_mips(
                        &self.device,
                        &self.queue,
                        &descriptor,
                        &levels,
                    ) {
                        Ok(slot) => {
                            for (material, channel) in bindings {
                                let Some(mut row) = self
                                    .scene_db
                                    .world
                                    .get_mut::<helio_components::MaterialComponent>(
                                    self.materials[&material],
                                ) else {
                                    continue;
                                };
                                match channel {
                                    0 => {
                                        row.tex_base_color = slot;
                                        row.base_color = [1.0; 4];
                                    }
                                    1 => row.tex_normal = slot,
                                    _ => {
                                        // ARM: occlusion R, roughness G, metalness B.
                                        // Factors multiply the maps, so pass them through.
                                        row.tex_roughness = slot;
                                        row.tex_occlusion = slot;
                                        row.roughness_metallic[0] = 1.0;
                                        row.roughness_metallic[1] = 1.0;
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            self.loading_errors += 1;
                            log::error!("Museum texture: {error:?}");
                        }
                    };
                    break;
                }
                Event::Status(status) => {
                    println!("[Museum] {status}");
                    self.loading_status = status;
                }
                Event::Error(error) => {
                    self.loading_errors += 1;
                    log::error!("Museum loading: {error}");
                }
                Event::Finished => {
                    self.loader = None;
                    self.loading_status = format!(
                        "Ready: {} objects, {}/{} VG assets",
                        self.loaded_objects, self.vg_loaded, self.vg_expected
                    );
                    println!(
                        "[Museum] Loading complete: {} objects, {} errors",
                        self.loaded_objects, self.loading_errors
                    );
                    self.print_instancing_report();
                    break;
                }
            }
        }
    }

    fn render(&mut self, dt: f32) {
        if self.profile && self.frame_samples.len() < 1024 {
            self.frame_samples.push(dt * 1000.0);
        }
        self.poll_loader();
        let dt = dt.min(0.1);
        const SPEED: f32 = 6.0;
        const SENS: f32 = 0.002;

        self.cam_yaw += self.mouse_delta.0 * SENS;
        self.cam_pitch = (self.cam_pitch - self.mouse_delta.1 * SENS).clamp(-1.4, 1.4);
        self.mouse_delta = (0.0, 0.0);

        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let forward = glam::Vec3::new(sy * cp, sp, -cy * cp);
        let right = glam::Vec3::new(cy, 0.0, sy);

        if self.keys.contains(&KeyCode::KeyW) {
            self.cam_pos += forward * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::KeyS) {
            self.cam_pos -= forward * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::KeyA) {
            self.cam_pos -= right * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::KeyD) {
            self.cam_pos += right * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::Space) {
            self.cam_pos += glam::Vec3::Y * SPEED * dt;
        }
        if self.keys.contains(&KeyCode::ShiftLeft) {
            self.cam_pos -= glam::Vec3::Y * SPEED * dt;
        }

        let size = self.window.inner_size();
        let aspect = size.width as f32 / size.height.max(1) as f32;
        let camera = Camera::perspective_look_at(
            self.cam_pos,
            self.cam_pos + forward,
            glam::Vec3::Y,
            std::f32::consts::FRAC_PI_4,
            aspect,
            0.1,
            250.0,
        );

        v3_demo_common::flush_scene_db(&self.scene_db, &self.queue);
        self.rays.publish(&self.scene_db.world, &mut self.renderer);
        if let Err(e) = self.renderer.render_window_frame(&camera, &self.surface) {
            log::error!("Render: {:?}", e);
            self.render_errors += 1;
        }
        self.frames += 1;
        if self.last_title.elapsed() >= std::time::Duration::from_millis(250) {
            self.update_title();
        }
        if self.frames == 1 {
            println!(
                "[Museum] First frame in {:.3}s; {} objects, loader active={}",
                self.started.elapsed().as_secs_f32(),
                self.loaded_objects,
                self.loader.is_some()
            );
        }
    }
}

#[cfg(test)]
mod gating_tests {
    use super::*;

    fn target(offset: u32, row: u32) -> VgTarget {
        VgTarget {
            vertex_offset: offset,
            vertex_count: 3,
            object_row: row,
        }
    }

    fn ready(offset: u32, slot: u32, flags: u32) -> VgAssetReadyRow {
        VgAssetReadyRow {
            vertex_offset: offset,
            vertex_count: 3,
            asset_slot: slot,
            flags,
        }
    }

    #[test]
    fn instances_are_visible_only_once_their_asset_is_ready() {
        // Two assets: A (rows 0, 1) and B (row 2).
        let targets = [target(100, 0), target(100, 1), target(200, 2)];
        let mut classic = vec![0u32; 3];

        // Nothing resident yet: no instance, nothing suppressed, nothing drawn.
        let none = select_visible(&targets, &[VgAssetReadyRow::PADDING; 4], &mut classic);
        assert!(none.is_empty() && classic == [0, 0, 0]);

        // Registered but root pages not resident (flag clear): still hidden.
        let pending = [ready(100, 0, 0), ready(200, 1, 0)];
        assert!(select_visible(&targets, &pending, &mut classic).is_empty());

        // A becomes ready: exactly its two rows appear, B stays hidden.
        let a_ready = [ready(100, 5, VG_ASSET_READY_FLAG_READY), ready(200, 1, 0)];
        let visible = select_visible(&targets, &a_ready, &mut classic);
        assert_eq!(
            visible
                .iter()
                .map(|i| (i.asset_slot, i.object_row))
                .collect::<Vec<_>>(),
            [(5, 0), (5, 1)]
        );
        assert_eq!(classic, [1, 1, 0]);

        // Both ready, with the other part of the same asset key space unaffected.
        let both = [
            ready(100, 5, VG_ASSET_READY_FLAG_READY),
            ready(200, 7, VG_ASSET_READY_FLAG_READY),
        ];
        let mut classic = vec![0u32; 3];
        assert_eq!(select_visible(&targets, &both, &mut classic).len(), 3);
        assert_eq!(classic, [1, 1, 1]);
    }

    #[test]
    fn tau_parses_and_clamps() {
        assert_eq!(parse_tau(None), 1.0);
        assert_eq!(parse_tau(Some("2.5")), 2.5);
        assert_eq!(parse_tau(Some("nope")), 1.0);
        assert_eq!(parse_tau(Some("-3")), 1.0);
        assert_eq!(parse_tau(Some("1000")), TAU_RANGE.1);
    }
}

#[cfg(test)]
mod sampler_tests {
    #[test]
    fn material_sampler_repeats_and_filters_trilinearly() {
        let s = super::material_sampler();
        assert_eq!(s.address_mode_u, wgpu::AddressMode::Repeat);
        assert_eq!(s.address_mode_v, wgpu::AddressMode::Repeat);
        assert_eq!(s.mipmap_filter, wgpu::MipmapFilterMode::Linear);
        assert_eq!(s.min_filter, wgpu::FilterMode::Linear);
        assert!(s.anisotropy_clamp > 1);
        // The default it replaces is the streaking bug.
        let d = wgpu::SamplerDescriptor::default();
        assert_eq!(d.address_mode_u, wgpu::AddressMode::ClampToEdge);
    }
}
