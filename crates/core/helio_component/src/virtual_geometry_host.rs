//! The CPU side of virtual geometry for a host (editor, game): which meshes
//! need a `.vgeo`, building and caching it off-thread, handing it to the
//! streaming runtime, and publishing which assets are ready to the GPU.
//!
//! ```text
//! host frame          VirtualGeometryHost::sync(world, mirror, settings)
//!                       - settings -> VgRuntime, VgInstanceControl
//!                       - meshes of owners with an enabled VirtualGeometryComponent
//!                           -> embedded `.vgeo` chunk in the native `.mesh`
//!                           -> VgRuntime::request_asset(content, interned vertex range)
//! scene derivations   VgAssetReadyPublisher  -> `vg_asset_ready` (rows from the runtime)
//!                     VirtualGeometryInstances -> `vg_instances`, `lod_suppressed`,
//!                                                 `classic_suppressed`
//! graph passes        stream pages, cull, raster, shade (helio-default-passes)
//! ```
//!
//! Until an asset's root pages are resident its instances are not VG-active:
//! they render classically and their authored LOD applies (plan section 3,
//! item 7). Only static meshes take part; skinned meshes, voxels and foliage
//! are other components and stay classic. The mesh retains classic geometry
//! as a non-destructive fallback; the independently versioned VG chunk is
//! embedded in the same native asset.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;

use helio_core::{
    BufferKey, PackedVertex, SceneDerivation, SceneDerivationContext, SceneDerivationOutput,
};
use helio_vg_format::{VgAssetReadyRow, FORMAT_VERSION};
use helio_vg_stream::{content_words, VgRuntime, VgRuntimeSettings, VgeoBytes, VgeoSource};
use pulsar_scenedb::gpu::GpuMirrorHandle;
use pulsar_scenedb::handle_ledger::HandleId;
use pulsar_scenedb::{Entity, World};

use crate::components::{MeshAssetPath, StaticMeshComponent, VirtualGeometryComponent};
use crate::virtual_geometry_instances::{
    Output, VgInstanceControl, VgInstanceKeys, VirtualGeometryInstances, VG_ASSET_READY_KEY,
    VG_ASSET_READY_ROW_BYTES,
};

/// How often (in `sync` calls) the world is rescanned when no build is in
/// flight. Builds in flight are polled every call.
const SCAN_INTERVAL: u64 = 8;

enum HostAsset {
    Building(mpsc::Receiver<Result<Arc<VgeoBytes>, String>>),
    Ready(Arc<VgeoBytes>),
    Failed,
}

/// See the module doc. Owned by whoever owns the renderer.
pub struct VirtualGeometryHost {
    runtime: VgRuntime,
    control: Option<VgInstanceControl>,
    assets: HashMap<u128, HostAsset>,
    calls: u64,
}

impl Default for VirtualGeometryHost {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualGeometryHost {
    pub fn new() -> Self {
        Self {
            runtime: VgRuntime::new(VgRuntimeSettings::default()),
            control: None,
            assets: HashMap::new(),
            calls: 0,
        }
    }

    /// The streaming runtime, for `RendererConfig::with_virtual_geometry`.
    pub fn runtime(&self) -> &VgRuntime {
        &self.runtime
    }

    /// The `vg_asset_ready` publisher. Install it after the scene join and
    /// before [`Self::instances_derivation`].
    pub fn asset_ready_derivation(&self, device: &wgpu::Device) -> Box<VgAssetReadyPublisher> {
        Box::new(VgAssetReadyPublisher::new(device, self.runtime.clone()))
    }

    /// The instance derivation. Install it after [`Self::asset_ready_derivation`]
    /// and before the authored-LOD selection.
    pub fn instances_derivation(&mut self, device: &wgpu::Device) -> Box<VirtualGeometryInstances> {
        let derivation = Box::new(VirtualGeometryInstances::new(device, VgInstanceKeys::standard()));
        let control = derivation.control();
        control.set_enabled(self.runtime.settings().mode != helio_vg_format::VgMode::Off);
        self.control = Some(control);
        derivation
    }

