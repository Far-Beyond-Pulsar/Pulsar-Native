//! Virtual geometry for the examples: cook a mesh to a `.vgeo` (cached on disk),
//! register it with the VG runtime, and place instances of it. Triangle count
//! is then bounded by VRAM and the streamer, not by the classic mesh pool.
//!
//! Adapted from the museum scene (`plugins/vendor/meshes/examples/vg_stress*.rs`).

use bytemuck::Zeroable;
use glam::Mat4;
use helio_core::{
    BufferHandle, BufferKey, SceneDerivation, SceneDerivationContext, SceneDerivationOutput,
};
use helio_mesh_api::{MeshUpload, PackedVertex};
use helio_vg_build::{BuildOptions, MeshInput};
use helio_vg_format::{
    GpuVgInstance, VgAssetReadyRow, INST_CONE_CULL, INST_ENABLED, INST_LOD_SUPPRESS,
    INST_SHADOW_CASTER, VG_ASSET_READY_FLAG_READY,
};
use helio_vg_stream::{content_words, VgRuntime, VgeoBytes, VgeoSource};
use pulsar_scenedb::{Entity, World};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const VG_INSTANCES: BufferKey = BufferKey::of("vg_instances");
const VG_INSTANCE_COUNT: BufferKey = BufferKey::of("vg_instance_count");
const VG_ASSET_READY: BufferKey = BufferKey::of("vg_asset_ready");
const CLASSIC_SUPPRESSED: BufferKey = BufferKey::of("classic_suppressed");
const SCENE_OBJECTS_STRUCTURE: BufferKey = BufferKey::of("scene_objects_structure");
const VG_FLAGS: u32 = INST_ENABLED | INST_SHADOW_CASTER | INST_LOD_SUPPRESS | INST_CONE_CULL;

#[derive(Clone, Copy)]
struct VgTarget {
    vertex_offset: u32,
    vertex_count: u32,
    object_row: u32,
}

#[derive(Default)]
struct Targets {
    targets: Vec<VgTarget>,
    object_rows: u32,
    generation: u64,
    structure: u64,
}

/// One cooked asset: its placeholder mesh row and the interned range that keys it.
#[derive(Clone, Copy)]
pub struct Asset {
    pub mesh: Entity,
    offset: u32,
    count: u32,
}

pub struct Vg {
    pub runtime: VgRuntime,
    targets: Arc<Mutex<Targets>>,
    cache: PathBuf,
    placeholders: u32,
    by_content: HashMap<u128, Asset>,
    pub instances: usize,
    pub triangles: u64,
}

impl Vg {
    pub fn supported(device: &wgpu::Device) -> bool {
        helio_pass_vg::virtual_geometry_supported(device)
    }

    pub fn new(cache: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&cache);
        let runtime = VgRuntime::default();
        // Pre-rendered stills: every cluster is cut at the true screen-space error
        // (tau). The screen-wide density budget (default two triangles per pixel)
        // would raise tau for the whole view once distant crowds eat the budget,
        // coarsening the near trees while the far ones cannot go below their roots.
        let sensitivity = std::env::var("MUSKOKA_VG_SENSITIVITY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        let tau = std::env::var("MUSKOKA_VG_TAU")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.5);
        runtime.set_settings(helio_vg_stream::VgRuntimeSettings {
            selection_sensitivity: sensitivity,
            tau_px: tau,
            max_visible_clusters: 16_777_216,
            page_budget_mb: 4096,
            ..runtime.settings()
        });
        Self {
            runtime,
            targets: Arc::new(Mutex::new(Targets::default())),
            cache,
            placeholders: 0,
            by_content: HashMap::new(),
            instances: 0,
            triangles: 0,
        }
    }

    pub fn config(&self, config: helio::RendererConfig) -> helio::RendererConfig {
        helio_pass_vg::with_virtual_geometry(config, &self.runtime)
    }

    pub fn derivation(&self, device: &wgpu::Device) -> Box<dyn SceneDerivation> {
        Box::new(Derivation::new(
            device,
            self.runtime.clone(),
            self.targets.clone(),
        ))
    }

