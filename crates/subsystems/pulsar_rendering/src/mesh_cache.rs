//! Baked mesh sidecar cache (issues #391 / #409).
//!
//! Model import happens **at copy time**: a source model (fbx/obj/gltf/usd) is
//! converted with the chosen import options and the resulting geometry is
//! written to a `<source>.mesh` sidecar. Components keep referencing the source
//! path; [`crate::subsystems::load_mesh_upload`] prefers the sidecar so it loads
//! the already-imported result without re-converting (and without needing the
//! import options at load time).

use std::path::{Path, PathBuf};

use bytemuck::Zeroable;
use helio::{MeshUpload, PackedVertex};

const MAGIC: &[u8; 4] = b"PMSH";
const VERSION: u32 = 1;
const HEADER: usize = 4 + 4 + 8 + 8; // magic + version + vertex_count + index_count

/// Sidecar path for a source model: `foo.fbx` → `foo.fbx.mesh`.
pub fn sidecar_path(source: &Path) -> PathBuf {
    let mut s = source.as_os_str().to_os_string();
    s.push(".mesh");
    PathBuf::from(s)
}

/// Whether `ext` (without leading dot) is a source model format we import/bake.
pub fn is_importable_model(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "fbx" | "obj" | "gltf" | "glb" | "usd" | "usda" | "usdc" | "usdz"
    )
}

/// Serialise a [`MeshUpload`] into the sidecar byte format (bytemuck-packed).
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

/// Parse a [`MeshUpload`] from sidecar bytes, or `None` if invalid / a version
/// or size mismatch (callers fall back to converting the source).
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
    // guaranteed to match `PackedVertex`, so `cast_slice` directly could panic.
    let mut vertices = vec![PackedVertex::zeroed(); vcount];
    bytemuck::cast_slice_mut(&mut vertices).copy_from_slice(&bytes[vstart..istart]);
    let mut indices = vec![0u32; icount];
    bytemuck::cast_slice_mut(&mut indices).copy_from_slice(&bytes[istart..end]);

    Some(MeshUpload { vertices, indices })
}

/// Import a source model at copy time: convert it with `values`, write the baked
/// `<source>.mesh` sidecar, and persist the chosen options for reimport (#409).
///
/// Called from the content-drawer drop flow. After this runs, components that
/// reference `source` load the baked sidecar with no per-load conversion.
pub fn import_mesh_asset(
    source: &Path,
    values: &helio_asset_compat::OptionValues,
) -> Result<(), String> {
    let scene = helio_asset_compat::load_scene_file_with_values(source, values)
        .map_err(|e| format!("import conversion failed: {e}"))?;

    if let Some(m) = scene.meshes.into_iter().next() {
        let upload = MeshUpload {
            vertices: m.vertices,
            indices: m.indices,
        };
        std::fs::write(sidecar_path(source), encode(&upload))
            .map_err(|e| format!("failed to write baked mesh sidecar: {e}"))?;
    }

    // Remember the chosen options so a later reimport can pre-fill the
    // configurator (issue #409). Best-effort — never fail the import over this.
    if let Some(root) = engine_state::get_project_path() {
        let root = Path::new(&root);
        let key = engine_fs::import_options::asset_key(root, source);
        if let Ok(json) = serde_json::to_value(values) {
            let _ = engine_fs::import_options::set(root, &key, json);
        }
    }

    Ok(())
}

/// Resolve import options for `source` — the options stored from a previous
/// import if this is a reimport, otherwise the format's schema defaults.
fn resolve_options(source: &Path, ext: &str) -> helio_asset_compat::OptionValues {
    if let Some(root) = engine_state::get_project_path() {
        let root = Path::new(&root);
        let key = engine_fs::import_options::asset_key(root, source);
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

/// Import `source` using its stored options (reimport) or schema defaults, and
/// write the baked sidecar. Convenience for the drop flow when no configurator
/// modal supplied explicit options.
pub fn import_mesh_asset_default(source: &Path) -> Result<(), String> {
    let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("");
    let values = resolve_options(source, ext);
    import_mesh_asset(source, &values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_path_appends_dot_mesh() {
        assert_eq!(
            sidecar_path(Path::new("/p/foo.fbx")),
            PathBuf::from("/p/foo.fbx.mesh")
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
