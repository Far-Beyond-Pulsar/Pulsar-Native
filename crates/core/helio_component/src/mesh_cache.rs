//! Engine-native baked mesh assets (issues #391 / #409).
//!
//! Model import happens **at copy time**: a dropped source model
//! (fbx/obj/gltf/usd) is converted with the chosen import options into an
//! engine-native `.mesh` asset written into the project. **The source file
//! itself is not brought into the project** — only the native asset. Components
//! reference the `.mesh` asset and [`crate::subsystems::load_mesh_upload`] loads
//! it directly, with no per-load conversion or import options.
//!
//! Format (`PMSH`): a small header (magic + version + vertex/index counts)
//! followed by the bytemuck-packed [`PackedVertex`] and `u32` index arrays.
//!
//! The v3 format also stores mesh sections and imported scalar PBR material
//! values. Source texture payloads are still not copied into native assets.
//!
//! v4 adds an optional length-delimited VG chunk after the metadata. v5 adds
//! validated local-space bounds in the header before geometry; neither changes
//! the source-geometry content id or discards the classic mesh.
//!
//! # v2: content-id provenance (Pulsar-Native#632/#658)
//!
//! v2 appends a 16-byte `content_id: u128` to the header — the xxh3-128
//! hash of the decoded vertex+index bytes, computed once at import time
//! (see [`content_id_for_bytes`]) and stored so every later load reads it
//! directly instead of re-hashing. This is what
//! [`crate::components::static_mesh_component::MeshAssetPath::content_id`]
//! feeds into SceneDB's `ContentAddressed`/interned-var-len mechanism —
//! `helio-component` and `pulsar_scenedb` both stay opaque to what the id
//! MEANS; it's just a stable identity two components can compare.
//!
//! v1 files (no id in the header) decode fine — [`decode`] computes the
//! same hash on the fly and [`crate::subsystems::load_mesh_upload`]
//! opportunistically rewrites the file as the current format (v5) so the NEXT
//! load reads the content id and local bounds directly. A failed rewrite (read-only project
//! dir, etc.) never fails the load itself; the in-memory id from that one
//! call is still correct, just not persisted.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use bytemuck::Zeroable;
use helio_mesh_api::{MeshUpload, PackedVertex};
use pulsar_reflection::{RUNTIME_TYPE_REGISTRY, RuntimeTypeInfo, TypeStructure};

const MAGIC: &[u8; 4] = b"PMSH";
/// Current WRITE version — every fresh `encode` call produces this.
const VERSION: u32 = 2;
/// Version 3 adds JSON-encoded section/material-slot metadata after the
/// geometry payload. The geometry content id deliberately remains based on
/// vertex/index bytes so identical geometry can still share GPU allocations
/// even when its material assignment differs.
const VERSION_WITH_SECTIONS: u32 = 3;
/// Version 4 adds an optional, length-delimited baked virtual-geometry image
/// after the original geometry and metadata. Geometry identity remains based
/// only on the classic vertex/index bytes, so adding VG is non-destructive.
const VERSION_WITH_VGEO: u32 = 4;
/// Version 5 adds the authoritative local-space AABB after the v4 lengths.
const VERSION_WITH_BOUNDS: u32 = 5;
const HEADER: usize = 4 + 4 + 8 + 8; // magic + version + vertex_count + index_count
/// v2 only: `HEADER` bytes plus a trailing 16-byte `content_id: u128`,
/// BEFORE the vertex/index payload (so a v1 reader that somehow ignored the
/// version check would still fail the length check rather than misread
/// content-id bytes as geometry).
const HEADER_V2: usize = HEADER + 16;
const HEADER_V3: usize = HEADER_V2 + 8; // metadata byte length
const HEADER_V4: usize = HEADER_V3 + 8; // embedded vgeo byte length
const BOUNDS_BYTES: usize = 1 + 6 * 4; // empty flag, min xyz, max xyz
const HEADER_V5: usize = HEADER_V4 + BOUNDS_BYTES;

/// One imported material slot on a static mesh asset. `source_material` is
/// the material index assigned by the source format (for example FBX); the
/// authored component may override that slot with a project material asset.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MeshMaterialSlot {
    pub source_material: Option<u32>,
    pub name: String,
    #[serde(default)]
    pub surface: ImportedSurfaceMaterial,
    /// Default material assigned to this slot on the asset itself (a `.mat`
    /// file or shader-graph folder, project-relative). Placed
    /// `StaticMeshComponent`s inherit it for every slot they do not
    /// override; empty means "use the imported surface". Authored in the
    /// mesh viewer via [`set_default_materials`].
    #[serde(default)]
    pub material_asset: String,
}

/// FBX/OBJ/glTF scalar material data retained by native mesh imports: the
/// materials' surface values. Texture payloads remain separate assets; these
/// values provide a deterministic PBR surface when a slot has no authored
/// material assignment.
pub use helio_mats::asset::SurfaceValues as ImportedSurfaceMaterial;

/// An indexed section in the mesh's shared index buffer. Its `material_slot`
/// selects the matching entry in [`MeshAssetUpload::material_slots`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MeshSection {
    pub first_index: u32,
    pub index_count: u32,
    pub material_slot: u32,
}

/// Full static-mesh payload used by import, hydration, and rendering.
#[derive(Debug, Clone)]
pub struct MeshAssetUpload {
    pub geometry: MeshUpload,
    pub sections: Vec<MeshSection>,
    pub material_slots: Vec<MeshMaterialSlot>,
    /// Authoritative local-space bounds. Imports derive this from final geometry;
    /// legacy files derive it once during decode.
    pub bounds: MeshLocalBounds,
}

/// A validated local-space axis-aligned bound. Empty geometry is represented
/// explicitly and has no meaningful min/max values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshLocalBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub is_empty: bool,
}

impl MeshLocalBounds {
    pub const EMPTY: Self = Self {
        min: [0.0; 3],
        max: [0.0; 3],
        is_empty: true,
    };

    pub fn from_geometry(geometry: &MeshUpload) -> Option<Self> {
        let mut bounds = Self::EMPTY;
        for vertex in &geometry.vertices {
            let p = vertex.position;
            if !p.iter().all(|v| v.is_finite()) {
                return None;
            }
            if bounds.is_empty {
                bounds = Self {
                    min: p,
                    max: p,
                    is_empty: false,
                };
            } else {
                for axis in 0..3 {
                    bounds.min[axis] = bounds.min[axis].min(p[axis]);
                    bounds.max[axis] = bounds.max[axis].max(p[axis]);
                }
            }
        }
        Some(bounds)
    }

    pub fn is_valid(self) -> bool {
        (self.is_empty && self.min == [0.0; 3] && self.max == [0.0; 3])
            || (!self.is_empty
                && self
                    .min
                    .iter()
                    .chain(self.max.iter())
                    .all(|v| v.is_finite())
                && (0..3).all(|i| self.min[i] <= self.max[i]))
    }
}

impl Default for MeshAssetUpload {
    fn default() -> Self {
        Self {
            geometry: MeshUpload {
                vertices: Vec::new(),
                indices: Vec::new(),
            },
            sections: Vec::new(),
            material_slots: Vec::new(),
            bounds: MeshLocalBounds::EMPTY,
        }
    }
}