    /// Per-frame (or per-tick) service: pushes `settings`, reaps finished
    /// builds, and starts builds for new meshes. Cheap when nothing changed.
    pub fn sync(&mut self, world: &World, mirror: &GpuMirrorHandle, settings: VgRuntimeSettings) {
        self.runtime.set_settings(settings);
        let on = settings.mode != helio_vg_format::VgMode::Off;
        if let Some(control) = &self.control {
            control.set_enabled(on);
        }
        self.calls += 1;
        let building = self.assets.values().any(|a| matches!(a, HostAsset::Building(_)));
        if !building && self.calls % SCAN_INTERVAL != 1 {
            return;
        }

        // Reap finished builds.
        for asset in self.assets.values_mut() {
            if let HostAsset::Building(rx) = asset {
                match rx.try_recv() {
                    Ok(Ok(bytes)) => *asset = HostAsset::Ready(bytes),
                    Ok(Err(error)) => {
                        tracing::warn!("virtual geometry: build failed: {error}");
                        *asset = HostAsset::Failed;
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => *asset = HostAsset::Failed,
                }
            }
        }
        if !on {
            return;
        }

        // Meshes whose owner has an enabled `VirtualGeometryComponent`.
        let owners: HashSet<Entity> =
            pulsar_scene_model::attachments::enabled_components::<VirtualGeometryComponent>(world)
                .filter(|(_, _, component)| component.enabled)
                .map(|(_, owner, _)| owner)
                .collect();
        if owners.is_empty() {
            return;
        }
        let pool = mirror
            .store()
            .interned_var_len_pool::<PackedVertex>(BufferKey::of("builtin_mesh_vertex"));
        for (_, owner, mesh) in
            pulsar_scene_model::attachments::enabled_components::<StaticMeshComponent>(world)
        {
            if !owners.contains(&owner) || mesh.vertices.is_empty() || mesh.indices.len() < 3 {
                continue;
            }
            let content = mesh.geometry_key.0;
            if content == 0 || !is_vg_eligible(mesh) {
                continue;
            }
            if !self.assets.contains_key(&content) {
                let job = spawn_build(content, mesh);
                self.assets.insert(content, HostAsset::Building(job));
            }
            // Ready assets are (re)offered every scan: the interned vertex
            // range is what identifies the mesh on the GPU and can move.
            if let (Some(HostAsset::Ready(bytes)), Some(pool)) = (self.assets.get(&content), &pool) {
                if let Some((offset, count)) = pool.resolve(HandleId(content)) {
                    let source: Arc<dyn VgeoSource> = bytes.clone();
                    self.runtime.request_asset(content, offset, count, source);
                }
            }
        }
    }
}

/// Whether a mesh may be virtual geometry. Blended (translucent) surfaces never
/// can: a mesh with a translucent surface slot stays classic. Alpha-TESTED
/// (masked) materials can: their rows carry `FLAG_ALPHA_TEST`, the cull routes
/// those clusters to the masked bins and the raster tiers evaluate the alpha
/// test (plan section 12, "Masked materials"). Masked-ness lives in the scene
/// material row, not in the surface, so nothing here needs to know about it.
fn is_vg_eligible(mesh: &StaticMeshComponent) -> bool {
    mesh.material_slots.slots.iter().all(|slot| {
        let surface = slot.surface_override.as_ref().unwrap_or(&slot.imported_surface);
        surface.alpha >= 0.999
    })
}

/// Resolve the native mesh file so its baked VG chunk lives with the geometry.
fn native_mesh_path(mesh: &MeshAssetPath) -> Option<PathBuf> {
    if mesh.generated_content_id().is_some() {
        return None;
    }
    let root = engine_state::get_project_path()?;
    let source = crate::components::mesh_source(&root, mesh.as_str());
    (!source.is_empty()).then(|| PathBuf::from(source))
}

/// Generated meshes have no native file, so retain a temporary cache for them.
fn generated_cache_path(content: u128) -> PathBuf {
    let name = format!("{content:032x}.v{FORMAT_VERSION}.vgeo");
    std::env::temp_dir().join("pulsar-vgeo").join(name)
}

fn f32_to_f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x007f_ffff;
    if exponent == 0xff {
        return sign | 0x7c00 | if mantissa != 0 { 0x0200 } else { 0 };
    }
    let unbiased = exponent - 127 + 15;
    if unbiased >= 0x1f {
        return sign | 0x7c00;
    }
    if unbiased <= 0 {
        if unbiased < -10 {
            return sign;
        }
        let m = mantissa | 0x0080_0000;
        let shift = (14 - unbiased) as u32;
        let half = (m >> shift) as u16;
        let round = ((m >> (shift - 1)) & 1) as u16;
        return sign | (half + round);
    }
    let half = sign | ((unbiased as u16) << 10) | (mantissa >> 13) as u16;
    // Round to nearest.
    if mantissa & 0x1000 != 0 {
        half.wrapping_add(1)
    } else {
        half
    }
}

/// The 16-byte attribute record of the `.vgeo` (`helio_vg_format::ATTR_STRIDE`):
/// normal snorm8x4, tangent snorm8x4 (w = bitangent sign), uv0 f16x2, uv1 f16x2.
fn attribute_bytes(vertices: &[PackedVertex]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vertices.len() * 16);
    for v in vertices {
        let tangent = (v.tangent & 0x00ff_ffff) | (if v.bitangent_sign < 0.0 { 0x81u32 } else { 0x7fu32 } << 24);
        out.extend_from_slice(&v.normal.to_le_bytes());
        out.extend_from_slice(&tangent.to_le_bytes());
        for uv in [v.tex_coords0, v.tex_coords1] {
            out.extend_from_slice(&f32_to_f16_bits(uv[0]).to_le_bytes());
            out.extend_from_slice(&f32_to_f16_bits(uv[1]).to_le_bytes());
        }
    }
    out
}

