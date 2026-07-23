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
//! NOTE: only mesh geometry is baked today. Materials/textures from the source
//! scene are not yet written as native assets — that's a follow-up once the
//! engine's native material-asset format is wired in here.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use bytemuck::Zeroable;
use helio::{MeshUpload, PackedVertex};
use pulsar_reflection::{RuntimeTypeInfo, TypeStructure, RUNTIME_TYPE_REGISTRY};

const MAGIC: &[u8; 4] = b"PMSH";
const VERSION: u32 = 1;
const HEADER: usize = 4 + 4 + 8 + 8; // magic + version + vertex_count + index_count

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
fn leak_static<T: 'static>(val: T) -> &'static T {
    Box::leak(Box::new(val))
}

fn build_enum_type_info(label: &str, choices: &[String]) -> &'static RuntimeTypeInfo {
    let variants: Vec<&'static str> = choices
        .iter()
        .map(|c| Box::leak(c.clone().into_boxed_str()) as &'static str)
        .collect();
    let variants: &'static [&'static str] = Box::leak(variants.into_boxed_slice());
    Box::leak(Box::new(RuntimeTypeInfo {
        type_id: TypeId::of::<u64>(),
        type_name: Box::leak(format!("enum:{label}").into_boxed_str()),
        size: 8,
        align: 8,
        structure: TypeStructure::Enum { variants },
        color: None,
    }))
}

fn convert_default(kind: &helio_asset_compat::OptionKind, dv: &helio_asset_compat::OptionValue) -> Box<dyn Any + Send> {
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
            RUNTIME_TYPE_REGISTRY.get::<bool>().expect("bool registered"),
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
            RUNTIME_TYPE_REGISTRY.get::<String>().expect("String registered"),
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
pub fn hashmap_from_option_values(values: &helio_asset_compat::OptionValues) -> HashMap<String, Box<dyn Any + Send>> {
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
pub fn option_values_from_hashmap(map: &HashMap<String, Box<dyn Any + Send>>) -> helio_asset_compat::OptionValues {
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

/// Native asset path for an imported source model: `<dest_dir>/<stem>.mesh`
/// (e.g. dropping `foo.fbx` into `dir` → `dir/foo.mesh`).
pub fn native_mesh_path(dest_dir: &Path, source: &Path) -> PathBuf {
    let stem = source.file_stem().and_then(|s| s.to_str()).unwrap_or("mesh");
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

/// Serialise a [`MeshUpload`] into the native `.mesh` byte format.
pub fn encode(mesh: &MeshUpload) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        HEADER
            + mesh.vertices.len() * std::mem::size_of::<PackedVertex>()
            + mesh.indices.len() * 4,
    );
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(mesh.vertices.len() as u64).to_le_bytes());
    out.extend_from_slice(&(mesh.indices.len() as u64).to_le_bytes());
    out.extend_from_slice(bytemuck::cast_slice(&mesh.vertices));
    out.extend_from_slice(bytemuck::cast_slice(&mesh.indices));
    out
}

/// Parse a [`MeshUpload`] from native `.mesh` bytes, or `None` if invalid / a
/// version or size mismatch (callers may fall back to converting a source).
pub fn decode(bytes: &[u8]) -> Option<MeshUpload> {
    if bytes.len() < HEADER || &bytes[0..4] != MAGIC {
        return None;
    }
    if u32::from_le_bytes(bytes[4..8].try_into().ok()?) != VERSION {
        return None;
    }
    let vcount = u64::from_le_bytes(bytes[8..16].try_into().ok()?) as usize;
    let icount = u64::from_le_bytes(bytes[16..24].try_into().ok()?) as usize;
    let vbytes = vcount.checked_mul(std::mem::size_of::<PackedVertex>())?;
    let ibytes = icount.checked_mul(4)?;
    let vstart = HEADER;
    let istart = vstart.checked_add(vbytes)?;
    let end = istart.checked_add(ibytes)?;
    if bytes.len() < end {
        return None;
    }

    // Copy into properly-aligned Vecs — the source byte slice alignment is not
    // guaranteed to match `PackedVertex`, so casting it directly could panic.
    let mut vertices = vec![PackedVertex::zeroed(); vcount];
    bytemuck::cast_slice_mut(&mut vertices).copy_from_slice(&bytes[vstart..istart]);
    let mut indices = vec![0u32; icount];
    bytemuck::cast_slice_mut(&mut indices).copy_from_slice(&bytes[istart..end]);

    Some(MeshUpload { vertices, indices })
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

/// Import `source` into an engine-native `.mesh` asset at `native`, converting
/// with `values`. The source file is **not** copied into the project. Persists
/// the chosen options (keyed by the native path) for reimport. Returns the
/// written native path.
pub fn import_model_to_native(
    source: &Path,
    native: &Path,
    values: &HashMap<String, Box<dyn Any + Send>>,
) -> Result<PathBuf, String> {
    let ov = option_values_from_hashmap(values);
    let scene = helio_asset_compat::load_scene_file_with_values(source, &ov)
        .map_err(|e| format!("import conversion failed: {e}"))?;

    let mesh = scene
        .meshes
        .into_iter()
        .next()
        .ok_or_else(|| "model contained no meshes".to_string())?;
    let upload = MeshUpload {
        vertices: mesh.vertices,
        indices: mesh.indices,
    };

    std::fs::write(native, encode(&upload))
        .map_err(|e| format!("failed to write native mesh {}: {e}", native.display()))?;

    // Persist chosen options for reimport / configurator pre-fill (#409).
    if let Some(root) = engine_state::get_project_path() {
        let root = Path::new(&root);
        let key = engine_fs::import_options::asset_key(root, native);
        let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("");
        if let Some(schema) = helio_asset_compat::options_schema_for_extension(ext) {
            let mut json_map = serde_json::Map::new();
            for field in &schema.fields {
                if let Some(val) = values.get(&field.key) {
                    if let Ok(json) = RUNTIME_TYPE_REGISTRY.serialize_json_for_any(val.as_ref()) {
                        json_map.insert(field.key.clone(), json);
                    }
                }
            }
            let _ = engine_fs::import_options::set(root, &key, serde_json::Value::Object(json_map));
        }
    }

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
        let bytes = encode(&mesh);
        let back = decode(&bytes).expect("decode");
        assert_eq!(back.vertices.len(), 3);
        assert_eq!(back.indices, vec![0, 1, 2]);
        // Truncated / garbage input is rejected, not panicked on.
        assert!(decode(&bytes[..10]).is_none());
        assert!(decode(b"nope").is_none());
    }
}