// ---------------------------------------------------------------------------
// Engine-native import-schema types (bridge from solid_rs::configurator).
// ---------------------------------------------------------------------------

/// UI constraints for an import field.
#[derive(Debug, Clone, Default)]
pub struct FieldConstraints {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
}

/// A single configurable import option — the engine-native equivalent of
/// [`solid_rs::configurator::OptionField`] but expressed in terms of the
/// reflection system so the gpui property editors can render it generically.
pub struct ImportField {
    pub key: String,
    pub label: String,
    pub doc: String,
    pub type_info: &'static RuntimeTypeInfo,
    pub default: Box<dyn Any + Send>,
    pub constraints: FieldConstraints,
}

impl fmt::Debug for ImportField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportField")
            .field("key", &self.key)
            .field("label", &self.label)
            .field("doc", &self.doc)
            .field("type_info", &self.type_info)
            .field("default", &"<dyn Any>")
            .field("constraints", &self.constraints)
            .finish()
    }
}

/// An ordered set of [`ImportField`]s describing a loader's import options.
#[derive(Debug)]
pub struct OptionsSchema {
    pub fields: Vec<ImportField>,
}

// ---------------------------------------------------------------------------
// Conversion from solid_rs schema types
// ---------------------------------------------------------------------------

/// Leak a value into a `&'static` reference (tiny allocation, modal lifetime).
#[allow(dead_code)]
fn leak_static<T: 'static>(val: T) -> &'static T {
    Box::leak(Box::new(val))
}

fn build_enum_type_info(label: &str, choices: &[String]) -> &'static RuntimeTypeInfo {
    static ENUM_TYPES: OnceLock<Mutex<HashMap<(String, Vec<String>), &'static RuntimeTypeInfo>>> =
        OnceLock::new();
    let key = (label.to_owned(), choices.to_vec());
    let cache = ENUM_TYPES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(existing) = cache.lock().expect("enum type cache poisoned").get(&key) {
        return existing;
    }
    let variants: Vec<&'static str> = choices
        .iter()
        .map(|c| Box::leak(c.clone().into_boxed_str()) as &'static str)
        .collect();
    let variants: &'static [&'static str] = Box::leak(variants.into_boxed_slice());
    let info = Box::leak(Box::new(RuntimeTypeInfo {
        type_id: TypeId::of::<u64>(),
        type_name: Box::leak(format!("enum:{label}").into_boxed_str()),
        size: 8,
        align: 8,
        structure: TypeStructure::Enum { variants },
        color: None,
    }));
    cache
        .lock()
        .expect("enum type cache poisoned")
        .insert(key, info);
    info
}

fn convert_default(
    _kind: &helio_asset_compat::OptionKind,
    dv: &helio_asset_compat::OptionValue,
) -> Box<dyn Any + Send> {
    use helio_asset_compat::OptionValue as OV;
    match dv {
        OV::Bool(b) => Box::new(*b),
        OV::Int(i) => Box::new(*i),
        OV::Float(f) => Box::new(*f),
        OV::Text(s) => Box::new(s.clone()),
        OV::Choice(s) => Box::new(s.clone()),
    }
}

fn convert_field(field: &helio_asset_compat::OptionField) -> ImportField {
    use helio_asset_compat::OptionKind as OK;

    let (type_info, constraints) = match &field.kind {
        OK::Bool => (
            RUNTIME_TYPE_REGISTRY
                .get::<bool>()
                .expect("bool registered"),
            FieldConstraints::default(),
        ),
        OK::Int { min, max, step } => (
            RUNTIME_TYPE_REGISTRY.get::<i64>().expect("i64 registered"),
            FieldConstraints {
                min: min.map(|v| v as f64),
                max: max.map(|v| v as f64),
                step: step.map(|v| v as f64),
            },
        ),
        OK::Float { min, max, step } => (
            RUNTIME_TYPE_REGISTRY.get::<f64>().expect("f64 registered"),
            FieldConstraints {
                min: *min,
                max: *max,
                step: *step,
            },
        ),
        OK::Enum { choices } => (
            build_enum_type_info(&field.label, choices),
            FieldConstraints::default(),
        ),
        OK::Text => (
            RUNTIME_TYPE_REGISTRY
                .get::<String>()
                .expect("String registered"),
            FieldConstraints::default(),
        ),
    };

    ImportField {
        key: field.key.clone(),
        label: field.label.clone(),
        doc: field.doc.clone(),
        type_info,
        default: convert_default(&field.kind, &field.default),
        constraints,
    }
}

/// Convert solid_rs configurator values to the engine's dynamic map.
pub fn hashmap_from_option_values(
    values: &helio_asset_compat::OptionValues,
) -> HashMap<String, Box<dyn Any + Send>> {
    use helio_asset_compat::OptionValue as OV;
    let mut map = HashMap::new();
    for (k, v) in values.0.iter() {
        let val: Box<dyn Any + Send> = match v {
            OV::Bool(b) => Box::new(*b),
            OV::Int(i) => Box::new(*i),
            OV::Float(f) => Box::new(*f),
            OV::Text(s) => Box::new(s.clone()),
            OV::Choice(s) => Box::new(s.clone()),
        };
        map.insert(k.clone(), val);
    }
    map
}

/// Convert the engine's dynamic value map back to solid_rs configurator values.
pub fn option_values_from_hashmap(
    map: &HashMap<String, Box<dyn Any + Send>>,
) -> helio_asset_compat::OptionValues {
    use helio_asset_compat::OptionValue as OV;
    let mut vals = helio_asset_compat::OptionValues::new();
    for (k, v) in map {
        let ov = if let Some(b) = v.downcast_ref::<bool>() {
            OV::Bool(*b)
        } else if let Some(i) = v.downcast_ref::<i64>() {
            OV::Int(*i)
        } else if let Some(f) = v.downcast_ref::<f64>() {
            OV::Float(*f)
        } else if let Some(s) = v.downcast_ref::<String>() {
            OV::Text(s.clone())
        } else if let Some(u) = v.downcast_ref::<u64>() {
            OV::Int(*u as i64)
        } else if let Some(i) = v.downcast_ref::<i32>() {
            OV::Int(*i as i64)
        } else if let Some(f) = v.downcast_ref::<f32>() {
            OV::Float(*f as f64)
        } else {
            continue;
        };
        vals.set(k, ov);
    }
    vals
}

/// Deserialize importer settings saved in an import database record.
pub fn option_values_from_json(
    json: &serde_json::Value,
) -> Option<HashMap<String, Box<dyn Any + Send>>> {
    serde_json::from_value::<helio_asset_compat::OptionValues>(json.clone())
        .ok()
        .map(|values| hashmap_from_option_values(&values))
}

/// Serialize importer settings for storage in an import database record.
pub fn option_values_to_json(map: &HashMap<String, Box<dyn Any + Send>>) -> serde_json::Value {
    let mut json = serde_json::Map::new();
    for (key, value) in map {
        if let Ok(value) = RUNTIME_TYPE_REGISTRY.serialize_json_for_any(value.as_ref()) {
            json.insert(key.clone(), value);
        }
    }
    serde_json::Value::Object(json)
}

