//! Cook-first import pipeline of the VG museum.
//!
//! Virtual-geometry assets (scans, trees) are never shown in a stand-in form.
//! The loader thread cooks every missing `.vgeo` first (memory-bounded, see
//! `vg_stress_cook`), and only then tells the window about the asset, which
//! creates its instances. Warm starts read the per-asset part record and the
//! `.vgeo` caches without parsing a source file. The only classic geometry in
//! the scene is the architecture kit and, until masked VG exists, a small
//! simplified stand-in of each alpha-masked foliage part.

use super::vg_stress_cook::{self as cook, JobCost};
use super::vg_stress_data::{
    layout::Placement,
    materials::Mat,
    textures::{self, GltfMaterial},
};
use super::vg_stress_prep::{catalog::Config, polyhaven};
use helio_mesh_api::{MeshUpload, PackedVertex};
use helio_asset_compat::{load_scene_file_with_config, ConvertedMesh, LoadConfig, MaterialSource};
use helio_vg_stream::{content_words, VgeoBytes};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    time::Instant,
};

/// Alpha-masked parts (twig / leaf cards, authored `FLAG_ALPHA_TEST | FLAG_DOUBLE_SIDED`)
/// are cooked and drawn through virtual geometry like every other part. Set
/// to `false` and they get no content id and are not shown (no classic path).
pub const MASKED_PARTS_USE_VG: bool = true;
/// A classic part: an architecture kit piece (one mesh).
pub struct PreparedPart {
    pub asset: String,
    pub meta: PartMeta,
    pub lods: Vec<MeshUpload>,
}

/// A virtual-geometry part: no classic geometry, only the cooked asset.
pub struct VgPart {
    pub meta: PartMeta,
    pub vgeo: Arc<VgeoBytes>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PartMeta {
    pub slot: String,
    pub transform: [f32; 16],
    pub radius: f32,
    /// Content id of the part's `.vgeo`; `None` for classic parts.
    pub content: Option<[u64; 2]>,
    pub source_triangles: u64,
    /// The part has an alpha-masked material (see [`MASKED_PARTS_USE_VG`]).
    #[serde(default)]
    pub masked: bool,
    /// `(vertices, indices)` of each stored classic mesh, LOD 0 first.
    lods: Vec<(usize, usize)>,
}

pub enum Event {
    /// A classic part: kit piece or masked foliage stand-in.
    Part(PreparedPart),
    /// Every VG part of an asset, all cooked: create its instances now.
    VgAsset {
        asset: String,
        parts: Vec<VgPart>,
    },
    /// One decoded texture (full mip chain) and every material channel it feeds.
    Texture {
        bindings: Vec<(Mat, u8)>,
        srgb: bool,
        width: u32,
        height: u32,
        mips: Vec<Vec<u8>>,
    },
    Status(String),
    /// Cook progress for the title bar; the loader prints its own console lines.
    Progress(String),
    Error(String),
    Finished,
}

pub struct Loader {
    pub events: Receiver<Event>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for Loader {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

/// `prepare_only` (headless `--prepare`): skip texture decoding, nothing
/// shows the result.
pub fn start(
    root: PathBuf,
    placements: &[Placement],
    vg_enabled: bool,
    config: Config,
    tau: f32,
    prepare_only: bool,
) -> Loader {
    let mut seen = HashSet::new();
    let assets: Vec<_> = placements
        .iter()
        .filter(|p| seen.insert(p.asset.clone()))
        .map(|p| (p.asset.clone(), p.virtual_geometry))
        .collect();
    let (tx, events) = mpsc::sync_channel(2);
    let cancelled = Arc::new(AtomicBool::new(false));
    let stop = cancelled.clone();
    std::thread::Builder::new()
        .name("museum-loader".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(
                    &root,
                    assets,
                    vg_enabled,
                    &config,
                    tau,
                    prepare_only,
                    &tx,
                    &stop,
                )
            }));
            if result.is_err() {
                let _ = tx.send(Event::Error(
                    "Asset loader panicked; loaded scene remains usable".into(),
                ));
            }
            let _ = tx.send(Event::Finished);
        })
        .expect("start museum asset worker");
    Loader { events, cancelled }
}

fn send(tx: &SyncSender<Event>, event: Event, stop: &AtomicBool) -> bool {
    !stop.load(Ordering::Relaxed) && tx.send(event).is_ok()
}

pub fn content_id(words: [u64; 2]) -> u128 {
    u128::from(words[0]) | (u128::from(words[1]) << 64)
}

/// Split `path#variant` asset keys (tree variants).
pub fn split_key(key: &str) -> (&str, Option<usize>) {
    match key.rsplit_once('#') {
        Some((path, v)) => match v.parse() {
            Ok(v) => (path, Some(v)),
            Err(_) => (key, None),
        },
        None => (key, None),
    }
}

/// Where the file of an asset key is: normally under the crate root, but a
/// tree provisioned at a lower resolution has a different glTF name.
fn resolve_path(root: &Path, key: &str) -> PathBuf {
    let (rel, _) = split_key(key);
    let direct = root.join(rel);
    if direct.exists() {
        return direct;
    }
    let assets = root.join("assets/vg_stress");
    rel.strip_prefix("assets/vg_stress/trees/")
        .and_then(|r| r.split('/').next())
        .and_then(|id| polyhaven::tree_gltf(&assets, id))
        .unwrap_or(direct)
}

fn short_name(asset: &str) -> String {
    let (path, variant) = split_key(asset);
    let stem = Path::new(path)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    match variant {
        Some(v) => format!("{stem}#{v}"),
        None => stem,
    }
}

// ── import ───────────────────────────────────────────────────────────────────

/// One imported source file, shared by every variant of it.
struct LoadedScene {
    meshes: Vec<ConvertedMesh>,
    sources: Vec<MaterialSource>,
    info: std::collections::HashMap<String, GltfMaterial>,
}

/// glTF `KHR_texture_transform` UV scale/offset is baked into the vertices.
fn load_scene(path: &Path) -> Result<LoadedScene, String> {
    let mut scene =
        load_scene_file_with_config(path, LoadConfig::default()).map_err(|e| e.to_string())?;
    let info = if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gltf"))
    {
        textures::read_gltf_materials(path)
    } else {
        Default::default()
    };
    let mut meshes = std::mem::take(&mut scene.meshes);
    for mesh in &mut meshes {
        let uv = mesh
            .material_index
            .and_then(|i| scene.material_sources.get(i))
            .and_then(|s| info.get(&s.name))
            .and_then(|m| m.uv);
        if let Some((scale, offset)) = uv {
            for v in &mut mesh.vertices {
                v.tex_coords0 = [
                    v.tex_coords0[0] * scale[0] + offset[0],
                    v.tex_coords0[1] * scale[1] + offset[1],
                ];
            }
        }
    }
    Ok(LoadedScene {
        meshes,
        sources: std::mem::take(&mut scene.material_sources),
        info,
    })
}