    /// Cooks (or loads from the cache) `mesh` and registers it. Identical
    /// geometry shares one asset.
    pub fn asset(
        &mut self,
        world: &mut World,
        vertices: &[PackedVertex],
        indices: &[u32],
    ) -> Result<Asset, String> {
        let content = content_id(vertices, indices);
        if let Some(found) = self.by_content.get(&content) {
            return Ok(*found);
        }
        let vgeo = load_or_cook(content, vertices, indices, &self.cache)?;
        self.placeholders += 1;
        let mut vertex = PackedVertex::default();
        vertex.tex_coords0 = [self.placeholders as f32, 0.0];
        let mesh = crate::common::spawn_mesh(
            world,
            MeshUpload {
                vertices: vec![vertex; 3],
                indices: vec![0, 1, 2],
            },
        );
        let mirror = world.gpu_mirror().cloned().ok_or("no GPU mirror")?;
        let range =
            helio_mesh_host::MeshComponent::vertices_gpu_handle(mirror.store(), mesh.index())
                .ok_or("placeholder vertex range")?;
        self.runtime
            .request_asset(content, range.offset, range.count, vgeo);
        let asset = Asset {
            mesh,
            offset: range.offset,
            count: range.count,
        };
        self.by_content.insert(content, asset);
        Ok(asset)
    }

    /// One instance of `asset`.
    pub fn instance(
        &mut self,
        world: &mut World,
        asset: Asset,
        material: Entity,
        transform: Mat4,
        radius: f32,
        triangles: u64,
    ) -> Result<(), String> {
        let object = crate::common::spawn_object(world, asset.mesh, material, transform, radius)
            .map_err(str::to_owned)?;
        let mut t = self.targets.lock().unwrap();
        t.object_rows = t.object_rows.max(object.index() + 1);
        t.structure += 1;
        t.targets.push(VgTarget {
            vertex_offset: asset.offset,
            vertex_count: asset.count,
            object_row: object.index(),
        });
        t.generation += 1;
        self.instances += 1;
        self.triangles += triangles;
        Ok(())
    }
}

// ── cooking ──────────────────────────────────────────────────────────────

fn content_id(vertices: &[PackedVertex], indices: &[u32]) -> u128 {
    let mut low = 0xcbf29ce484222325u64;
    let mut high = 0x84222325cbf29ce4u64;
    let bytes = bytemuck::cast_slice::<PackedVertex, u8>(vertices);
    for byte in bytes
        .iter()
        .copied()
        .chain(bytemuck::cast_slice::<u32, u8>(indices).iter().copied())
    {
        low = (low ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        high = (high ^ u64::from(byte).rotate_left(1)).wrapping_mul(0x9e3779b185ebca87);
    }
    (u128::from(high) << 64) | u128::from(low)
}

fn vgeo_path(cache: &Path, content: u128) -> PathBuf {
    cache.join(format!("{content:032x}.vgeo"))
}

fn read_cached(cache: &Path, content: u128) -> Option<Arc<VgeoBytes>> {
    let source = VgeoBytes::open(&vgeo_path(cache, content)).ok()?;
    (source.header().content_id == content_words(content)
        && helio_vg_build::is_current_cook(&source.header()))
    .then(|| Arc::new(source))
}

fn pack_attributes(vertices: &[PackedVertex]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vertices.len() * helio_vg_format::ATTR_STRIDE as usize);
    for vertex in vertices {
        let tangent = (vertex.tangent & 0x00ff_ffff)
            | ((if vertex.bitangent_sign < 0.0 {
                0x81u32
            } else {
                0x7fu32
            }) << 24);
        bytes.extend_from_slice(&vertex.normal.to_le_bytes());
        bytes.extend_from_slice(&tangent.to_le_bytes());
        for uv in [vertex.tex_coords0, vertex.tex_coords1] {
            bytes.extend_from_slice(&half::f16::from_f32(uv[0]).to_bits().to_le_bytes());
            bytes.extend_from_slice(&half::f16::from_f32(uv[1]).to_bits().to_le_bytes());
        }
    }
    bytes
}

fn load_or_cook(
    content: u128,
    vertices: &[PackedVertex],
    indices: &[u32],
    cache: &Path,
) -> Result<Arc<VgeoBytes>, String> {
    if let Some(hit) = read_cached(cache, content) {
        return Ok(hit);
    }
    let started = std::time::Instant::now();
    let positions: Vec<[f32; 3]> = vertices.iter().map(|v| v.position).collect();
    let attributes = pack_attributes(vertices);
    let mesh = MeshInput {
        positions: &positions,
        attributes: &attributes,
        attribute_stride: helio_vg_format::ATTR_STRIDE as usize,
        indices: &indices[..indices.len() / 3 * 3],
        tri_material: &[],
    };
    let bytes = helio_vg_build::build_vgeo(
        &mesh,
        &BuildOptions {
            content_id: content_words(content),
            resident_levels: 2,
        },
    )
    .map_err(|e| e.to_string())?;
    let path = vgeo_path(cache, content);
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, &path).map_err(|e| e.to_string())?;
    println!(
        "vg cook: {} triangles -> {:.1} MB in {:.1}s",
        indices.len() / 3,
        bytes.len() as f64 / 1e6,
        started.elapsed().as_secs_f64()
    );
    VgeoBytes::open(&path)
        .map(Arc::new)
        .map_err(|e| e.to_string())
}