/// Native asset path for an imported source model: `<dest_dir>/<stem>.mesh`
/// (e.g. dropping `foo.fbx` into `dir` → `dir/foo.mesh`).
pub fn native_mesh_path(dest_dir: &Path, source: &Path) -> PathBuf {
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("mesh");
    dest_dir.join(format!("{stem}.mesh"))
}

/// The import-options schema advertised for source extension `ext` (no leading
/// dot), for driving a configurator UI. `None` if the format isn't importable.
pub fn options_schema(ext: &str) -> Option<OptionsSchema> {
    let helio_schema = helio_asset_compat::options_schema_for_extension(ext)?;
    Some(OptionsSchema {
        fields: helio_schema.fields.iter().map(convert_field).collect(),
    })
}

/// Whether `ext` (without leading dot) is a source model format we import.
pub fn is_importable_model(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "fbx" | "obj" | "gltf" | "glb" | "usd" | "usda" | "usdc" | "usdz"
    )
}

/// Path → content id memoization, canonical-path-keyed so two different
/// spellings of the SAME file (`a.png` vs `A/../a.png`, a hardlink/second
/// copy resolving through a symlink, etc.) converge on one cache entry and
/// therefore one id (Pulsar-Native#661's path-aliasing test). Value is
/// `(mtime, len, id)`: a stale entry (file's current mtime/len disagree
/// with what's cached) is treated as a miss, so an edited file mints a new
/// id on its next resolve rather than serving a stale one forever.
static CONTENT_ID_CACHE: std::sync::OnceLock<
    std::sync::Mutex<HashMap<PathBuf, (std::time::SystemTime, u64, u128)>>,
> = std::sync::OnceLock::new();
const MAX_CONTENT_ID_CACHE_ENTRIES: usize = 4096;

fn content_id_cache()
-> &'static std::sync::Mutex<HashMap<PathBuf, (std::time::SystemTime, u64, u128)>> {
    CONTENT_ID_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Resolve a stable content id for the mesh asset at `abs_path` (an
/// already-resolved absolute path — see `subsystems::resolve_asset_path`).
///
/// - A native `.mesh` file: reads the id straight out of its v2 header (a
///   `HEADER_V2`-byte read, not a full geometry decode) when present; a v1
///   file with no stored id falls through to the memoization path below
///   (it'll be upgraded to v3 on the next load,
///   see that fn's doc, but this call itself doesn't write anything).
/// - Anything else (a v1 file with nothing to read yet, or a non-native
///   source path pre-import): canonicalizes `abs_path` (this is what makes
///   two spellings of the same file converge — symlinks and `..` segments
///   both resolve here) and checks the mtime/len-validated memoization
///   cache; on a miss/staleness, hashes the file's raw bytes once
///   (xxh3-128) and caches the result.
///
/// `None` only if `abs_path` can't be read/canonicalized at all (a
/// dangling reference) — mirrors `MeshAssetPath::content_id`'s own
/// `HandleId::ZERO`-on-failure convention at its call site.
pub fn content_id_for_path(abs_path: &Path) -> Option<u128> {
    if abs_path.extension().and_then(|e| e.to_str()) == Some("mesh") {
        if let Ok(bytes) = std::fs::read(abs_path) {
            if bytes.len() >= HEADER_V2 && &bytes[0..4] == MAGIC {
                if let Ok(version) = bytes[4..8].try_into().map(u32::from_le_bytes) {
                    if version == VERSION
                        || version == VERSION_WITH_SECTIONS
                        || version == VERSION_WITH_VGEO
                        || version == VERSION_WITH_BOUNDS
                    {
                        if let Ok(id_bytes) = bytes[HEADER..HEADER_V2].try_into() {
                            return Some(u128::from_le_bytes(id_bytes));
                        }
                    }
                }
            }
        }
    }

    let canonical = std::fs::canonicalize(abs_path).ok()?;
    let meta = std::fs::metadata(&canonical).ok()?;
    let mtime = meta.modified().ok()?;
    let len = meta.len();

    {
        let cache = content_id_cache()
            .lock()
            .expect("content id cache mutex poisoned");
        if let Some(&(cached_mtime, cached_len, id)) = cache.get(&canonical) {
            if cached_mtime == mtime && cached_len == len {
                return Some(id);
            }
        }
    }

    let bytes = std::fs::read(&canonical).ok()?;
    let id = twox_hash::XxHash3_128::oneshot(&bytes);
    let mut cache = content_id_cache()
        .lock()
        .expect("content id cache mutex poisoned");
    cache.insert(canonical, (mtime, len, id));
    while cache.len() > MAX_CONTENT_ID_CACHE_ENTRIES {
        if let Some(oldest) = cache.keys().next().cloned() {
            cache.remove(&oldest);
        } else {
            break;
        }
    }
    Some(id)
}

/// Primes the memoization cache directly from an id [`decode`] already
/// computed (or read) during a hydrate-time load — so the write-path's
/// later [`content_id_for_path`] call (from
/// `MeshAssetPath::content_id`, driven by SceneDB's derive-generated GPU
/// mirror dispatch) is a warm cache hit, not a second cold hash of the same
/// file. A no-op if `abs_path` can't be canonicalized or stat'd (matches
/// [`content_id_for_path`]'s own silent-`None` tolerance — priming is an
/// optimization, never load-bearing for correctness).
pub fn prime_content_id_cache(abs_path: &Path, id: u128) {
    let Ok(canonical) = std::fs::canonicalize(abs_path) else {
        return;
    };
    let Ok(meta) = std::fs::metadata(&canonical) else {
        return;
    };
    let Ok(mtime) = meta.modified() else { return };
    let mut cache = content_id_cache()
        .lock()
        .expect("content id cache mutex poisoned");
    cache.insert(canonical, (mtime, meta.len(), id));
    while cache.len() > MAX_CONTENT_ID_CACHE_ENTRIES {
        if let Some(oldest) = cache.keys().next().cloned() {
            cache.remove(&oldest);
        } else {
            break;
        }
    }
}

/// xxh3-128 over the SAME bytes [`encode`] writes for the geometry payload
/// (vertices then indices, both in their native `Pod` byte representation)
/// — the content identity for a mesh whose header doesn't already carry one
/// (v1 backfill) or that was never a native `.mesh` file at all (a source
/// format, hashed post-conversion). Two calls with byte-identical geometry
/// always produce the same id; this crate never claims anything stronger
/// (e.g. semantic equivalence of differently-tessellated meshes).
pub fn content_id_for_bytes(mesh: &MeshUpload) -> u128 {
    let mut buf = Vec::with_capacity(
        mesh.vertices.len() * std::mem::size_of::<PackedVertex>() + mesh.indices.len() * 4,
    );
    buf.extend_from_slice(bytemuck::cast_slice(&mesh.vertices));
    buf.extend_from_slice(bytemuck::cast_slice(&mesh.indices));
    twox_hash::XxHash3_128::oneshot(&buf)
}

fn write_bounds(out: &mut Vec<u8>, bounds: MeshLocalBounds) {
    out.push(u8::from(bounds.is_empty));
    for value in bounds.min.into_iter().chain(bounds.max) {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

fn read_bounds(bytes: &[u8]) -> Option<MeshLocalBounds> {
    if bytes.len() != BOUNDS_BYTES {
        return None;
    }
    let is_empty = match bytes[0] {
        0 => false,
        1 => true,
        _ => return None,
    };
    let mut values = [0.0f32; 6];
    for (i, chunk) in bytes[1..].chunks_exact(4).enumerate() {
        values[i] = f32::from_le_bytes(chunk.try_into().ok()?);
    }
    Some(MeshLocalBounds {
        min: [values[0], values[1], values[2]],
        max: [values[3], values[4], values[5]],
        is_empty,
    })
}

/// Serialise a [`MeshUpload`] into the native `.mesh` byte format (always
/// v2 — see the module doc). `content_id` is the identity to store in the
/// header; callers that don't already have one on hand (a fresh import)
/// should compute it via [`content_id_for_bytes`] first.
pub fn encode(mesh: &MeshUpload, content_id: u128) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        HEADER_V2
            + mesh.vertices.len() * std::mem::size_of::<PackedVertex>()
            + mesh.indices.len() * 4,
    );
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(mesh.vertices.len() as u64).to_le_bytes());
    out.extend_from_slice(&(mesh.indices.len() as u64).to_le_bytes());
    out.extend_from_slice(&content_id.to_le_bytes());
    out.extend_from_slice(bytemuck::cast_slice(&mesh.vertices));
    out.extend_from_slice(bytemuck::cast_slice(&mesh.indices));
    out
}