/// Load or build the mesh's VG image on a worker thread and embed it in the
/// native `.mesh` when one exists.
fn spawn_build(
    content: u128,
    mesh: &StaticMeshComponent,
) -> mpsc::Receiver<Result<Arc<VgeoBytes>, String>> {
    let (tx, rx) = mpsc::channel();
    let native = native_mesh_path(&mesh.mesh_asset);
    let fallback = generated_cache_path(content);
    let vertices = mesh.vertices.clone();
    let indices = mesh.indices.clone();
    let sections: Vec<(u32, u32)> = mesh
        .mesh_sections
        .iter()
        .map(|s| (s.first_index, s.index_count))
        .collect();
    let spawned = std::thread::Builder::new()
        .name("vgeo-build".into())
        .spawn({
            let tx = tx.clone();
            move || {
                let _ = tx.send(build_or_load(
                    content,
                    native.as_deref(),
                    &fallback,
                    &vertices,
                    &indices,
                    &sections,
                ));
            }
        });
    if let Err(error) = spawned {
        let _ = tx.send(Err(format!("could not start the build thread: {error}")));
    }
    rx
}

fn build_or_load(
    content: u128,
    native_mesh: Option<&std::path::Path>,
    fallback: &std::path::Path,
    vertices: &[PackedVertex],
    indices: &[u32],
    sections: &[(u32, u32)],
) -> Result<Arc<VgeoBytes>, String> {
    if let Some(path) = native_mesh {
        if let Ok(bytes) = std::fs::read(path) {
            if let Some(embedded) = crate::mesh_cache::embedded_vgeo(&bytes) {
                if let Ok(loaded) = VgeoBytes::new(embedded.to_vec()) {
                    if loaded.header().content_id == content_words(content)
                        && helio_vg_build::is_current_cook(&loaded.header()) {
                        return Ok(Arc::new(loaded));
                    }
                }
            }
        }
    } else if let Ok(bytes) = std::fs::read(fallback) {
        if let Ok(loaded) = VgeoBytes::new(bytes) {
            if loaded.header().content_id == content_words(content)
                && helio_vg_build::is_current_cook(&loaded.header()) {
                return Ok(Arc::new(loaded));
            }
        }
    }
    let positions: Vec<[f32; 3]> = vertices.iter().map(|v| v.position).collect();
    let attributes = attribute_bytes(vertices);
    // One material section per `mesh_sections` entry: the scene join emits one
    // `static_objects` row per section in this order.
    let mut tri_material = Vec::new();
    if !sections.is_empty() {
        tri_material = vec![0u32; indices.len() / 3];
        for (section, &(first, count)) in sections.iter().enumerate() {
            let first_tri = (first / 3) as usize;
            let tris = (count / 3) as usize;
            for tri in tri_material.iter_mut().skip(first_tri).take(tris) {
                *tri = section as u32;
            }
        }
    }
    let mesh = helio_vg_build::MeshInput {
        positions: &positions,
        attributes: &attributes,
        attribute_stride: helio_vg_format::ATTR_STRIDE as usize,
        indices: &indices[..indices.len() / 3 * 3],
        tri_material: &tri_material,
    };
    let options = helio_vg_build::BuildOptions {
        content_id: content_words(content),
        resident_levels: 2,
    };
    let bytes = helio_vg_build::build_vgeo(&mesh, &options).map_err(|e| e.to_string())?;
    if let Some(path) = native_mesh {
        // Keep the original geometry/material payload intact and replace the
        // native file atomically. A read-only project remains usable this run.
        if let Err(error) = crate::mesh_cache::write_embedded_vgeo(path, content, &bytes) {
            tracing::warn!(
                "virtual geometry: could not embed data in {}: {error}",
                path.display()
            );
        }
    } else {
        if let Some(parent) = fallback.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(fallback, &bytes);
    }
    VgeoBytes::new(bytes).map(Arc::new)
}

