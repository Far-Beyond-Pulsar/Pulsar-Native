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

use std::path::{Path, PathBuf};

use bytemuck::Zeroable;
use helio::{MeshUpload, PackedVertex};

const MAGIC: &[u8; 4] = b"PMSH";
const VERSION: u32 = 1;
const HEADER: usize = 4 + 4 + 8 + 8; // magic + version + vertex_count + index_count

/// Native asset path for an imported source model: `<dest_dir>/<stem>.mesh`
/// (e.g. dropping `foo.fbx` into `dir` → `dir/foo.mesh`).
pub fn native_mesh_path(dest_dir: &Path, source: &Path) -> PathBuf {
    let stem = source.file_stem().and_then(|s| s.to_str()).unwrap_or("mesh");
    dest_dir.join(format!("{stem}.mesh"))
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
fn resolve_options(native: &Path, ext: &str) -> helio_asset_compat::OptionValues {
    if let Some(root) = engine_state::get_project_path() {
        let root = Path::new(&root);
        let key = engine_fs::import_options::asset_key(root, native);
        if let Some(json) = engine_fs::import_options::get(root, &key) {
            if let Ok(values) = serde_json::from_value(json) {
                return values;
            }
        }
    }
    helio_asset_compat::options_schema_for_extension(ext)
        .map(|s| s.default_values())
        .unwrap_or_default()
}

/// Import `source` into an engine-native `.mesh` asset at `native`, converting
/// with `values`. The source file is **not** copied into the project. Persists
/// the chosen options (keyed by the native path) for reimport. Returns the
/// written native path.
pub fn import_model_to_native(
    source: &Path,
    native: &Path,
    values: &helio_asset_compat::OptionValues,
) -> Result<PathBuf, String> {
    let scene = helio_asset_compat::load_scene_file_with_values(source, values)
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
    // Best-effort — never fail the import over this.
    if let Some(root) = engine_state::get_project_path() {
        let root = Path::new(&root);
        let key = engine_fs::import_options::asset_key(root, native);
        if let Ok(json) = serde_json::to_value(values) {
            let _ = engine_fs::import_options::set(root, &key, json);
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