/// Serialize a full mesh asset with section and imported material-slot
/// metadata. Older `.mesh` files remain readable through [`decode_asset`].
pub fn encode_asset(asset: &MeshAssetUpload, content_id: u128) -> Vec<u8> {
    let metadata =
        serde_json::to_vec(&(asset.sections.as_slice(), asset.material_slots.as_slice()))
            .expect("mesh section metadata is serializable");
    let mesh = &asset.geometry;
    let geometry_is_empty = asset.geometry.vertices.is_empty();
    let bounds = if asset.bounds.is_valid() && asset.bounds.is_empty == geometry_is_empty {
        asset.bounds
    } else {
        MeshLocalBounds::from_geometry(&asset.geometry).unwrap_or(MeshLocalBounds::EMPTY)
    };
    let mut out = Vec::with_capacity(
        HEADER_V5
            + mesh.vertices.len() * std::mem::size_of::<PackedVertex>()
            + mesh.indices.len() * 4
            + metadata.len(),
    );
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION_WITH_BOUNDS.to_le_bytes());
    out.extend_from_slice(&(mesh.vertices.len() as u64).to_le_bytes());
    out.extend_from_slice(&(mesh.indices.len() as u64).to_le_bytes());
    out.extend_from_slice(&content_id.to_le_bytes());
    out.extend_from_slice(&(metadata.len() as u64).to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes()); // optional vgeo length, populated by vgeo writer
    write_bounds(&mut out, bounds);
    out.extend_from_slice(bytemuck::cast_slice(&mesh.vertices));
    out.extend_from_slice(bytemuck::cast_slice(&mesh.indices));
    out.extend_from_slice(&metadata);
    out
}

/// Serialize a mesh with an optional baked virtual-geometry payload. The
/// classic geometry, sections and material metadata remain equivalent to a
/// normal v5 mesh; the VG payload is a separate trailing
/// chunk and can be discarded/rebuilt without reimporting the source model.
pub fn encode_asset_with_vgeo(
    asset: &MeshAssetUpload,
    content_id: u128,
    vgeo: Option<&[u8]>,
) -> Vec<u8> {
    let mut out = encode_asset(asset, content_id);
    let vgeo = vgeo.unwrap_or_default();
    out[4..8].copy_from_slice(&VERSION_WITH_BOUNDS.to_le_bytes());
    out[HEADER_V3..HEADER_V4].copy_from_slice(&(vgeo.len() as u64).to_le_bytes());
    out.extend_from_slice(vgeo);
    out
}

/// Borrow the embedded VG chunk from a v4/v5 `.mesh`, if present and the
/// enclosing asset has a valid structure. Legacy mesh formats have no chunk.
pub fn embedded_vgeo(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() < HEADER_V4 || &bytes[0..4] != MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    if version != VERSION_WITH_VGEO && version != VERSION_WITH_BOUNDS {
        return None;
    }
    let vcount = usize::try_from(u64::from_le_bytes(bytes[8..16].try_into().ok()?)).ok()?;
    let icount = usize::try_from(u64::from_le_bytes(bytes[16..24].try_into().ok()?)).ok()?;
    let metadata_len = usize::try_from(u64::from_le_bytes(
        bytes[HEADER_V2..HEADER_V3].try_into().ok()?,
    ))
    .ok()?;
    let vgeo_len = usize::try_from(u64::from_le_bytes(
        bytes[HEADER_V3..HEADER_V4].try_into().ok()?,
    ))
    .ok()?;
    let vstart = if version == VERSION_WITH_BOUNDS {
        HEADER_V5
    } else {
        HEADER_V4
    };
    if version == VERSION_WITH_BOUNDS {
        let bounds = read_bounds(bytes.get(HEADER_V4..HEADER_V5)?)?;
        if !bounds.is_valid() || bounds.is_empty != (vcount == 0) {
            return None;
        }
    }
    let geometry_end = vstart
        .checked_add(vcount.checked_mul(std::mem::size_of::<PackedVertex>())?)?
        .checked_add(icount.checked_mul(4)?)?;
    let metadata_end = geometry_end.checked_add(metadata_len)?;
    let end = metadata_end.checked_add(vgeo_len)?;
    if end != bytes.len() {
        return None;
    }
    Some(&bytes[metadata_end..end])
}

/// Atomically attach a baked VG payload to an existing native mesh while
/// preserving its decoded classic geometry and authored material metadata.
/// The mesh content id remains the identity of the original vertex/index data.
pub fn write_embedded_vgeo(path: &Path, content_id: u128, vgeo: &[u8]) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let (asset, stored_id) = decode_asset(&bytes)
        .ok_or_else(|| format!("{} is not a valid native mesh", path.display()))?;
    if stored_id != content_id {
        return Err(format!(
            "{} changed while its VG data was being built",
            path.display()
        ));
    }
    let updated = encode_asset_with_vgeo(&asset, stored_id, Some(vgeo));
    let tmp = path.with_extension("mesh.vgeo.tmp");
    std::fs::write(&tmp, updated).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("replace {}: {e}", path.display())
    })?;
    prime_content_id_cache(path, stored_id);
    Ok(())
}

/// Parse a [`MeshUpload`] plus its content id from native `.mesh` bytes, or
/// `None` if invalid / an unsupported version / a size mismatch (callers
/// may fall back to converting a source). v1 files (no stored id) get one
/// computed on the fly via [`content_id_for_bytes`] — identical to what a
/// fresh v2 write of the same geometry would store, so a later backfill
/// write produces a byte-stable upgrade, not a new identity.
pub fn decode(bytes: &[u8]) -> Option<(MeshUpload, u128)> {
    let (asset, id) = decode_asset(bytes)?;
    Some((asset.geometry, id))
}

