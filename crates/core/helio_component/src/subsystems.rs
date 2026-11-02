use std::path::{Path, PathBuf};

use helio::MeshUpload;

/// The engine's built-in assets — resolved at compile time so embedded
/// primitives (SM_Cube, SM_Sphere, etc.) are always available.
///
/// Five levels up, not three: this crate lives at
/// `crates/renderer/helio/crates/helio-component` (moved here from
/// `crates/subsystems/pulsar_rendering`, renamed `helio_component` --
/// Pulsar-Native#561), two levels deeper than its old home.
const ENGINE_ASSETS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../assets");

macro_rules! prim_bytes {
    ($name:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../assets/meshes/primitives/",
            $name,
            ".fbx"
        ))
    };
}

fn embedded_primitive(stem: &str) -> Option<&'static [u8]> {
    match stem {
        "SM_Cube" => Some(prim_bytes!("SM_Cube")),
        "SM_Sphere" => Some(prim_bytes!("SM_Sphere")),
        "SM_Cylinder" => Some(prim_bytes!("SM_Cylinder")),
        "SM_Plane" => Some(prim_bytes!("SM_Plane")),
        "SM_Torus" => Some(prim_bytes!("SM_Torus")),
        _ => None,
    }
}

/// Resolve an asset path to an existing file on disk.
///
/// Checks (in order):
///  1. absolute path
///  2. project-root-relative
///  3. working-directory-relative
///  4. `cwd/assets/` (editor convention)
///  5. engine built-in assets (embedded primitives dir)
pub fn resolve_asset_path(project_root: &Path, asset: &str) -> PathBuf {
    let norm = asset.replace('\\', "/");
    let p = Path::new(&norm);

    if p.is_absolute() && p.exists() {
        return p.to_path_buf();
    }

    let proj = project_root.join(&norm);
    if proj.exists() {
        return proj;
    }

    if let Ok(cwd) = std::env::current_dir() {
        let cwd_path = cwd.join(&norm);
        if cwd_path.exists() {
            return cwd_path;
        }
        let cwd_assets = cwd.join("assets").join(&norm);
        if cwd_assets.exists() {
            return cwd_assets;
        }
    }

    let engine = Path::new(ENGINE_ASSETS_DIR).join(&norm);
    if engine.exists() {
        return engine;
    }

    proj
}

/// Load a mesh file from disk (or from embedded primitive bytes) into a
/// [`MeshUpload`] payload.
///
/// Components call this when they need to load geometry that hasn't been
/// cached yet.  The `path` should already be resolved to an absolute path
/// (use [`resolve_asset_path`] first if needed).
pub fn load_mesh_upload(path: &Path) -> Option<MeshUpload> {
    load_mesh_asset_upload(path).map(|asset| asset.geometry)
}

/// Load mesh geometry together with section/material-slot metadata. Direct
/// source formats are converted with section merging enabled, so assigning an
/// FBX path directly to `StaticMeshComponent` preserves its material groups.
pub fn load_mesh_asset_upload(path: &Path) -> Option<crate::mesh_cache::MeshAssetUpload> {
    // Engine-native baked mesh asset (`.mesh`), produced at import time from the
    // source model. Load it directly — no conversion or options (issues #391/#409).
    if path.extension().and_then(|e| e.to_str()) == Some("mesh") {
        let bytes = std::fs::read(path).ok()?;
        let (mut asset, id) = crate::mesh_cache::decode_asset(&bytes)?;

        // Content-id provenance (Pulsar-Native#658): prime the memoization
        // cache now, from the id `decode` just produced (read directly for
        // v2-v5, computed on the fly for v1) — so `MeshAssetPath::content_id`'s
        // later resolve, driven by SceneDB's GPU-mirror write dispatch, is
        // a warm hit instead of a second cold hash of this same file.
        crate::mesh_cache::prime_content_id_cache(path, id);

        // v1 backfill: a file with no header id gets upgraded to v5 in
        // place, atomically (write to a sibling temp file, then rename —
        // never a partial/torn write visible to a concurrent reader) so
        // the NEXT load reads the id straight from the header instead of
        // re-hashing. Best-effort: any failure here (read-only project
        // dir, concurrent access, etc.) is silently ignored -- the load
        // itself already succeeded with a correct in-memory id, and the
        // upgrade is a pure optimization, never load-bearing.
        if bytes.len() >= 8 && u32::from_le_bytes(bytes[4..8].try_into().unwrap_or_default()) == 1 {
            let upgraded = crate::mesh_cache::encode_asset(&asset, id);
            let tmp = path.with_extension("mesh.tmp");
            if std::fs::write(&tmp, &upgraded).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }

        // Meshes baked before UV fallback existed still show UV materials
        // (applied after the backfill so the file's stored bytes stay as baked).
        crate::mesh_cache::ensure_uv0(&mut asset.geometry);
        return Some(asset);
    }

    let cfg = helio_asset_compat::LoadConfig {
        flip_uv_y: true,
        merge_meshes: true,
        import_scale: glam::Vec3::ONE,
    };

    // Try disk first.
    if path.exists() {
        let scene = helio_asset_compat::load_scene_file_with_config(path, cfg).ok()?;
        return crate::mesh_cache::mesh_asset_from_converted_scene(scene);
    }

    // Fallback: check embedded primitives.
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if let Some(bytes) = embedded_primitive(stem) {
        let scene =
            helio_asset_compat::load_scene_bytes_with_config(bytes, "fbx", None, cfg).ok()?;
        return crate::mesh_cache::mesh_asset_from_converted_scene(scene);
    }

    None
}