impl LoadedScene {
    fn slot(&self, mesh: &ConvertedMesh) -> String {
        mesh.material_index
            .and_then(|i| self.sources.get(i))
            .map(|s| s.name.clone())
            .unwrap_or_default()
    }

    fn masked(&self, mesh: &ConvertedMesh) -> bool {
        self.info.get(&self.slot(mesh)).is_some_and(|m| m.masked)
    }

    /// Meshes of `variant` (all of them for `None`).
    fn variant_meshes(&self, variant: Option<usize>) -> impl Iterator<Item = &ConvertedMesh> {
        self.meshes
            .iter()
            .filter(move |m| variant.is_none_or(|v| m.source_mesh == v))
    }
}

/// The parts of one asset (kit piece, scan, tree variant) with their scene
/// transform, material slot and, for VG parts, their content id. No geometry
/// is copied: the mesh is the importer's.
fn describe<'a>(
    scene: &'a LoadedScene,
    root: &Path,
    path: &Path,
    asset: &str,
    vg: bool,
) -> Result<Vec<(&'a ConvertedMesh, PartMeta)>, String> {
    let (_, variant) = split_key(asset);
    let mut min = glam::Vec3::splat(f32::INFINITY);
    let mut max = glam::Vec3::splat(f32::NEG_INFINITY);
    for mesh in scene.variant_meshes(variant) {
        for vertex in &mesh.vertices {
            let point = mesh
                .node_transform
                .transform_point3(glam::Vec3::from_array(vertex.position));
            min = min.min(point);
            max = max.max(point);
        }
    }
    let height = max.y - min.y;
    let expected_height = if vg {
        super::is_scan_asset(path).then_some(super::SCAN_SCENE_HEIGHT_METERS)
    } else {
        let manifest: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join("assets/vg_stress/kit/meshes/kit_manifest.json"))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let bounds = &manifest[stem.as_ref()]["bounds_blender"];
        bounds[5]
            .as_f64()
            .zip(bounds[2].as_f64())
            .map(|(hi, lo)| (hi - lo) as f32)
    };
    let scale = expected_height
        .filter(|h| *h > 1e-5 && height > 1e-5)
        .map(|expected| expected / height)
        .unwrap_or(1.0);
    log::info!("{asset}: imported height {height:.3}, scene scale {scale:.6}");
    let correction = glam::Mat4::from_scale(glam::Vec3::splat(scale));
    let mut parts = Vec::new();
    for mesh in scene.variant_meshes(variant) {
        if mesh.indices.len() < 3 || mesh.vertices.is_empty() {
            continue;
        }
        let masked = scene.masked(mesh);
        if vg && masked && !MASKED_PARTS_USE_VG {
            continue; // shown through nothing: there is no classic path for VG assets
        }
        // Masked parts get no VG content id while they render classically.
        let use_vg = vg && (MASKED_PARTS_USE_VG || !masked);
        let content =
            use_vg.then(|| content_words(cook::stable_content_id(&mesh.vertices, &mesh.indices)));
        let radius = mesh
            .vertices
            .iter()
            .map(|v| glam::Vec3::from_array(v.position).length())
            .fold(0.0f32, f32::max);
        let mut node = mesh.node_transform;
        if variant.is_some() {
            // The package lays its variants side by side for display; each
            // placed tree stands on its own origin.
            node.w_axis = glam::Vec4::new(0.0, 0.0, 0.0, 1.0);
        }
        parts.push((
            mesh,
            PartMeta {
                slot: scene.slot(mesh),
                transform: (correction * node).to_cols_array(),
                radius,
                content,
                source_triangles: (mesh.indices.len() / 3) as u64,
                masked,
                lods: Vec::new(),
            },
        ));
    }
    if parts.is_empty() {
        return Err("asset contains no triangles".into());
    }
    Ok(parts)
}

// ── per-asset part record ────────────────────────────────────────────────────
//
// Small, independent of the `.vgeo` caches: the scene transform, slot, radius
// and content id of each part, plus the classic meshes the asset keeps (the
// whole kit piece, or the masked stand-in LODs). VG parts store no geometry.

const RECORD_MAGIC: &[u8; 8] = b"VGPART06";

fn record_path(
    cache: &Path,
    path: &Path,
    variant: Option<usize>,
    vg: bool,
) -> Result<PathBuf, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let modified = meta
        .modified()
        .map_err(|e| e.to_string())?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let key = format!(
        "{}:{}:{}:{}:{}:{}:{}",
        path.display().to_string().replace('\\', "/"),
        meta.len(),
        modified,
        super::SCAN_SCENE_HEIGHT_METERS,
        MASKED_PARTS_USE_VG,
        vg,
        variant.map(|v| v as i64).unwrap_or(-1)
    );
    let hash = key.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    });
    Ok(cache.join(format!("parts-v6-{hash:016x}.bin")))
}

fn write_record(path: &Path, parts: &[PreparedPart]) -> Result<(), String> {
    let metadata = serde_json::to_vec(&parts.iter().map(|p| &p.meta).collect::<Vec<_>>())
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(RECORD_MAGIC);
    bytes.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&metadata);
    for part in parts {
        for lod in &part.lods {
            bytes.extend_from_slice(bytemuck::cast_slice(&lod.vertices));
            bytes.extend_from_slice(bytemuck::cast_slice(&lod.indices));
        }
    }
    cook::write_atomic(path, &bytes).map_err(|e| e.to_string())
}

fn read_record(path: &Path, asset: &str) -> Result<Vec<PreparedPart>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > 256 * 1024 * 1024 {
        return Err("oversize part record".into());
    }
    let mut bytes = Vec::new();
    std::io::BufReader::new(file)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    decode_record(&bytes, asset)
}