/// Decode legacy geometry-only assets (v1/v2), sectioned v3 assets, v4 assets
/// with an optional trailing VG chunk, and v5 assets with bounds. Legacy assets become a single
/// section and one unnamed material slot so downstream has one representation.
pub fn decode_asset(bytes: &[u8]) -> Option<(MeshAssetUpload, u128)> {
    if bytes.len() < HEADER || &bytes[0..4] != MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    if version != 1
        && version != VERSION
        && version != VERSION_WITH_SECTIONS
        && version != VERSION_WITH_VGEO
        && version != VERSION_WITH_BOUNDS
    {
        return None;
    }
    let vcount = usize::try_from(u64::from_le_bytes(bytes[8..16].try_into().ok()?)).ok()?;
    let icount = usize::try_from(u64::from_le_bytes(bytes[16..24].try_into().ok()?)).ok()?;
    let vbytes = vcount.checked_mul(std::mem::size_of::<PackedVertex>())?;
    let ibytes = icount.checked_mul(4)?;
    let (vstart, stored_id, metadata_len) = match version {
        1 => (HEADER, None, 0usize),
        VERSION => (
            HEADER_V2,
            Some(u128::from_le_bytes(
                bytes[HEADER..HEADER_V2].try_into().ok()?,
            )),
            0,
        ),
        VERSION_WITH_SECTIONS => {
            if bytes.len() < HEADER_V3 {
                return None;
            }
            (
                HEADER_V3,
                Some(u128::from_le_bytes(
                    bytes[HEADER..HEADER_V2].try_into().ok()?,
                )),
                usize::try_from(u64::from_le_bytes(
                    bytes[HEADER_V2..HEADER_V3].try_into().ok()?,
                ))
                .ok()?,
            )
        }
        VERSION_WITH_VGEO | VERSION_WITH_BOUNDS => {
            if bytes.len() < HEADER_V4 {
                return None;
            }
            (
                if version == VERSION_WITH_BOUNDS {
                    HEADER_V5
                } else {
                    HEADER_V4
                },
                Some(u128::from_le_bytes(
                    bytes[HEADER..HEADER_V2].try_into().ok()?,
                )),
                usize::try_from(u64::from_le_bytes(
                    bytes[HEADER_V2..HEADER_V3].try_into().ok()?,
                ))
                .ok()?,
            )
        }
        _ => return None,
    };
    let istart = vstart.checked_add(vbytes)?;
    let geometry_end = istart.checked_add(ibytes)?;
    let end = geometry_end.checked_add(metadata_len)?;
    let end = if version == VERSION_WITH_VGEO || version == VERSION_WITH_BOUNDS {
        end.checked_add(
            usize::try_from(u64::from_le_bytes(
                bytes[HEADER_V3..HEADER_V4].try_into().ok()?,
            ))
            .ok()?,
        )?
    } else {
        end
    };
    if bytes.len() != end {
        return None;
    }

    // Copy into properly-aligned Vecs — the source byte slice alignment is not
    // guaranteed to match `PackedVertex`, so casting it directly could panic.
    let mut vertices = vec![PackedVertex::zeroed(); vcount];
    bytemuck::cast_slice_mut(&mut vertices).copy_from_slice(&bytes[vstart..istart]);
    let mut indices = vec![0u32; icount];
    bytemuck::cast_slice_mut(&mut indices).copy_from_slice(&bytes[istart..geometry_end]);

    let geometry = MeshUpload { vertices, indices };
    let id = match stored_id {
        Some(id) => id,
        None => content_id_for_bytes(&geometry),
    };
    let metadata_end = geometry_end.checked_add(metadata_len)?;
    let (sections, material_slots) = if version == VERSION_WITH_SECTIONS
        || version == VERSION_WITH_VGEO
        || version == VERSION_WITH_BOUNDS
    {
        serde_json::from_slice(&bytes[geometry_end..metadata_end]).ok()?
    } else if geometry.indices.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        (
            vec![MeshSection {
                first_index: 0,
                index_count: geometry.indices.len() as u32,
                material_slot: 0,
            }],
            vec![MeshMaterialSlot::default()],
        )
    };
    let bounds = if version == VERSION_WITH_BOUNDS {
        let bounds = read_bounds(bytes.get(HEADER_V4..HEADER_V5)?)?;
        if !bounds.is_valid() || bounds.is_empty != geometry.vertices.is_empty() {
            return None;
        }
        bounds
    } else {
        MeshLocalBounds::from_geometry(&geometry)?
    };
    Some((
        MeshAssetUpload {
            geometry,
            sections,
            material_slots,
            bounds,
        },
        id,
    ))
}

/// Resolve import options for a native asset — options stored from a previous
/// import (keyed by the native path) if present, otherwise the source format's
/// schema defaults.
pub fn resolve_options(native: &Path, ext: &str) -> HashMap<String, Box<dyn Any + Send>> {
    if let Some(root) = engine_state::get_project_path() {
        let root = Path::new(&root);
        let key = engine_fs::import_options::asset_key(root, native);
        if let Some(json) = engine_fs::import_options::get(root, &key) {
            if let Ok(values) = serde_json::from_value::<helio_asset_compat::OptionValues>(json) {
                return hashmap_from_option_values(&values);
            }
        }
    }
    helio_asset_compat::options_schema_for_extension(ext)
        .map(|s| hashmap_from_option_values(&s.default_values()))
        .unwrap_or_default()
}

pub(crate) fn mesh_asset_from_converted_scene(
    mut scene: helio_asset_compat::ConvertedScene,
) -> Option<MeshAssetUpload> {
    let mut material_slots: Vec<_> = (0..scene.materials.len())
        .map(|index| material_slot(&scene.materials, index))
        .collect();

    let sectioned = scene.sectioned_mesh.take()?;
    sectioned_asset(sectioned, &mut material_slots)
}

/// The mesh material slot for source material `index`.
fn material_slot(
    materials: &[helio_asset_compat::ConvertedMaterial],
    index: usize,
) -> MeshMaterialSlot {
    MeshMaterialSlot {
        source_material: Some(index as u32),
        name: format!("Material {}", index + 1),
        material_asset: String::new(),
        surface: materials
            .get(index)
            .map(|material| ImportedSurfaceMaterial {
                base_color: material.gpu.base_color,
                roughness: material.gpu.roughness_metallic[0],
                metallic: material.gpu.roughness_metallic[1],
                emissive: [
                    material.gpu.emissive[0],
                    material.gpu.emissive[1],
                    material.gpu.emissive[2],
                ],
                emissive_intensity: material.gpu.emissive[3],
                alpha: material.gpu.base_color[3],
            })
            .unwrap_or_default(),
    }
}