/// Publishes `vg_asset_ready` from the runtime (sorted by interned vertex
/// offset, padded; see `virtual_geometry_instances`).
pub struct VgAssetReadyPublisher {
    runtime: VgRuntime,
    output: Output,
    last_version: Option<u64>,
}

impl VgAssetReadyPublisher {
    pub fn new(device: &wgpu::Device, runtime: VgRuntime) -> Self {
        Self {
            runtime,
            output: Output::new(device, "VG Asset Ready", VG_ASSET_READY_ROW_BYTES),
            last_version: None,
        }
    }
}

impl SceneDerivation for VgAssetReadyPublisher {
    fn name(&self) -> &'static str {
        "VgAssetReadyPublisher"
    }

    fn derive(
        &mut self,
        ctx: &SceneDerivationContext<'_>,
        _encoder: &mut wgpu::CommandEncoder,
    ) -> SceneDerivationOutput {
        if self.runtime.settings().mode != helio_vg_format::VgMode::Off {
            // Creates the store on first use and registers requested assets.
            self.runtime.service(ctx.device, ctx.queue);
        }
        let (rows, version) = self.runtime.asset_ready_rows(1);
        let grew = self.output.ensure(ctx.device, rows.len() as u32);
        if grew || self.last_version != Some(version) {
            self.last_version = Some(version);
            let mut padded: Vec<VgAssetReadyRow> = rows;
            padded.resize(self.output.rows as usize, VgAssetReadyRow::PADDING);
            ctx.queue.write_buffer(&self.output.buffer, 0, bytemuck::cast_slice(&padded));
            self.output.content_generation += 1;
        }
        SceneDerivationOutput {
            buffers: vec![(VG_ASSET_READY_KEY, self.output.handle())],
            flushed: Vec::new(),
            recorded: false,
        }
    }
}