fn decode_record(bytes: &[u8], asset: &str) -> Result<Vec<PreparedPart>, String> {
    if bytes.get(..8) != Some(RECORD_MAGIC.as_slice()) {
        return Err("record header".into());
    }
    let n = u32::from_le_bytes(
        bytes
            .get(8..12)
            .ok_or("record metadata length")?
            .try_into()
            .unwrap(),
    ) as usize;
    let mut cursor = 12usize.checked_add(n).ok_or("record overflow")?;
    let metadata: Vec<PartMeta> =
        serde_json::from_slice(bytes.get(12..cursor).ok_or("record metadata")?)
            .map_err(|e| e.to_string())?;
    let mut parts = Vec::new();
    for meta in metadata {
        let mut lods = Vec::new();
        for &(vertex_count, index_count) in &meta.lods {
            let mut take = |n: usize, stride: usize| -> Result<&[u8], String> {
                let end = cursor
                    .checked_add(n.checked_mul(stride).ok_or("record overflow")?)
                    .ok_or("record overflow")?;
                let result = bytes.get(cursor..end).ok_or("record truncated")?;
                cursor = end;
                Ok(result)
            };
            let vertices: Vec<PackedVertex> = bytemuck::pod_collect_to_vec(take(
                vertex_count,
                std::mem::size_of::<PackedVertex>(),
            )?);
            let indices: Vec<u32> = bytemuck::pod_collect_to_vec(take(index_count, 4)?);
            if indices.len() % 3 != 0 || indices.iter().any(|&i| i as usize >= vertices.len()) {
                return Err("invalid record geometry".into());
            }
            lods.push(MeshUpload { vertices, indices });
        }
        if !meta.radius.is_finite() || meta.transform.iter().any(|f| !f.is_finite()) {
            return Err("invalid record transform".into());
        }
        parts.push(PreparedPart {
            asset: asset.to_string(),
            meta,
            lods,
        });
    }
    if cursor != bytes.len() || parts.is_empty() {
        return Err("record trailing data or empty".into());
    }
    Ok(parts)
}

// ── processing one asset ─────────────────────────────────────────────────────

/// Everything the window needs for one asset.
#[derive(Default)]
struct Processed {
    classic: Vec<PreparedPart>,
    vg: Vec<VgPart>,
}

/// Split a record into classic parts and VG parts, reading each part's
/// `.vgeo`; `None` if any cooked file is missing or stale (the asset is cold).
fn assemble(cache: &Path, record: Vec<PreparedPart>) -> Option<Processed> {
    let mut out = Processed::default();
    for part in record {
        match part.meta.content {
            Some(words) => {
                let vgeo = cook::read_vgeo(cache, content_id(words))?;
                out.vg.push(VgPart {
                    meta: part.meta,
                    vgeo,
                });
            }
            None if !part.lods.is_empty() => out.classic.push(part),
            None => return None,
        }
    }
    Some(out)
}

/// Warm start: the part record plus valid `.vgeo` caches, no source parsing.
fn warm_asset(root: &Path, cache: &Path, asset: &str, vg: bool) -> Option<Processed> {
    let (_, variant) = split_key(asset);
    let path = record_path(cache, &resolve_path(root, asset), variant, vg).ok()?;
    assemble(cache, read_record(&path, asset).ok()?)
}

/// Import-side work for one asset whose scene is loaded: kit pieces keep their
/// geometry, masked parts get their simplified stand-in, every other part of a
/// VG asset is cooked (or its valid cache read). Writes the part record last.
fn process_asset(
    cache: &Path,
    pool: Option<&rayon::ThreadPool>,
    scene: &LoadedScene,
    root: &Path,
    path: &Path,
    asset: &str,
    vg: bool,
) -> Result<Processed, String> {
    let (_, variant) = split_key(asset);
    let mut record = Vec::new();
    let mut cooked: Vec<Arc<VgeoBytes>> = Vec::new();
    for (mesh, mut meta) in describe(scene, root, path, asset, vg)? {
        let mut lods = Vec::new();
        if !vg {
            lods.push(MeshUpload {
                vertices: mesh.vertices.clone(),
                indices: mesh.indices.clone(),
            });
        } else if let Some(words) = meta.content {
            let content = content_id(words);
            let build =
                || cook::load_or_build_vgeo_cache(content, &mesh.vertices, &mesh.indices, cache);
            let (vgeo, _) = match pool {
                Some(pool) => pool.install(build),
                None => build(),
            }?;
            cooked.push(vgeo);
        }
        meta.lods = lods
            .iter()
            .map(|m| (m.vertices.len(), m.indices.len()))
            .collect();
        record.push(PreparedPart {
            asset: asset.to_string(),
            meta,
            lods,
        });
    }
    let record_file = record_path(cache, path, variant, vg)?;
    if let Err(error) = write_record(&record_file, &record) {
        log::warn!("Part record {}: {error}", record_file.display());
    }
    let mut cooked = cooked.into_iter();
    let mut out = Processed::default();
    for part in record {
        if part.meta.content.is_some() {
            let vgeo = cooked.next().ok_or("cook result missing")?;
            out.vg.push(VgPart {
                meta: part.meta,
                vgeo,
            });
        } else {
            out.classic.push(part);
        }
    }
    Ok(out)
}

/// Architecture kit piece: its record, or import and record it.
fn load_classic(root: &Path, cache: &Path, asset: &str) -> Result<Vec<PreparedPart>, String> {
    let (_, variant) = split_key(asset);
    let path = resolve_path(root, asset);
    let record = record_path(cache, &path, variant, false)?;
    if let Ok(parts) = read_record(&record, asset) {
        return Ok(parts);
    }
    let scene = load_scene(&path)?;
    Ok(process_asset(cache, None, &scene, root, &path, asset, false)?.classic)
}

// ── cost model ───────────────────────────────────────────────────────────────

/// `(triangles of the file, triangles of the asset)` before importing it.
fn triangle_estimate(root: &Path, key: &str) -> (u64, u64) {
    use super::vg_stress_data::layout::{DRAGON, HAPPY, LUCY, MANUSCRIPT, THAI, XYZ_DRAGON};
    use super::vg_stress_prep::catalog::TREES;
    let (rel, variant) = split_key(key);
    if let Some(tree) = TREES.iter().find(|t| rel.contains(t.id)) {
        let part = variant
            .and_then(|v| tree.variants.iter().find(|x| x.mesh == v))
            .map_or_else(|| tree.triangles(), |v| v.triangles);
        return (tree.triangles(), part);
    }
    let stem = Path::new(rel)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    if let Some(def) = [LUCY, THAI, XYZ_DRAGON, MANUSCRIPT, HAPPY, DRAGON]
        .iter()
        .find(|d| d.stem == stem)
    {
        return (def.triangles, def.triangles);
    }
    let bytes = std::fs::metadata(resolve_path(root, key))
        .map(|m| m.len())
        .unwrap_or(0);
    (bytes / 48, bytes / 48)
}