/// One mesh of a scene that has several, as its own asset: only the materials
/// its sections use are slots, in order of first use. Returns the asset and
/// the source material index of each slot (`None` for a section with no
/// material).
pub(crate) fn mesh_asset_from_part(
    materials: &[helio_asset_compat::ConvertedMaterial],
    part: &helio_asset_compat::ConvertedSectionedMesh,
) -> Option<(MeshAssetUpload, Vec<Option<usize>>)> {
    let mut used: Vec<Option<usize>> = Vec::new();
    for section in &part.sections {
        if !used.contains(&section.material_index) {
            used.push(section.material_index);
        }
    }
    let mut slots: Vec<MeshMaterialSlot> = used
        .iter()
        .enumerate()
        .map(|(position, material)| match material {
            Some(index) => material_slot(materials, *index),
            None => MeshMaterialSlot {
                source_material: None,
                name: format!("Material {}", position + 1),
                surface: ImportedSurfaceMaterial::default(),
                material_asset: String::new(),
            },
        })
        .collect();
    let remapped = helio_asset_compat::ConvertedSectionedMesh {
        skin: part.skin,
        name: part.name.clone(),
        vertices: part.vertices.clone(),
        sections: part
            .sections
            .iter()
            .map(|section| helio_asset_compat::ConvertedMeshSection {
                indices: section.indices.clone(),
                material_index: used.iter().position(|m| *m == section.material_index),
            })
            .collect(),
    };
    let asset = sectioned_asset(remapped, &mut slots)?;
    Some((asset, used))
}

/// The asset for `sectioned`, whose sections' `material_index` index
/// `material_slots` (a section with none gets a new slot).
fn sectioned_asset(
    sectioned: helio_asset_compat::ConvertedSectionedMesh,
    material_slots: &mut Vec<MeshMaterialSlot>,
) -> Option<MeshAssetUpload> {
    let mut indices = Vec::new();
    let mut sections = Vec::with_capacity(sectioned.sections.len());
    for imported in sectioned.sections {
        let material_slot = imported.material_index.map_or_else(
            || {
                let index = material_slots.len() as u32;
                material_slots.push(MeshMaterialSlot {
                    source_material: None,
                    name: format!("Material {}", index + 1),
                    surface: ImportedSurfaceMaterial::default(),
                    material_asset: String::new(),
                });
                index
            },
            |index| index as u32,
        );
        let first_index = u32::try_from(indices.len()).ok()?;
        let index_count = u32::try_from(imported.indices.len()).ok()?;
        indices.extend(imported.indices);
        sections.push(MeshSection {
            first_index,
            index_count,
            material_slot,
        });
    }

    let mut geometry = MeshUpload {
        vertices: sectioned.vertices,
        indices,
    };
    ensure_uv0(&mut geometry);
    let bounds = MeshLocalBounds::from_geometry(&geometry)?;
    Some(MeshAssetUpload {
        geometry,
        sections,
        material_slots: std::mem::take(material_slots),
        bounds,
    })
}

fn unpack_snorm4x8(packed: u32) -> [f32; 3] {
    let channel = |shift: u32| ((packed >> shift) as u8 as i8 as f32 / 127.0).max(-1.0);
    [channel(0), channel(8), channel(16)]
}

/// Give a mesh that carries no UV layer a deterministic box projection.
///
/// Source models without UVs import with every `tex_coords0` at (0, 0), so
/// any UV-driven material (a shader graph's texture or `rainbow` node)
/// evaluates at a single point and paints the whole mesh one flat colour.
/// Each vertex projects onto the plane its normal faces most, in object-space
/// units, so such meshes still show their materials (tiling once per unit).
/// Meshes with real UVs are untouched. Idempotent.
pub fn ensure_uv0(mesh: &mut MeshUpload) {
    if mesh.vertices.is_empty() || mesh.vertices.iter().any(|v| v.tex_coords0 != [0.0, 0.0]) {
        return;
    }
    for vertex in &mut mesh.vertices {
        let n = unpack_snorm4x8(vertex.normal);
        let (ax, ay, az) = (n[0].abs(), n[1].abs(), n[2].abs());
        let p = vertex.position;
        vertex.tex_coords0 = if ax >= ay && ax >= az {
            [p[2], p[1]]
        } else if ay >= az {
            [p[0], p[2]]
        } else {
            [p[0], p[1]]
        };
    }
}

/// Rewrite the default material assignment of the native `.mesh` at `path`:
/// `materials[i]` is the project-relative material asset for slot `i` (empty
/// clears it). Geometry and content id are untouched, so placed meshes keep
/// their shared GPU geometry; a mesh `AssetUpdated` is published so open
/// levels re-resolve their slots.
pub fn set_default_materials(path: &Path, materials: &[String]) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let (mut asset, id) =
        decode_asset(&bytes).ok_or_else(|| format!("{} is not a valid .mesh", path.display()))?;
    for (slot, material) in asset.material_slots.iter_mut().zip(materials) {
        slot.material_asset = material.trim().to_owned();
    }
    let tmp = path.with_extension("mesh.tmp");
    let encoded = embedded_vgeo(&bytes)
        .filter(|vgeo| !vgeo.is_empty())
        .map(|vgeo| encode_asset_with_vgeo(&asset, id, Some(vgeo)))
        .unwrap_or_else(|| encode_asset(&asset, id));
    std::fs::write(&tmp, encoded)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    prime_content_id_cache(path, id);
    pulsar_events::publish_asset_updated(
        pulsar_events::AssetUpdated::new(plugin_editor_api::AssetKind::Mesh).with_path(path),
    );
    Ok(())
}

/// A reimport rewrites the `.mesh`; keep the default materials authored on
/// the previous version. Slots match by source material index, then by name.
fn carry_over_default_materials(native: &Path, upload: &mut MeshAssetUpload) {
    let Some((previous, _)) = std::fs::read(native).ok().and_then(|b| decode_asset(&b)) else {
        return;
    };
    for slot in &mut upload.material_slots {
        let old = previous.material_slots.iter().find(|old| {
            (slot.source_material.is_some() && old.source_material == slot.source_material)
                || (!slot.name.is_empty() && old.name == slot.name)
        });
        if let Some(old) = old {
            slot.material_asset = old.material_asset.clone();
        }
    }
}

/// Import `source` into an engine-native `.mesh` asset at `native`, converting
/// with `values`. The source file is **not** copied into the project. Persists
/// the chosen options (keyed by the native path) for reimport. Returns the
/// written native path.
pub fn import_model_to_native(
    source: &Path,
    native: &Path,
    values: &HashMap<String, Box<dyn Any + Send>>,
) -> Result<PathBuf, String> {
    write_static_mesh(load_model_scene(source, values)?, native)
}

/// Load `source` for import with `values`. A static mesh asset must retain
/// source material boundaries, so conversion is always the merged, sectioned
/// form; a rigged source also carries its [`helio_asset_compat::ImportedRig`].
pub fn load_model_scene(
    source: &Path,
    values: &HashMap<String, Box<dyn Any + Send>>,
) -> Result<helio_asset_compat::ConvertedScene, String> {
    let mut ov = option_values_from_hashmap(values);
    // The loader normally returns one mesh per primitive; merged conversion
    // bakes node transforms and emits a section table grouped by source
    // material.
    ov.set("merge_meshes", helio_asset_compat::OptionValue::Bool(true));
    helio_asset_compat::load_scene_file_with_values(source, &ov)
        .map_err(|e| format!("import conversion failed: {e}"))
}

/// Write the static geometry of a converted scene as the native `.mesh` at
/// `native` and announce it. Returns the written native path.
pub fn write_static_mesh(
    scene: helio_asset_compat::ConvertedScene,
    native: &Path,
) -> Result<PathBuf, String> {
    let upload = mesh_asset_from_converted_scene(scene)
        .ok_or_else(|| "model contained no sectioned mesh geometry".to_string())?;
    write_mesh_asset(upload, native)
}

