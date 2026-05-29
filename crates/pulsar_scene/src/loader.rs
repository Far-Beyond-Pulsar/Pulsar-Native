//! Pulsar scene loader — canonical implementation shared by game runtime and editor engine.
//!
//! ## Design
//!
//! Component dispatch goes through the **inventory registration system**:
//!
//! 1. Each component crate (e.g. `pulsar_rendering`) submits a
//!    `RuntimeBehaviorRegistration` via `inventory::submit!` in its
//!    `#[derive(RegisterRuntimeBehavior)]` expansion.
//! 2. The loader creates a [`SceneObjectContext`] that implements
//!    [`ComponentRuntimeContext`] and owns all renderer state needed to
//!    materialise lights and meshes.
//! 3. `apply_runtime_behavior_for_class` iterates the inventory and calls the
//!    matching component's `sync_component` — which parses its own fields and
//!    calls `context.upsert_light` / `context.upsert_mesh`.
//!
//! The loader **never touches component field values**.  All parsing, defaults,
//! and unit conversions live inside the component's `sync_component`.  Adding a
//! new field to `LightComponent` automatically works here with zero loader edits.
//!
//! ## Linker note
//!
//! `pulsar_rendering` types are re-exported from `pulsar_scene::rendering` to
//! create a live code reference.  Without it the linker can silently drop
//! `pulsar_rendering`'s `#[used]` inventory statics.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::{EulerRot, Mat4, Quat, Vec3};
use helio::{
    GpuMaterial, GroupMask, MeshId, MaterialId, MeshUpload,
    Movability, ObjectDescriptor, Renderer, SceneActor,
};
use serde_json::Value;

use pulsar_reflection::{
    apply_runtime_behavior_for_class,
    ComponentRuntimeContext, RuntimeComponentOwner,
};

use crate::format::{SceneFile, SceneLoadError};

// ── Force pulsar_rendering into the binary ────────────────────────────────────
// Re-exporting these types creates a live symbol reference that prevents the
// linker from dropping pulsar_rendering's #[used] inventory statics.
// (ComponentRuntimeContext dispatch only works if those statics are linked in.)
pub use pulsar_rendering::LightComponent as _ForceLink_LightComponent;
pub use pulsar_rendering::StaticMeshComponent as _ForceLink_StaticMeshComponent;
pub use pulsar_rendering::ScriptComponent as _ForceLink_ScriptComponent;

// ── SceneLoader ───────────────────────────────────────────────────────────────

pub struct SceneLoader;

impl SceneLoader {
    pub fn load_file(
        path: &Path,
        project_root: &Path,
        renderer: &mut Renderer,
    ) -> Result<(), SceneLoadError> {
        let scene = SceneFile::load(path)?;
        Self::load_scene(&scene, project_root, renderer)
    }

    pub fn load_scene(
        scene: &SceneFile,
        project_root: &Path,
        renderer: &mut Renderer,
    ) -> Result<(), SceneLoadError> {
        Self::load_objects(&scene.objects, project_root, renderer);
        Ok(())
    }

    /// Core loader — dispatches every scene object through the component system.
    ///
    /// Each object gets a [`SceneObjectContext`] implementing
    /// [`ComponentRuntimeContext`].  `apply_runtime_behavior_for_class` calls the
    /// matching component's `sync_component`, which owns all parsing and renderer
    /// interaction.  The loader never touches component field values.
    ///
    /// V1 objects (no `__component_instances`) have synthetic component data
    /// constructed from their flat props and dispatched through the same path.
    pub fn load_objects(
        objects: &[crate::format::SceneObject],
        project_root: &Path,
        mut renderer: &mut Renderer,
    ) {
        tracing::info!(total = objects.len(), "Loading scene objects");

        // Shared per-load cache: avoid re-uploading the same mesh geometry for
        // every object that references it (e.g. many cubes using SM_Cube.fbx).
        let mut mesh_cache: HashMap<String, (MeshId, MaterialId)> = HashMap::new();

        for obj in objects {
            if !obj.visible { continue; }
            tracing::debug!(id = obj.id, name = obj.name, "Scene object");

            let owner = RuntimeComponentOwner {
                scene_object_id: &obj.id,
                position: obj.world_position(),
                rotation: obj.world_rotation(),
                scale:    obj.world_scale(),
                props:    &obj.props,
            };

            let instances = component_instances_from_props(&obj.props);
            {
                let mut ctx = SceneObjectContext {
                    obj_id: &obj.id,
                    project_root,
                    renderer,
                    mesh_cache: &mut mesh_cache,
                };
                for (idx, class_name, data) in &instances {
                    let handled = apply_runtime_behavior_for_class(class_name, &owner, *idx, data, &mut ctx);
                    if !handled {
                        tracing::debug!(class = class_name, id = obj.id, "No runtime behavior (skipped)");
                    }
                }
                renderer = ctx.renderer;
            }
        }
        tracing::info!(objects = objects.len(), "Scene loaded");
    }
}

// ── SceneObjectContext — ComponentRuntimeContext impl ─────────────────────────

struct SceneObjectContext<'r, 'p, 'c> {
    obj_id:       &'p str,
    project_root: &'p Path,
    renderer:     &'r mut Renderer,
    /// Geometry/material cache shared across all objects in one load pass.
    mesh_cache:   &'c mut HashMap<String, (MeshId, MaterialId)>,
}