// ── cook phase ───────────────────────────────────────────────────────────────

struct Running {
    asset: String,
    triangles: u64,
    since: Instant,
}

struct Progress {
    started: Instant,
    total_assets: usize,
    done_assets: usize,
    total_triangles: u64,
    done_triangles: u64,
    running: Vec<Running>,
    last_print: Instant,
}

/// The title / console progress text.
fn progress_line(p: &Progress) -> String {
    let elapsed = p.started.elapsed().as_secs_f64();
    let eta = cook::eta_seconds(p.done_triangles, p.total_triangles, elapsed).map_or_else(
        || "estimating".to_string(),
        |s| format!("about {}", cook::format_duration(s)),
    );
    let now = p
        .running
        .iter()
        .map(|r| {
            format!(
                "{} ({} tris, {})",
                short_name(&r.asset),
                cook::millions(r.triangles),
                cook::format_duration(r.since.elapsed().as_secs_f64())
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Cooking VG {}/{} assets ({} of {} tris) | {} | elapsed {} | ETA {}",
        p.done_assets,
        p.total_assets,
        cook::millions(p.done_triangles),
        cook::millions(p.total_triangles),
        if now.is_empty() {
            "queued".to_string()
        } else {
            now
        },
        cook::format_duration(elapsed),
        eta
    )
}

struct Group {
    path: PathBuf,
    /// `(asset key, source triangles)`, in processing order.
    assets: Vec<(String, u64)>,
}

struct Ctx<'a> {
    root: &'a Path,
    cache: &'a Path,
    pool: &'a rayon::ThreadPool,
    tx: &'a SyncSender<Event>,
    stop: &'a AtomicBool,
    tau: f32,
    lod_target: Option<String>,
    progress: Mutex<Progress>,
}

impl Ctx<'_> {
    fn emit(&self, asset: &str, processed: Processed) -> bool {
        for part in processed.classic {
            if !send(self.tx, Event::Part(part), self.stop) {
                return false;
            }
        }
        if processed.vg.is_empty() {
            return true;
        }
        if self.lod_target.as_deref() == Some(asset) {
            for line in lod_report(self.cache, asset, &processed.vg, self.tau) {
                println!("{line}");
            }
        }
        send(
            self.tx,
            Event::VgAsset {
                asset: asset.to_string(),
                parts: processed.vg,
            },
            self.stop,
        )
    }

    fn begin(&self, asset: &str, triangles: u64) {
        let mut p = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        p.running.push(Running {
            asset: asset.to_string(),
            triangles,
            since: Instant::now(),
        });
        println!(
            "[Museum cook] start {} ({} tris)",
            short_name(asset),
            cook::millions(triangles)
        );
    }

    fn finish(&self, asset: &str, triangles: u64, ok: bool) {
        let mut p = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        let took = p
            .running
            .iter()
            .position(|r| r.asset == asset)
            .map(|i| p.running.remove(i).since.elapsed().as_secs_f64())
            .unwrap_or_default();
        p.done_assets += 1;
        p.done_triangles += triangles;
        println!(
            "[Museum cook] {} {} in {} | {}",
            if ok { "ready" } else { "FAILED" },
            short_name(asset),
            cook::format_duration(took),
            progress_line(&p)
        );
        p.last_print = Instant::now();
    }

    fn run_group(&self, group: Group) {
        if self.stop.load(Ordering::Relaxed) {
            return;
        }
        let scene = match load_scene(&group.path) {
            Ok(scene) => scene,
            Err(error) => {
                for (asset, triangles) in &group.assets {
                    self.begin(asset, *triangles);
                    self.finish(asset, *triangles, false);
                    send(
                        self.tx,
                        Event::Error(format!(
                            "VG {asset}: import failed: {error}; nothing is shown for it"
                        )),
                        self.stop,
                    );
                }
                return;
            }
        };
        for (asset, triangles) in &group.assets {
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            self.begin(asset, *triangles);
            let result = process_asset(
                self.cache,
                Some(self.pool),
                &scene,
                self.root,
                &group.path,
                asset,
                true,
            );
            self.finish(asset, *triangles, result.is_ok());
            match result {
                Ok(processed) => {
                    if !self.emit(asset, processed) {
                        return;
                    }
                }
                // No classic fallback of millions of triangles: show nothing.
                Err(error) => {
                    if !send(
                        self.tx,
                        Event::Error(format!(
                            "VG {asset}: cook failed: {error}; nothing is shown for it"
                        )),
                        self.stop,
                    ) {
                        return;
                    }
                }
            }
        }
    }

    fn heartbeat(&self) {
        let mut p = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        let line = progress_line(&p);
        let _ = self.tx.try_send(Event::Progress(line.clone()));
        if p.last_print.elapsed().as_secs() >= 15 {
            println!("[Museum cook] {line}");
            p.last_print = Instant::now();
        }
    }
}