/// What [`write_mesh_parts`] wrote for one mesh of the source.
#[derive(Debug)]
pub struct WrittenPart {
    /// The mesh's name in the source (`Name_2` for a second instance).
    pub name: String,
    pub path: PathBuf,
    /// The source material behind each material slot of the written mesh
    /// (`None` for a section the source gave no material).
    pub source_materials: Vec<Option<usize>>,
}

/// Write each mesh of a converted scene as its own native `.mesh`, named
/// `<stem>_<mesh name>.mesh` in `dir`. Node transforms are baked in, so the
/// parts placed together at the origin rebuild the source model.
pub fn write_mesh_parts(
    scene: &helio_asset_compat::ConvertedScene,
    dir: &Path,
    stem: &str,
) -> Result<Vec<WrittenPart>, String> {
    let mut written: Vec<WrittenPart> = Vec::new();
    for part in &scene.parts {
        let Some((upload, source_materials)) = mesh_asset_from_part(&scene.materials, part) else {
            continue;
        };
        let mut file = format!("{stem}_{}", sanitize_file_name(&part.name));
        // Distinct meshes can share a name; the file must not.
        let mut suffix = 1;
        while written
            .iter()
            .any(|w| w.path == dir.join(format!("{file}.mesh")))
        {
            suffix += 1;
            file = format!("{stem}_{}_{suffix}", sanitize_file_name(&part.name));
        }
        let path = write_mesh_asset(upload, &dir.join(format!("{file}.mesh")))?;
        written.push(WrittenPart {
            name: part.name.clone(),
            path,
            source_materials,
        });
    }
    if written.is_empty() {
        return Err("model contained no sectioned mesh geometry".to_string());
    }
    Ok(written)
}

/// Write one source part at a host-selected path, retaining its default materials.
pub fn write_mesh_part(
    scene: &helio_asset_compat::ConvertedScene,
    part: &helio_asset_compat::ConvertedSectionedMesh,
    path: &Path,
) -> Result<PathBuf, String> {
    let (upload, _) = mesh_asset_from_part(&scene.materials, part)
        .ok_or_else(|| "mesh part has no geometry".to_string())?;
    write_mesh_asset(upload, path)
}

fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "mesh".to_string()
    } else {
        cleaned
    }
}

fn write_mesh_asset(mut upload: MeshAssetUpload, native: &Path) -> Result<PathBuf, String> {
    carry_over_default_materials(native, &mut upload);
    let content_id = content_id_for_bytes(&upload.geometry);
    std::fs::write(native, encode_asset(&upload, content_id))
        .map_err(|e| format!("failed to write native mesh {}: {e}", native.display()))?;

    // Placed meshes naming this asset reload it (the level editor subscribes).
    pulsar_events::publish_asset_updated(
        pulsar_events::AssetUpdated::new(plugin_editor_api::AssetKind::Mesh).with_path(native),
    );
    Ok(native.to_path_buf())
}