// ── the scene derivation ─────────────────────────────────────────────────

fn select_visible(
    targets: &[VgTarget],
    ready: &[VgAssetReadyRow],
    classic: &mut [u32],
) -> Vec<GpuVgInstance> {
    let slots: HashMap<(u32, u32), u32> = ready
        .iter()
        .filter(|r| r.flags & VG_ASSET_READY_FLAG_READY != 0)
        .map(|r| ((r.vertex_offset, r.vertex_count), r.asset_slot))
        .collect();
    let mut out = Vec::new();
    for target in targets {
        let Some(&asset_slot) = slots.get(&(target.vertex_offset, target.vertex_count)) else {
            continue;
        };
        out.push(GpuVgInstance {
            asset_slot,
            flags: VG_FLAGS,
            lod_error_scale: 1.0,
            object_row: target.object_row,
        });
        if let Some(row) = classic.get_mut(target.object_row as usize) {
            *row = 1;
        }
    }
    out
}

struct Derivation {
    runtime: VgRuntime,
    targets: Arc<Mutex<Targets>>,
    instances: wgpu::Buffer,
    count: wgpu::Buffer,
    ready: wgpu::Buffer,
    classic: wgpu::Buffer,
    marker: wgpu::Buffer,
    instance_capacity: u32,
    object_rows: u32,
    ready_capacity: u32,
    content_generation: u64,
    buffer_epoch: u64,
    last_inputs: Option<(u64, u64)>,
}

fn buffer(device: &wgpu::Device, label: &'static str, bytes: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.max(16),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl Derivation {
    fn new(device: &wgpu::Device, runtime: VgRuntime, targets: Arc<Mutex<Targets>>) -> Self {
        Self {
            runtime,
            targets,
            instances: buffer(device, "VG Instances", 16),
            count: buffer(device, "VG Instance Count", 16),
            ready: buffer(device, "VG Asset Ready", 16),
            classic: buffer(device, "VG Classic Suppression", 4),
            marker: buffer(device, "VG Structure Marker", 16),
            instance_capacity: 1,
            object_rows: 1,
            ready_capacity: 1,
            content_generation: 1,
            buffer_epoch: 1,
            last_inputs: None,
        }
    }

    fn handle(b: &wgpu::Buffer, row_bytes: u64, generation: u64, epoch: u64) -> BufferHandle {
        BufferHandle {
            buffer: b.clone(),
            epoch,
            row_bytes,
            content_generation: generation,
        }
    }
}

impl SceneDerivation for Derivation {
    fn name(&self) -> &'static str {
        "MuskokaVg"
    }

    fn derive(
        &mut self,
        ctx: &SceneDerivationContext<'_>,
        _encoder: &mut wgpu::CommandEncoder,
    ) -> SceneDerivationOutput {
        self.runtime.service(ctx.device, ctx.queue);
        let targets = self.targets.lock().unwrap();
        let structure = targets.structure.max(1);
        let instance_count = targets.targets.len().max(1) as u32;
        let object_rows = targets.object_rows.max(1);
        let (ready_rows, version) = self.runtime.asset_ready_rows(1);
        let mut grew = instance_count > self.instance_capacity || object_rows > self.object_rows;
        if ready_rows.len() > self.ready_capacity as usize {
            self.ready_capacity = ready_rows.len().next_power_of_two() as u32;
            self.ready = buffer(
                ctx.device,
                "VG Asset Ready",
                self.ready_capacity as u64 * 16,
            );
            self.buffer_epoch += 1;
            grew = true;
        }
        if instance_count > self.instance_capacity {
            self.instance_capacity = instance_count.next_power_of_two();
            self.instances = buffer(
                ctx.device,
                "VG Instances",
                self.instance_capacity as u64 * 16,
            );
            self.buffer_epoch += 1;
        }
        if object_rows > self.object_rows {
            self.object_rows = object_rows.next_power_of_two();
            self.classic = buffer(
                ctx.device,
                "VG Classic Suppression",
                self.object_rows as u64 * 4,
            );
            self.buffer_epoch += 1;
        }
        let inputs = (targets.generation, version);
        if grew || self.last_inputs != Some(inputs) {
            self.last_inputs = Some(inputs);
            let mut classic = vec![0u32; self.object_rows as usize];
            let mut instances = select_visible(&targets.targets, &ready_rows, &mut classic);
            let count = [instances.len() as u32, 0, 0, 0];
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
                        buffer: self.marker.clone(),
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