/// CPU reference cut of a cooked asset at 20/50/100 m, printed at startup.
fn lod_report(cache: &Path, asset: &str, parts: &[VgPart], tau: f32) -> Vec<String> {
    use helio_pass_vg_cull::reference::{select_clusters_ex, RefAsset, RefInstance, RefView};
    use helio_vg_build::VgeoReader;
    use helio_vg_format::{
        cluster_slot_local, cluster_slot_page, GpuCluster, GpuVgInstance, INST_CONE_CULL,
        INST_ENABLED,
    };
    let distances = [20.0f32, 50.0, 100.0];
    let mut totals = [0u64; 3];
    let mut source = 0u64;
    for part in parts {
        let Some(words) = part.meta.content else {
            continue;
        };
        let Ok(bytes) = std::fs::read(cook::vgeo_path(cache, content_id(words))) else {
            continue;
        };
        let Ok(reader) = VgeoReader::parse(&bytes) else {
            continue;
        };
        let pages: Option<Vec<Vec<GpuCluster>>> = (0..reader.header.asset.page_count)
            .map(|p| {
                reader
                    .page(p)
                    .ok()
                    .and_then(|page| page.clusters().ok().map(|c| c.to_vec()))
            })
            .collect();
        let Some(pages) = pages else { continue };
        let slots: Vec<_> = (0..reader.header.asset.page_count).map(Some).collect();
        let lookup = |p: u32, l: u32| pages[p as usize][l as usize];
        let ref_asset = RefAsset {
            root_node: reader.header.asset.root_node,
            nodes: &reader.nodes,
            group_refs: &reader.group_refs,
            page_slot: &slots,
            cluster: &lookup,
        };
        let inst = RefInstance {
            matrix: part.meta.transform,
            inst: GpuVgInstance {
                asset_slot: 0,
                flags: INST_ENABLED | INST_CONE_CULL,
                lod_error_scale: 1.0,
                object_row: 0,
            },
        };
        let mut lo = glam::Vec3::splat(f32::INFINITY);
        let mut hi = glam::Vec3::splat(f32::NEG_INFINITY);
        for c in pages
            .iter()
            .flatten()
            .filter(|c| c.flags & GpuCluster::FLAG_LEAF != 0)
        {
            let center = glam::Vec3::new(c.sphere[0], c.sphere[1], c.sphere[2]);
            lo = lo.min(center - glam::Vec3::splat(c.sphere[3]));
            hi = hi.max(center + glam::Vec3::splat(c.sphere[3]));
        }
        let center =
            glam::Mat4::from_cols_array(&part.meta.transform).transform_point3((lo + hi) * 0.5);
        source += part.meta.source_triangles;
        for (i, distance) in distances.iter().enumerate() {
            let view = RefView::looking_down_neg_z(
                (center + glam::Vec3::Z * *distance).to_array(),
                std::f32::consts::FRAC_PI_4,
                0.1,
                720.0,
            );
            totals[i] += select_clusters_ex(&ref_asset, &view, tau, &inst, true)
                .drawn
                .iter()
                .map(|&s| {
                    u64::from(
                        lookup(cluster_slot_page(s), cluster_slot_local(s)).counts_material & 0xff,
                    )
                })
                .sum::<u64>();
        }
    }
    vec![format!(
        "[Museum LOD] {}: source {} tris; VG cut at 20 / 50 / 100 m = {} / {} / {} tris (CPU reference, tau {tau} px, 45 deg fov, 720p)",
        short_name(asset),
        source,
        totals[0],
        totals[1],
        totals[2]
    )]
}