/// Import `source` into `dest_dir` as a native `.mesh`, resolving options from
/// storage (reimport) or schema defaults. Convenience for the drop flow when no
/// configurator modal supplied explicit options. Returns the native path.
pub fn import_model_to_native_default(source: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    let native = native_mesh_path(dest_dir, source);
    let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("");
    let values = resolve_options(&native, ext);
    import_model_to_native(source, &native, &values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_path_uses_stem_and_dot_mesh() {
        assert_eq!(
            native_mesh_path(Path::new("/proj/models"), Path::new("/downloads/foo.fbx")),
            PathBuf::from("/proj/models/foo.mesh")
        );
    }

    #[test]
    fn encode_decode_roundtrip() {
        let mesh = MeshUpload {
            vertices: vec![PackedVertex::zeroed(); 3],
            indices: vec![0u32, 1, 2],
        };
        let id = content_id_for_bytes(&mesh);
        let bytes = encode(&mesh, id);
        let (back, decoded_id) = decode(&bytes).expect("decode");
        assert_eq!(back.vertices.len(), 3);
        assert_eq!(back.indices, vec![0, 1, 2]);
        assert_eq!(
            decoded_id, id,
            "v2 must round-trip the exact stored id, not recompute it"
        );
        // Truncated / garbage input is rejected, not panicked on.
        assert!(decode(&bytes[..10]).is_none());
        assert!(decode(b"nope").is_none());
    }

    #[test]
    fn v1_files_decode_with_a_computed_id_matching_a_fresh_v2_encode() {
        // Hand-roll a v1 header (no content_id field) around the same
        // geometry `encode` would write, to prove the backfill path
        // produces the IDENTICAL id a fresh v2 write of the same bytes
        // would -- the whole point of `content_id_for_bytes` being the
        // single source both `encode` (fresh import) and `decode`'s v1 arm
        // (backfill) call.
        let mesh = MeshUpload {
            vertices: vec![PackedVertex::zeroed(); 2],
            indices: vec![0u32, 1],
        };
        let mut v1 = Vec::new();
        v1.extend_from_slice(MAGIC);
        v1.extend_from_slice(&1u32.to_le_bytes());
        v1.extend_from_slice(&(mesh.vertices.len() as u64).to_le_bytes());
        v1.extend_from_slice(&(mesh.indices.len() as u64).to_le_bytes());
        v1.extend_from_slice(bytemuck::cast_slice(&mesh.vertices));
        v1.extend_from_slice(bytemuck::cast_slice(&mesh.indices));

        let (decoded, id) = decode(&v1).expect("v1 must still decode");
        assert_eq!(decoded.indices, mesh.indices);
        assert_eq!(id, content_id_for_bytes(&mesh));
        let (legacy_asset, _) = decode_asset(&v1).expect("v1 asset bounds are derived");
        assert_eq!(legacy_asset.bounds.min, [0.0; 3]);
        assert_eq!(legacy_asset.bounds.max, [0.0; 3]);
        assert!(!legacy_asset.bounds.is_empty);

        let v2 = encode(&mesh, id);
        let (_, id2) = decode(&v2).expect("v2 decode");
        assert_eq!(
            id2, id,
            "backfilled id matches a fresh v2 write of the same geometry"
        );
    }

    #[test]
    fn different_geometry_never_collides_in_practice() {
        let a = MeshUpload {
            vertices: vec![PackedVertex::zeroed(); 3],
            indices: vec![0, 1, 2],
        };
        let b = MeshUpload {
            vertices: vec![PackedVertex::zeroed(); 3],
            indices: vec![0, 2, 1],
        };
        assert_ne!(content_id_for_bytes(&a), content_id_for_bytes(&b));
    }

    #[test]
    fn an_import_announces_the_written_mesh() {
        let dir = std::env::temp_dir().join(format!("mesh-import-event-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../assets/meshes/primitives/SM_Cube.fbx");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = std::sync::Arc::clone(&seen);
        let _subscription = pulsar_events::subscribe_asset_updates(
            Some(plugin_editor_api::AssetKind::Mesh),
            move |event| record.lock().unwrap().push(event.path.clone()),
        );
        let native = import_model_to_native_default(&source, &dir).expect("the cube imports");
        assert!(
            seen.lock().unwrap().contains(&Some(native.clone())),
            "{native:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uv_less_meshes_get_a_box_projection_and_real_uvs_are_kept() {
        let vertex = |position: [f32; 3], normal: [f32; 3], uv: [f32; 2]| {
            PackedVertex::from_components(position, normal, uv, [1.0, 0.0, 0.0], 1.0)
        };
        let mut bare = MeshUpload {
            vertices: vec![
                vertex([1.0, 2.0, 3.0], [0.0, 1.0, 0.0], [0.0; 2]),
                vertex([4.0, 5.0, 6.0], [0.0, 0.0, 1.0], [0.0; 2]),
            ],
            indices: vec![0, 1, 0],
        };
        ensure_uv0(&mut bare);
        assert_eq!(bare.vertices[0].tex_coords0, [1.0, 3.0]);
        assert_eq!(bare.vertices[1].tex_coords0, [4.0, 5.0]);

        let mut authored = MeshUpload {
            vertices: vec![vertex([1.0, 2.0, 3.0], [0.0, 1.0, 0.0], [0.25, 0.5])],
            indices: vec![0],
        };
        ensure_uv0(&mut authored);
        assert_eq!(authored.vertices[0].tex_coords0, [0.25, 0.5]);
    }

    #[test]
    fn default_materials_rewrite_metadata_and_keep_geometry_identity() {
        let dir = std::env::temp_dir().join(format!("mesh-default-mat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.mesh");
        let asset = MeshAssetUpload {
            geometry: MeshUpload {
                vertices: vec![PackedVertex::zeroed(); 3],
                indices: vec![0, 1, 2],
            },
            sections: vec![MeshSection {
                first_index: 0,
                index_count: 3,
                material_slot: 0,
            }],
            material_slots: vec![MeshMaterialSlot::default()],
            bounds: MeshLocalBounds::EMPTY,
        };
        std::fs::write(&path, encode_asset(&asset, 7)).unwrap();
        set_default_materials(&path, &["materials/A.mat".to_owned()]).unwrap();
        let (decoded, id) = decode_asset(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(id, 7);
        assert_eq!(decoded.material_slots[0].material_asset, "materials/A.mat");
        assert_eq!(decoded.geometry.indices, vec![0, 1, 2]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn v5_round_trips_exact_bounds_and_vgeo() {
        let mesh = MeshUpload {
            vertices: vec![
                PackedVertex {
                    position: [-3.0, 2.0, 1.0],
                    ..PackedVertex::zeroed()
                },
                PackedVertex {
                    position: [4.0, -5.0, 8.0],
                    ..PackedVertex::zeroed()
                },
            ],
            indices: vec![0, 1],
        };
        let asset = MeshAssetUpload {
            bounds: MeshLocalBounds::from_geometry(&mesh).unwrap(),
            geometry: mesh,
            sections: vec![],
            material_slots: vec![],
        };
        let bytes = encode_asset_with_vgeo(&asset, 123, Some(b"vg"));
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            VERSION_WITH_BOUNDS
        );
        let (decoded, id) = decode_asset(&bytes).unwrap();
        assert_eq!(id, 123);
        assert_eq!(
            decoded.bounds,
            MeshLocalBounds {
                min: [-3.0, -5.0, 1.0],
                max: [4.0, 2.0, 8.0],
                is_empty: false
            }
        );
        assert_eq!(embedded_vgeo(&bytes), Some(b"vg".as_slice()));
    }

    #[test]
    fn v5_rejects_malformed_bounds_and_legacy_derives_bounds() {
        let mesh = MeshUpload {
            vertices: vec![PackedVertex {
                position: [1.0, 2.0, 3.0],
                ..PackedVertex::zeroed()
            }],
            indices: vec![],
        };
        let asset = MeshAssetUpload {
            bounds: MeshLocalBounds::from_geometry(&mesh).unwrap(),
            geometry: mesh,
            sections: vec![],
            material_slots: vec![],
        };
        let bytes = encode_asset(&asset, 9);
        let mut malformed = bytes.clone();
        malformed[HEADER_V4] = 2; // unknown empty flag
        assert!(decode_asset(&malformed).is_none());
        let mut contradictory_nonempty = bytes.clone();
        contradictory_nonempty[HEADER_V4] = 1;
        assert!(decode_asset(&contradictory_nonempty).is_none());
        assert!(embedded_vgeo(&contradictory_nonempty).is_none());

        let empty_asset = MeshAssetUpload {
            geometry: MeshUpload {
                vertices: vec![],
                indices: vec![],
            },
            sections: vec![],
            material_slots: vec![],
            bounds: MeshLocalBounds::EMPTY,
        };
        let mut contradictory_empty = encode_asset(&empty_asset, 10);
        contradictory_empty[HEADER_V4] = 0;
        assert!(decode_asset(&contradictory_empty).is_none());
        assert!(embedded_vgeo(&contradictory_empty).is_none());

        let mut legacy = encode(&asset.geometry, 9);
        legacy[4..8].copy_from_slice(&VERSION.to_le_bytes());
        // Geometry-only v2 header is 40 bytes; decode supplies transient bounds.
        let (decoded, _) = decode_asset(&legacy).unwrap();
        assert_eq!(decoded.bounds.min, [1.0, 2.0, 3.0]);
        assert_eq!(decoded.bounds.max, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn v3_and_v4_derive_bounds_and_v4_keeps_embedded_vgeo() {
        let geometry = MeshUpload {
            vertices: vec![
                PackedVertex {
                    position: [-2.0, 1.0, 3.0],
                    ..PackedVertex::zeroed()
                },
                PackedVertex {
                    position: [5.0, 7.0, -4.0],
                    ..PackedVertex::zeroed()
                },
            ],
            indices: vec![0, 1],
        };
        let asset = MeshAssetUpload {
            bounds: MeshLocalBounds::from_geometry(&geometry).unwrap(),
            geometry,
            sections: vec![MeshSection {
                first_index: 0,
                index_count: 2,
                material_slot: 0,
            }],
            material_slots: vec![MeshMaterialSlot {
                name: "legacy slot".into(),
                ..MeshMaterialSlot::default()
            }],
        };
        let v5 = encode_asset_with_vgeo(&asset, 17, Some(b"legacy-vgeo"));

        let mut v4 = v5.clone();
        v4.drain(HEADER_V4..HEADER_V5);
        v4[4..8].copy_from_slice(&VERSION_WITH_VGEO.to_le_bytes());
        let (decoded_v4, _) = decode_asset(&v4).unwrap();
        assert_eq!(decoded_v4.bounds, asset.bounds);
        assert_eq!(decoded_v4.material_slots, asset.material_slots);
        assert_eq!(embedded_vgeo(&v4), Some(b"legacy-vgeo".as_slice()));

        let mut v3 = encode_asset(&asset, 17);
        v3.drain(HEADER_V4..HEADER_V5);
        v3.drain(HEADER_V3..HEADER_V4);
        v3[4..8].copy_from_slice(&VERSION_WITH_SECTIONS.to_le_bytes());
        let (decoded_v3, _) = decode_asset(&v3).unwrap();
        assert_eq!(decoded_v3.bounds, asset.bounds);
        assert_eq!(decoded_v3.sections, asset.sections);
        assert_eq!(decoded_v3.material_slots, asset.material_slots);
        assert!(embedded_vgeo(&v3).is_none());
    }
}