impl ComponentRuntimeContext for SceneObjectContext<'_, '_, '_> {
    fn renderer_mut(&mut self) -> &mut Renderer { self.renderer }

    fn project_root(&self) -> &std::path::Path { self.project_root }

    fn load_mesh_file(&mut self, path: &std::path::Path) -> Option<MeshUpload> {
        let s = path.to_str().unwrap_or("");
        if s.is_empty() || s == "None" { return None; }
        let full = resolve_asset(self.project_root, s);
        load_fbx(full.as_path()).ok()
    }

    fn sync_mesh_object(
        &mut self,
        tag: u64,
        mesh_asset: &str,
        transform: glam::Mat4,
        bounds: [f32; 4],
    ) {
        if mesh_asset.is_empty() { return; }

        let (mesh_id, mat_id) = if let Some(&ids) = self.mesh_cache.get(mesh_asset) {
            ids
        } else {
            let path = resolve_asset(self.project_root, mesh_asset);
            let upload = match load_fbx(&path) {
                Ok(u)  => u,
                Err(e) => {
                    tracing::warn!(id = self.obj_id, "Mesh load failed for '{mesh_asset}': {e}");
                    return;
                }
            };
            let mid = match self.renderer.scene_mut()
                .insert_actor(SceneActor::mesh(upload))
                .as_mesh()
            {
                Some(m) => m,
                None    => return,
            };
            let mat = default_material();
            let matid = self.renderer.scene_mut().insert_material(mat);
            self.mesh_cache.insert(mesh_asset.to_string(), (mid, matid));
            (mid, matid)
        };

        self.renderer.scene_mut().insert_actor(SceneActor::object(ObjectDescriptor {
            mesh: mesh_id,
            material: mat_id,
            transform,
            bounds,
            flags: 0,
            groups: GroupMask::NONE,
            movability: Some(Movability::Movable),
            user_tag: tag,
        }));
    }

    fn report_error(&mut self, message: String) {
        tracing::warn!(id = self.obj_id, "{message}");
    }
}

fn default_material() -> GpuMaterial {
    GpuMaterial {
        base_color: [0.6, 0.6, 0.65, 1.0],
        emissive: [0.0, 0.0, 0.0, 0.0],
        roughness_metallic: [0.7, 0.0, 1.5, 0.5],
        tex_base_color: GpuMaterial::NO_TEXTURE,
        tex_normal: GpuMaterial::NO_TEXTURE,
        tex_roughness: GpuMaterial::NO_TEXTURE,
        tex_emissive: GpuMaterial::NO_TEXTURE,
        tex_occlusion: GpuMaterial::NO_TEXTURE,
        workflow: 0,
        flags: 0,
        _pad: 0,
    }
}

// ── Shared public API (called by engine_backend too) ─────────────────────────

/// Extract `(index, class_name, data)` from `__component_instances`.
/// Identical to engine's `component_instances_from_snap`.
pub fn component_instances_from_props(
    props: &HashMap<String, Value>,
) -> Vec<(usize, String, Value)> {
    let Some(arr) = props.get("__component_instances").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter().enumerate().filter_map(|(fi, entry)| {
        let o = entry.as_object()?;
        let idx = o.get("index").and_then(|v| v.as_u64()).map(|v| v as usize).unwrap_or(fi);
        let cls = o.get("class_name").and_then(|v| v.as_str()).map(str::to_string)?;
        let dat = o.get("data").cloned().unwrap_or(Value::Null);
        Some((idx, cls, dat))
    }).collect()
}

/// Build transform from position / rotation (degrees YXZ) / scale.
/// Identical to engine's `build_transform`.
pub fn build_transform_parts(position: [f32;3], rotation: [f32;3], scale: [f32;3]) -> Mat4 {
    let q = Quat::from_euler(EulerRot::YXZ,
        rotation[1].to_radians(), rotation[0].to_radians(), rotation[2].to_radians());
    Mat4::from_scale_rotation_translation(Vec3::from_array(scale), q, Vec3::from_array(position))
}


/// Load a mesh file via helio-asset-compat.
pub fn load_mesh_upload(path: &Path) -> Result<MeshUpload, String> {
    load_fbx(path)
}

/// The engine's built-in assets live next to pulsar_scene in the Pulsar-Native
/// source tree.  At *compile time* this is a known absolute path; at runtime
/// it allows game binaries to load engine primitives (SM_Cube.fbx, etc.) even
/// when those files haven't been copied into the game project yet.
const ENGINE_ASSETS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");

fn resolve_asset(project_root: &Path, asset: &str) -> PathBuf {
    let norm = asset.replace('\\', "/");
    let p = Path::new(&norm);

    // 1. Absolute path — check first.
    if p.is_absolute() && p.exists() { return p.to_path_buf(); }

    // 2. Relative to the game project root.
    let proj = project_root.join(&norm);
    if proj.exists() { return proj.clone(); }

    // 3. Relative to the working directory (matches engine's cwd/assets check).
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_path = cwd.join(&norm);
        if cwd_path.exists() { return cwd_path; }
        let cwd_assets = cwd.join("assets").join(&norm);
        if cwd_assets.exists() { return cwd_assets; }
    }

    // 4. Engine built-in assets — compiled-in path (dev builds only).
    let engine_path = Path::new(ENGINE_ASSETS_DIR).join(&norm);
    if engine_path.exists() {
        tracing::debug!(
            path = %engine_path.display(),
            "Mesh resolved from engine assets (copy to game project for release)"
        );
        return engine_path;
    }

    // Not found — return the project-relative path; caller will use fallback.
    proj
}

fn load_fbx(path: &Path) -> Result<MeshUpload, String> {
    let cfg = helio_asset_compat::LoadConfig {
        flip_uv_y: true, merge_meshes: false, import_scale: glam::Vec3::ONE,
    };
    helio_asset_compat::load_scene_file_with_config(path, cfg)
        .map_err(|e| format!("{}: {}", path.display(), e))?
        .meshes.into_iter().next()
        .map(|m| MeshUpload { vertices: m.vertices, indices: m.indices })
        .ok_or_else(|| format!("{}: no geometry", path.display()))
}