#[allow(clippy::too_many_arguments)]
fn run(
    root: &Path,
    mut assets: Vec<(String, bool)>,
    vg_enabled: bool,
    config: &Config,
    tau: f32,
    prepare_only: bool,
    tx: &SyncSender<Event>,
    stop: &AtomicBool,
) {
    let cache = root.join("assets/vg_stress/cache");
    // Runtime fallback of the build-script provisioning: download and convert
    // the scans if they are missing. Status shows in the window title.
    // `HELIO_VG_STRESS_SKIP_PROVISION` skips it here as in `build.rs`: the
    // museum opens with what is on disk (offline, or a source unreachable).
    let assets_dir = root.join("assets/vg_stress");
    let skip = std::env::var_os("HELIO_VG_STRESS_SKIP_PROVISION").is_some();
    if !skip && !crate::vg_stress_prep::is_provisioned_with(&assets_dir, config) {
        let progress = |message: &str| {
            let _ = tx.send(Event::Status(format!("Preparing assets: {message}")));
        };
        if let Err(error) = crate::vg_stress_prep::provision_with(&assets_dir, config, &progress) {
            if !send(
                tx,
                Event::Error(format!("asset provisioning: {error}")),
                stop,
            ) {
                return;
            }
        }
    }
    cook::clean_stale_temp_files(&cache);
    // Architectural order (ground/shell first), then the smaller scans.
    assets.sort_by_key(|(asset, vg)| {
        (
            *vg,
            if *vg {
                std::fs::metadata(resolve_path(root, asset))
                    .map(|m| m.len())
                    .unwrap_or(u64::MAX)
            } else {
                0
            },
            asset.clone(),
        )
    });
    let (vg_assets, classic): (Vec<_>, Vec<_>) = assets.into_iter().partition(|(_, vg)| *vg);

    // 1. The architecture kit: the only always-classic geometry.
    if !prepare_only {
        for (asset, _) in &classic {
            if !send(
                tx,
                Event::Status(format!("Loading {}", short_name(asset))),
                stop,
            ) {
                return;
            }
            match load_classic(root, &cache, asset) {
                Ok(parts) => {
                    for part in parts {
                        if !send(tx, Event::Part(part), stop) {
                            return;
                        }
                    }
                }
                Err(error) => {
                    if !send(tx, Event::Error(format!("{asset}: {error}")), stop) {
                        return;
                    }
                }
            }
        }
    }

    // 2. Poly Haven marble and tree textures: decoded, mipped and capped to the
    // quality budget off the window thread; one bounded upload per event.
    // The instance benchmark (VG_STRESS_INSTANCES) skips the texture phase.
    if !prepare_only && std::env::var_os("VG_STRESS_INSTANCES").is_none() {
        let mats = Mat::all(config);
        let jobs = textures::jobs(&assets_dir, &mats, config);
        let texture_count = jobs.len();
        let _ = tx.send(Event::Status(format!("Decoding {} textures", jobs.len())));
        let texture_started = Instant::now();
        let texture_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .expect("texture decode pool");
        texture_pool.install(|| {
            jobs.into_par_iter().for_each(|job| {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                match textures::decode_with_opacity(
                    &job.path,
                    job.opacity.as_deref(),
                    job.role,
                    job.cap,
                ) {
                    Ok(d) => {
                        let _ = send(
                            tx,
                            Event::Texture {
                                bindings: job.bindings,
                                srgb: job.role == textures::Role::Srgb,
                                width: d.width,
                                height: d.height,
                                mips: d.mips,
                            },
                            stop,
                        );
                    }
                    // The material keeps its flat colour.
                    Err(error) => {
                        let _ = send(tx, Event::Error(format!("texture {error}")), stop);
                    }
                }
            });
        });
        eprintln!(
            "[Museum] decoded {texture_count} textures in {:.2}s (4 workers)",
            texture_started.elapsed().as_secs_f64()
        );
    }

    // 3. Virtual geometry, cook-first.
    if vg_assets.is_empty() {
        return;
    }
    if !vg_enabled {
        let _ = send(
            tx,
            Event::Error(
                "this device lacks the virtual-geometry passes: scans and trees are not shown \
                 (there is no classic stand-in for them)"
                    .into(),
            ),
            stop,
        );
        return;
    }
    let lod_target = vg_assets
        .iter()
        .map(|(a, _)| a)
        .find(|a| a.contains("pine_tree_01"))
        .or_else(|| vg_assets.first().map(|(a, _)| a))
        .cloned();
    let cpus = std::thread::available_parallelism().map_or(4, |n| n.get());
    let threads = cook::cook_threads(
        std::env::var("VG_STRESS_COOK_THREADS").ok().as_deref(),
        cpus,
    );
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("VG cook pool");
    let ctx = Ctx {
        root,
        cache: &cache,
        pool: &pool,
        tx,
        stop,
        tau,
        lod_target,
        progress: Mutex::new(Progress {
            started: Instant::now(),
            total_assets: 0,
            done_assets: 0,
            total_triangles: 0,
            done_triangles: 0,
            running: Vec::new(),
            last_print: Instant::now(),
        }),
    };

    // Warm assets first (smallest source file first): record + valid caches.
    let mut cold: BTreeMap<PathBuf, Vec<(String, u64)>> = BTreeMap::new();
    let mut warm = 0usize;
    for (asset, _) in &vg_assets {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match warm_asset(root, &cache, asset, true) {
            Some(processed) => {
                warm += 1;
                if !ctx.emit(asset, processed) {
                    return;
                }
            }
            None => cold
                .entry(resolve_path(root, asset))
                .or_default()
                .push((asset.clone(), triangle_estimate(root, asset).1)),
        }
    }
    println!(
        "[Museum] virtual geometry: {warm} of {} assets ready from cache, {} to cook",
        vg_assets.len(),
        vg_assets.len() - warm
    );
    if cold.is_empty() {
        return;
    }

    let available = cook::available_memory_bytes();
    let budget = cook::budget_bytes(
        std::env::var("VG_STRESS_COOK_MEM_GB").ok().as_deref(),
        available,
    );
    let workers = cook::job_workers(cpus);
    let mut work = Vec::new();
    let mut total_assets = 0;
    {
        let mut p = ctx.progress.lock().unwrap_or_else(|e| e.into_inner());
        for (path, mut group_assets) in cold {
            group_assets.sort_by_key(|(a, t)| (*t, a.clone()));
            let (file_tris, _) = triangle_estimate(root, &group_assets[0].0);
            let largest = group_assets.iter().map(|(_, t)| *t).max().unwrap_or(0);
            p.total_assets += group_assets.len();
            p.total_triangles += group_assets.iter().map(|(_, t)| *t).sum::<u64>();
            work.push((
                JobCost {
                    peak_bytes: cook::job_peak_bytes(file_tris, largest),
                    heavy: cook::is_heavy(file_tris),
                },
                Group {
                    path,
                    assets: group_assets,
                },
            ));
        }
        p.started = Instant::now();
        total_assets += p.total_assets;
    }
    println!(
        "[Museum] cook plan: {total_assets} assets in {} source files; memory budget {:.1} GiB (free now: {}; \
         VG_STRESS_COOK_MEM_GB overrides), {threads} cook threads, {workers} files at once, one 10M+ triangle file at a time",
        work.len(),
        budget as f64 / (1u64 << 30) as f64,
        available.map_or("unknown".to_string(), |a| format!(
            "{:.1} GiB",
            a as f64 / (1u64 << 30) as f64
        )),
    );
    let gate = cook::CookGate::new(budget);
    cook::run_jobs(
        work,
        &gate,
        workers,
        &|group| ctx.run_group(group),
        &mut || ctx.heartbeat(),
    );
    let p = ctx.progress.lock().unwrap_or_else(|e| e.into_inner());
    println!(
        "[Museum cook] finished: {}/{} assets in {}",
        p.done_assets,
        p.total_assets,
        cook::format_duration(p.started.elapsed().as_secs_f64())
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vg-loading-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn corrupt_record_is_a_cache_miss() {
        assert!(decode_record(b"VGPREV05", "mesh").is_err());
        let mut bytes = RECORD_MAGIC.to_vec();
        bytes.extend(u32::MAX.to_le_bytes());
        assert!(decode_record(&bytes, "mesh").is_err());
    }

    #[test]
    fn imported_column_matches_blender_bounds_and_record_round_trips() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let asset = "assets/vg_stress/kit/meshes/doric_column.fbx";
        let cache = scratch("column");
        let parts = load_classic(root, &cache, asset).unwrap();
        let path = record_path(&cache, &root.join(asset), None, false).unwrap();
        let loaded = read_record(&path, asset).unwrap();
        let mut lo = glam::Vec3::splat(f32::INFINITY);
        let mut hi = glam::Vec3::splat(f32::NEG_INFINITY);
        for (a, b) in parts.iter().zip(&loaded) {
            assert_eq!(a.lods.len(), 1, "a kit piece keeps one full mesh");
            assert_eq!(a.lods[0].indices, b.lods[0].indices);
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(&a.lods[0].vertices),
                bytemuck::cast_slice::<_, u8>(&b.lods[0].vertices)
            );
            assert_eq!(a.meta.transform, b.meta.transform);
            assert_eq!(a.meta.slot, "marble");
            assert!(a.meta.content.is_none());
            let transform = glam::Mat4::from_cols_array(&a.meta.transform);
            for v in &a.lods[0].vertices {
                let p = transform.transform_point3(glam::Vec3::from_array(v.position));
                lo = lo.min(p);
                hi = hi.max(p);
            }
        }
        let dimensions = hi - lo;
        assert!(
            (dimensions - glam::Vec3::new(1.34, 6.2, 1.34))
                .abs()
                .max_element()
                < 0.005,
            "{dimensions:?}"
        );
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.pop();
        assert!(decode_record(&bytes, asset).is_err());
        let _ = std::fs::remove_dir_all(cache);
    }

    #[test]
    fn museum_references_existing_assets_and_material_slots() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let layout = super::super::vg_stress_data::layout::museum_layout(
            &super::super::vg_stress_data::dims::Dims::load(),
        );
        assert!(layout.placements.len() > 400);
        assert!(layout.statues.len() >= 18);
        for placement in layout.placements {
            if placement.kit_piece().is_some() {
                assert!(root.join(&placement.asset).is_file(), "{}", placement.asset);
            } else if placement.asset.contains('#') {
                // Poly Haven trees are provisioned on demand.
                let (path, variant) = split_key(&placement.asset);
                assert!(
                    path.starts_with("assets/vg_stress/trees/") && path.ends_with(".gltf"),
                    "{path}"
                );
                assert!(variant.is_some());
            } else {
                // Scans are prepared on demand (vg_stress_prep), not committed.
                assert!(
                    crate::is_scan_asset(&root.join(&placement.asset))
                        && placement.asset.ends_with(".glb"),
                    "{}",
                    placement.asset
                );
            }
            if let Some(piece) = placement.kit_piece() {
                assert!(
                    !super::super::vg_stress_data::materials::kit_slots(piece).is_empty(),
                    "{piece}"
                );
            }
        }
    }

    /// Instancing audit of the default scene: prints what is shared. One
    /// asset key is one mesh (or one VG asset per part) however many
    /// placements use it; only an object row is made per placement.
    #[test]
    fn instancing_is_per_asset_not_per_placement() {
        use super::super::vg_stress_data::{dims::Dims, layout::museum_layout_with};
        let layout = museum_layout_with(&Dims::load(), &Config::default());
        let mut per_asset: BTreeMap<&str, (usize, bool)> = BTreeMap::new();
        for p in &layout.placements {
            let e = per_asset
                .entry(p.asset.as_str())
                .or_insert((0, p.virtual_geometry));
            e.0 += 1;
            assert_eq!(
                e.1, p.virtual_geometry,
                "{}: one asset must not be both VG and classic",
                p.asset
            );
        }
        let vg_assets = per_asset.values().filter(|(_, vg)| *vg).count();
        let vg_instances: usize = per_asset
            .values()
            .filter(|(_, vg)| *vg)
            .map(|(n, _)| n)
            .sum();
        let kit_assets = per_asset.len() - vg_assets;
        let kit_instances: usize = per_asset
            .values()
            .filter(|(_, vg)| !*vg)
            .map(|(n, _)| n)
            .sum();
        eprintln!(
            "[instancing] placements {}: kit {} distinct meshes / {} instances; VG {} distinct assets / {} instances",
            layout.placements.len(),
            kit_assets,
            kit_instances,
            vg_assets,
            vg_instances
        );
        assert!(vg_assets <= 16, "{vg_assets}");
        assert!(
            vg_instances > vg_assets * 5,
            "instances must far outnumber assets"
        );
        assert!(kit_instances >= kit_assets);
    }

    #[test]
    fn progress_line_reports_counts_running_and_eta() {
        let mut p = Progress {
            started: Instant::now(),
            total_assets: 4,
            done_assets: 1,
            total_triangles: 40_000_000,
            done_triangles: 10_000_000,
            running: vec![Running {
                asset: "assets/vg_stress/trees/pine_tree_01/p_2k.gltf#0".into(),
                triangles: 6_951_524,
                since: Instant::now(),
            }],
            last_print: Instant::now(),
        };
        let line = progress_line(&p);
        assert!(
            line.contains("1/4 assets") && line.contains("p_2k#0") && line.contains("6.95M tris"),
            "{line}"
        );
        assert!(line.contains("ETA"), "{line}");
        p.done_triangles = 0;
        assert!(progress_line(&p).contains("estimating"));
    }

    /// End to end on a small asset that is already on disk (never pine or
    /// Lucy): a cold cook into a scratch cache, then a warm start that must
    /// not need the source, and the invariants of the cook-first design.
    #[test]
    fn cook_first_end_to_end_on_a_small_tree() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let Some(gltf) = polyhaven::tree_gltf(&root.join("assets/vg_stress"), "island_tree_02")
        else {
            eprintln!("island_tree_02 is not provisioned; skipping");
            return;
        };
        let asset = "assets/vg_stress/trees/island_tree_02/island_tree_02_2k.gltf#0";
        let cache = scratch("island");
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap();
        assert!(
            warm_asset(root, &cache, asset, true).is_none(),
            "cold cache has nothing ready"
        );
        let scene = load_scene(&gltf).unwrap();
        let cold = process_asset(&cache, Some(&pool), &scene, root, &gltf, asset, true).unwrap();
        drop(scene);

        // No stand-in: VG parts carry no classic mesh at all.
        assert!(!cold.vg.is_empty());
        assert!(cold
            .vg
            .iter()
            .all(|p| p.meta.content.is_some() && p.meta.lods.is_empty()));
        assert!(
            cold.classic.is_empty(),
            "no classic part: masked leaves are VG too"
        );
        assert_eq!(
            cold.vg.iter().any(|p| p.meta.masked),
            MASKED_PARTS_USE_VG,
            "island_tree_02 leaves are cooked when masked VG is on"
        );
        // Atomic cache files exist, valid, and no temp file is left behind.
        for part in &cold.vg {
            assert!(cook::read_vgeo(&cache, content_id(part.meta.content.unwrap())).is_some());
        }
        assert!(std::fs::read_dir(&cache)
            .unwrap()
            .flatten()
            .all(|e| e.path().extension().is_none_or(|x| x != "tmp")));

        // Warm start: same parts straight from the caches, source not needed.
        if warm_asset(root, &cache, asset, true).is_none() {
            let rp = record_path(&cache, &resolve_path(root, asset), None, true).unwrap();
            eprintln!(
                "record {} -> {:?}",
                rp.display(),
                read_record(&rp, asset).err()
            );
            eprintln!("variant path {:?}", split_key(asset));
        }
        let warm = warm_asset(root, &cache, asset, true).expect("warm start");
        assert_eq!(warm.vg.len(), cold.vg.len());
        assert_eq!(warm.classic.len(), cold.classic.len());
        for (a, b) in cold.vg.iter().zip(&warm.vg) {
            assert_eq!(a.meta.content, b.meta.content);
            assert_eq!(a.meta.transform, b.meta.transform);
        }
        // A stale or missing cook makes the asset cold again (never half-ready).
        let victim = cook::vgeo_path(&cache, content_id(cold.vg[0].meta.content.unwrap()));
        std::fs::write(&victim, b"truncated").unwrap();
        assert!(warm_asset(root, &cache, asset, true).is_none());
        let _ = std::fs::remove_dir_all(cache);
    }

    /// Local scan fixtures are intentionally not downloaded or cooked by a
    /// test. Run explicitly after the demo (or `--prepare`) has cooked them.
    #[test]
    #[ignore = "requires the six local museum scan part records and VG caches"]
    fn cached_museum_scans_reduce_triangles_with_distance() {
        use helio_pass_vg_cull::reference::{
            select_clusters, select_clusters_ex, RefAsset, RefInstance, RefView,
        };
        use helio_vg_build::VgeoReader;
        use helio_vg_format::{
            cluster_slot_local, cluster_slot_page, GpuCluster, GpuVgInstance, INST_CONE_CULL,
            INST_ENABLED,
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let cache = root.join("assets/vg_stress/cache");
        let vgeo_cache = std::env::var_os("HELIO_VG_CANDIDATE_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|| cache.clone());
        for name in [
            "dragon",
            "happy_buddha",
            "manuscript",
            "xyzrgb_dragon",
            "thai_statue",
            "lucy",
        ] {
            if std::env::var("HELIO_VG_SCAN").is_ok_and(|scan| scan != name) {
                continue;
            }
            let asset = format!("assets/vg_stress/meshes/{name}.glb");
            let parts = warm_asset(root, &cache, &asset, true).unwrap_or_else(|| {
                panic!("{name}: not cooked; run `cargo vg-stress -- --prepare`")
            });
            for part in parts.vg {
                let content = content_id(part.meta.content.unwrap());
                let bytes = std::fs::read(vgeo_cache.join(format!("{content:032x}.vgeo")))
                    .expect("missing VG cache");
                let reader = VgeoReader::parse(&bytes).unwrap();
                assert!(
                    helio_vg_build::is_current_cook(&reader.header),
                    "{name}: stale cook, rebuild cache"
                );
                let pages: Vec<Vec<GpuCluster>> = (0..reader.header.asset.page_count)
                    .map(|p| reader.page(p).unwrap().clusters().unwrap().to_vec())
                    .collect();
                let slots: Vec<_> = (0..reader.header.asset.page_count).map(Some).collect();
                let lookup = |p: u32, l: u32| pages[p as usize][l as usize];
                let ref_asset = RefAsset {
                    root_node: reader.header.asset.root_node,
                    nodes: &reader.nodes,
                    group_refs: &reader.group_refs,
                    page_slot: &slots,
                    cluster: &lookup,
                };
                let mut lo = glam::Vec3::splat(f32::INFINITY);
                let mut hi = glam::Vec3::splat(f32::NEG_INFINITY);
                for c in pages
                    .iter()
                    .flatten()
                    .filter(|c| c.flags & GpuCluster::FLAG_LEAF != 0)
                {
                    let center = glam::Vec3::new(c.sphere[0], c.sphere[1], c.sphere[2]);
                    lo = lo.min(center - glam::Vec3::splat(c.sphere[3]));
                    hi = hi.max(center + glam::Vec3::splat(c.sphere[3]));
                }
                let center = glam::Mat4::from_cols_array(&part.meta.transform)
                    .transform_point3((lo + hi) * 0.5);
                let inst = RefInstance {
                    matrix: part.meta.transform,
                    inst: GpuVgInstance {
                        asset_slot: 0,
                        flags: INST_ENABLED | INST_CONE_CULL,
                        lod_error_scale: 1.0,
                        object_row: 0,
                    },
                };
                let roots: Vec<_> = pages
                    .iter()
                    .flatten()
                    .filter(|c| c.flags & GpuCluster::FLAG_ROOT != 0)
                    .collect();
                let root_tris: u64 = roots
                    .iter()
                    .map(|c| u64::from(c.counts_material & 0xff))
                    .sum();
                let tris = |distance: f32, cone: bool| -> u64 {
                    let view = RefView::looking_down_neg_z(
                        (center + glam::Vec3::Z * distance).to_array(),
                        std::f32::consts::FRAC_PI_4,
                        0.1,
                        720.0,
                    );
                    let selected = if cone {
                        select_clusters_ex(&ref_asset, &view, 1.0, &inst, true)
                    } else {
                        select_clusters(&ref_asset, &view, 1.0, &inst)
                    };
                    assert!(selected.requested_pages.is_empty());
                    selected
                        .drawn
                        .iter()
                        .map(|&s| {
                            u64::from(
                                lookup(cluster_slot_page(s), cluster_slot_local(s)).counts_material
                                    & 0xff,
                            )
                        })
                        .sum()
                };
                let distances = [3.0, 6.0, 12.0, 24.0, 48.0, 96.0];
                let counts: Vec<u64> = distances.iter().map(|&d| tris(d, false)).collect();
                let cone: Vec<u64> = distances.iter().map(|&d| tris(d, true)).collect();
                eprintln!(
                    "{name}: source={} coarsest={root_tris}; 3/6/12/24/48/96m at 720p, 1px: no-cone={counts:?}, cone={cone:?}",
                    part.meta.source_triangles
                );
                assert!(
                    counts.iter().all(|&c| c > 0),
                    "{name}: disappeared: {counts:?}"
                );
                assert!(
                    cone.iter().zip(&counts).all(|(c, n)| *c > 0 && c <= n),
                    "{name}: cone {cone:?} vs {counts:?}"
                );
                assert!(
                    counts.windows(2).all(|w| w[1] <= w[0]),
                    "{name}: {counts:?}"
                );
                assert!(
                    counts[5] * 4 < counts[0],
                    "{name}: insufficient distance reduction: {counts:?}"
                );
                assert_eq!(
                    tris(1.0e8, false),
                    root_tris,
                    "{name}: finite error must eventually select roots"
                );
            }
        }
    }

    #[test]
    #[ignore = "explicit offline cache cook of local scan sources"]
    fn cook_museum_scan_caches() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let cache = std::env::var_os("HELIO_VG_CANDIDATE_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("assets/vg_stress/cache"));
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        for name in [
            "dragon",
            "happy_buddha",
            "manuscript",
            "xyzrgb_dragon",
            "thai_statue",
            "lucy",
        ] {
            if std::env::var("HELIO_VG_SCAN").is_ok_and(|scan| scan != name) {
                continue;
            }
            eprintln!("Cooking {name}");
            let now = Instant::now();
            let path = root.join(format!("assets/vg_stress/meshes/{name}.glb"));
            let scene = load_scene(&path).unwrap();
            let asset = format!("assets/vg_stress/meshes/{name}.glb");
            process_asset(&cache, Some(&pool), &scene, root, &path, &asset, true).unwrap();
            eprintln!("Cooked {name} in {:.2}s", now.elapsed().as_secs_f64());
        }
    }
}
