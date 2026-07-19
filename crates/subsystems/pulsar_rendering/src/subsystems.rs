use std::collections::HashMap;
use std::path::{Path, PathBuf};

use helio::{MaterialId, MeshId, MeshUpload};

/// Cache of GPU-uploaded mesh geometry, keyed by the resolved asset path.
///
/// Registered as a subsystem by both the game loader and editor contexts.
/// Components check this cache before loading and uploading mesh files.
pub struct MeshCache {
    pub upload_cache: HashMap<String, (MeshId, MaterialId)>,
}

impl MeshCache {
    pub fn new() -> Self {
        Self {
            upload_cache: HashMap::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<(MeshId, MaterialId)> {
        self.upload_cache.get(key).copied()
    }

    pub fn insert(&mut self, key: String, ids: (MeshId, MaterialId)) {
        self.upload_cache.insert(key, ids);
    }
}

/// Per-object-instance scene cache, keyed by scene-object ID.
///
/// Tracks which scene objects exist per component instance so that
/// the editor can update transforms in-place instead of deleting and
/// re-inserting every frame (which would cascade-free meshes/materials
/// in the helio scene).
pub struct SceneObjectCache {
    /// scene_object_id → (ObjectId, mesh_asset_path)
    pub map: HashMap<String, (helio::ObjectId, String)>,
}

impl SceneObjectCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn get(&self, scene_id: &str) -> Option<(helio::ObjectId, &str)> {
        self.map
            .get(scene_id)
            .map(|(id, path)| (*id, path.as_str()))
    }

    pub fn insert(&mut self, scene_id: String, obj_id: helio::ObjectId, mesh_asset: String) {
        self.map.insert(scene_id, (obj_id, mesh_asset));
    }

    pub fn remove(&mut self, scene_id: &str) -> Option<(helio::ObjectId, String)> {
        self.map.remove(scene_id)
    }
}

/// The engine's built-in assets — resolved at compile time so embedded
/// primitives (SM_Cube, SM_Sphere, etc.) are always available.
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
    let cfg = helio_asset_compat::LoadConfig {
        flip_uv_y: true,
        merge_meshes: false,
        import_scale: glam::Vec3::ONE,
    };

    // Try disk first.
    if path.exists() {
        return helio_asset_compat::load_scene_file_with_config(path, cfg)
            .ok()?
            .meshes
            .into_iter()
            .next()
            .map(|m| MeshUpload {
                vertices: m.vertices,
                indices: m.indices,
            });
    }

    // Fallback: check embedded primitives.
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if let Some(bytes) = embedded_primitive(stem) {
        return helio_asset_compat::load_scene_bytes_with_config(bytes, "fbx", None, cfg)
            .ok()?
            .meshes
            .into_iter()
            .next()
            .map(|m| MeshUpload {
                vertices: m.vertices,
                indices: m.indices,
            });
    }

    None
}
