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
    GpuLight, GpuMaterial, LightId, LightType as HelioLightType,
    MaterialId, MeshId, MeshUpload, ObjectDescriptor, ObjectId, PackedVertex,
    Renderer, SceneActor,
};
use serde_json::Value;

use pulsar_reflection::{
    apply_runtime_behavior_for_class,
    ComponentRuntimeContext, RuntimeComponentOwner,
    RuntimeLightDesc, RuntimeLightType, RuntimeMeshDesc,
};

use crate::format::{LightType, MeshType, ObjectType, SceneFile, SceneLoadError};

// ── Force pulsar_rendering into the binary ────────────────────────────────────
// Re-exporting these types creates a live symbol reference that prevents the
// linker from dropping pulsar_rendering's #[used] inventory statics.
// (ComponentRuntimeContext dispatch only works if those statics are linked in.)
pub use pulsar_rendering::LightComponent as _ForceLink_LightComponent;
pub use pulsar_rendering::StaticMeshComponent as _ForceLink_StaticMeshComponent;

// ── Public result types ───────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct LoadedMesh {
    pub id: String,
    pub name: String,
    pub mesh_id: MeshId,
    pub object_id: ObjectId,
    pub material_id: MaterialId,
}

#[derive(Clone, Debug)]
pub struct LoadedLight {
    pub id: String,
    pub name: String,
    pub light_id: LightId,
}

#[derive(Default, Debug)]
pub struct LoadedScene {
    pub meshes: Vec<LoadedMesh>,
    pub lights: Vec<LoadedLight>,
}

// ── Engine-agnostic object view ───────────────────────────────────────────────

pub struct SceneObjectView<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub visible: bool,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
    pub props: &'a HashMap<String, Value>,
    pub fallback_mesh: Option<MeshType>,
    pub fallback_light: Option<LightType>,
}

// ── SceneLoader ───────────────────────────────────────────────────────────────

pub struct SceneLoader;

impl SceneLoader {
    pub fn load_file(
        path: &Path,
        project_root: &Path,
        renderer: &mut Renderer,
    ) -> Result<LoadedScene, SceneLoadError> {
        let scene = SceneFile::load(path)?;
        Self::load_scene(&scene, project_root, renderer)
    }

    pub fn load_scene(
        scene: &SceneFile,
        project_root: &Path,
        renderer: &mut Renderer,
    ) -> Result<LoadedScene, SceneLoadError> {
        let views: Vec<SceneObjectView> = scene.objects.iter().map(|obj| SceneObjectView {
            id:             &obj.id,
            name:           &obj.name,
            visible:        obj.visible,
            position:       obj.world_position(),
            rotation:       obj.world_rotation(),
            scale:          obj.world_scale(),
            props:          &obj.props,
            fallback_mesh:  match obj.object_type { ObjectType::Mesh(mt)  => Some(mt), _ => None },
            fallback_light: match obj.object_type { ObjectType::Light(lt) => Some(lt), _ => None },
        }).collect();
        Ok(Self::load_views(&views, project_root, renderer))
    }

    /// Core loader — walks every object, dispatches every component instance.
    ///
    /// For each object a [`SceneObjectContext`] is created that implements
    /// [`ComponentRuntimeContext`].  `apply_runtime_behavior_for_class` looks up
    /// the matching inventory registration (submitted by the component crate) and
    /// calls its `sync_component` — which parses its own fields and calls
    /// `context.upsert_light` / `context.upsert_mesh`.
    ///
    /// The loader never inspects component field values.
    pub fn load_views(
        objects: &[SceneObjectView<'_>],
        project_root: &Path,
        renderer: &mut Renderer,
    ) -> LoadedScene {
        let mut loaded = LoadedScene::default();
        tracing::info!(total = objects.len(), "Processing scene objects");

        for obj in objects {
            if !obj.visible { continue; }
            tracing::debug!(id = obj.id, name = obj.name, "Scene object");

            let instances = component_instances_from_props(obj.props);

            let owner = RuntimeComponentOwner {
                scene_object_id: obj.id,
                position: obj.position,
                rotation: obj.rotation,
                scale:    obj.scale,
                props:    obj.props,
            };

            let mut ctx = SceneObjectContext {
                obj_id:       obj.id,
                obj_name:     obj.name,
                position:     obj.position,
                rotation:     obj.rotation,
                scale:        obj.scale,
                project_root,
                renderer,
                lights: &mut loaded.lights,
                meshes: &mut loaded.meshes,
                had_light: false,
                had_mesh:  false,
            };

            for (idx, class_name, data) in &instances {
                let handled = apply_runtime_behavior_for_class(
                    class_name, &owner, *idx, data, &mut ctx,
                );
                if !handled {
                    tracing::debug!(
                        class = class_name, id = obj.id,
                        "No runtime behavior registered for component (skipped)"
                    );
                }
            }

            let had_light = ctx.had_light;
            let had_mesh  = ctx.had_mesh;
            // Reborrow renderer after ctx is dropped.
            let renderer  = ctx.renderer;

            // ── Legacy fallback (v1 scenes, no __component_instances) ─────
            if !had_light {
                if let Some(lt) = obj.fallback_light {
                    let color     = prop_f32_4(obj.props, "color",     [1.;4]);
                    let intensity = prop_f32  (obj.props, "intensity", 1.0);
                    let range     = prop_f32  (obj.props, "range",     10.0);
                    let desc = RuntimeLightDesc {
                        actor_key: format!("{}::light::0", obj.id),
                        light_type: match lt {
                            LightType::Directional => RuntimeLightType::Directional,
                            LightType::Point       => RuntimeLightType::Point,
                            LightType::Spot        => RuntimeLightType::Spot,
                        },
                        color, intensity, range,
                        inner_cone_angle_deg: prop_f32(obj.props, "inner_angle", 30.0),
                        outer_cone_angle_deg: prop_f32(obj.props, "outer_angle", 45.0),
                    };
                    let gpu = build_gpu_light(&desc, obj.position);
                    if let Some(light_id) = renderer.scene_mut()
                        .insert_actor(SceneActor::light(gpu)).as_light()
                    {
                        tracing::info!(id = obj.id, name = obj.name, "Light loaded (legacy)");
                        loaded.lights.push(LoadedLight {
                            id: desc.actor_key, name: obj.name.to_string(), light_id,
                        });
                    }
                }
            }
            if !had_mesh {
                if let Some(mt) = obj.fallback_mesh {
                    let asset = obj.props.get("mesh_asset")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty() && *s != "None");
                    let upload = match asset {
                        Some(p) => load_mesh_or_fallback(&resolve_asset(project_root, p)),
                        None    => build_primitive(mt),
                    };
                    if let Err(e) = spawn_mesh(
                        obj.id, obj.name, 0,
                        obj.position, obj.rotation, obj.scale,
                        upload, renderer, &mut loaded.meshes,
                    ) {
                        tracing::warn!(id = obj.id, "Mesh spawn failed (legacy): {e}");
                    }
                }
            }
        }

        tracing::info!(meshes = loaded.meshes.len(), lights = loaded.lights.len(), "Scene load complete");
        loaded
    }
}

// ── SceneObjectContext — ComponentRuntimeContext impl ─────────────────────────

/// Per-object context passed into each component's `sync_component`.
///
/// Components call `upsert_light` / `upsert_mesh`; the context translates
/// those into helio renderer calls.  The loader never reads component fields.
struct SceneObjectContext<'r, 'p> {
    obj_id:       &'p str,
    obj_name:     &'p str,
    position:     [f32; 3],
    rotation:     [f32; 3],
    scale:        [f32; 3],
    project_root: &'p Path,
    renderer:     &'r mut Renderer,
    lights:       &'p mut Vec<LoadedLight>,
    meshes:       &'p mut Vec<LoadedMesh>,
    had_light:    bool,
    had_mesh:     bool,
}

impl ComponentRuntimeContext for SceneObjectContext<'_, '_> {
    fn upsert_light(&mut self, actor_key: String, gpu: GpuLight) {
        self.had_light = true;
        // GpuLight is already fully constructed by the component — just insert.
        match self.renderer.scene_mut()
            .insert_actor(SceneActor::light(gpu)).as_light()
        {
            Some(light_id) => {
                tracing::info!(id = self.obj_id, name = self.obj_name, "Light loaded");
                self.lights.push(LoadedLight {
                    id: actor_key,
                    name: self.obj_name.to_string(),
                    light_id,
                });
            }
            None => tracing::warn!(id = self.obj_id, "non-light handle from insert_actor"),
        }
    }

    fn upsert_mesh(&mut self, desc: RuntimeMeshDesc) {
        self.had_mesh = true;
        let asset = desc.mesh_asset.trim();
        if asset.is_empty() || asset == "None" {
            tracing::warn!(id = self.obj_id, "StaticMeshComponent has no valid mesh_asset");
            return;
        }
        let full   = resolve_asset(self.project_root, asset);
        let upload = load_mesh_or_fallback(&full);
        if let Err(e) = spawn_mesh_with_key(
            &desc.actor_key, self.obj_name,
            self.position, self.rotation, self.scale,
            upload, self.renderer, self.meshes,
        ) {
            tracing::warn!(id = self.obj_id, "Mesh spawn failed: {e}");
        }
    }

    fn report_error(&mut self, message: String) {
        tracing::warn!(id = self.obj_id, "Component error: {message}");
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

/// Build a [`GpuLight`] from a v1 legacy [`RuntimeLightDesc`] and world position.
///
/// Only used by the backwards-compat fallback for scene files that have no
/// `__component_instances` (pre-v2 format).  For all current scenes,
/// `LightComponent::sync_component` builds the `GpuLight` directly.
fn build_gpu_light(desc: &RuntimeLightDesc, position: [f32; 3]) -> GpuLight {
    let lt = match desc.light_type {
        RuntimeLightType::Directional => HelioLightType::Directional,
        RuntimeLightType::Point       => HelioLightType::Point,
        RuntimeLightType::Spot        => HelioLightType::Spot,
        RuntimeLightType::Area        => HelioLightType::Point,
    };
    GpuLight {
        position_range:  [position[0], position[1], position[2], desc.range],
        direction_outer: [0.0, -1.0, 0.0, desc.outer_cone_angle_deg.to_radians()],
        color_intensity: [desc.color[0], desc.color[1], desc.color[2], desc.intensity],
        shadow_index:    u32::MAX,
        light_type:      lt as u32,
        inner_angle:     desc.inner_cone_angle_deg.to_radians(),
        _pad:            0,
    }
}

/// Load a mesh file via helio-asset-compat.
/// Identical to engine's `load_fbx_mesh`.
pub fn load_mesh_upload(path: &Path) -> Result<MeshUpload, String> {
    load_fbx(path)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Spawn a mesh+object into the renderer using a pre-formatted actor key.
///
/// Used by [`SceneObjectContext::upsert_mesh`]; the key is already formatted by
/// `StaticMeshComponent::sync_component` as `"<obj_id>::mesh::<idx>"`.
fn spawn_mesh_with_key(
    actor_key: &str,
    obj_name: &str,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
    upload: MeshUpload,
    renderer: &mut Renderer,
    out: &mut Vec<LoadedMesh>,
) -> Result<(), String> {
    let mesh_id = renderer.scene_mut()
        .insert_actor(SceneActor::mesh(upload))
        .as_mesh()
        .ok_or("non-mesh handle")?;

    let mat_id = renderer.scene_mut()
        .insert_material(make_material([0.6, 0.6, 0.65, 1.0], 0.7, 0.0));

    let transform = build_transform_parts(position, rotation, scale);
    let pos    = transform.w_axis.truncate();
    let radius = Vec3::from_array(scale).length() * 0.5;

    let obj_id = renderer.scene_mut()
        .insert_actor(SceneActor::object(ObjectDescriptor {
            mesh: mesh_id, material: mat_id, transform,
            bounds: [pos.x, pos.y, pos.z, radius.max(0.1)],
            flags: 0, groups: helio::GroupMask::NONE,
            movability: Some(helio::Movability::Movable),
        }))
        .as_object()
        .ok_or("non-object handle")?;

    out.push(LoadedMesh {
        id:          actor_key.to_string(),
        name:        obj_name.to_string(),
        mesh_id, object_id: obj_id, material_id: mat_id,
    });
    Ok(())
}

/// Legacy helper used by the v1 fallback path (no `__component_instances`).
fn spawn_mesh(
    obj_id: &str,
    obj_name: &str,
    idx: usize,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
    upload: MeshUpload,
    renderer: &mut Renderer,
    out: &mut Vec<LoadedMesh>,
) -> Result<(), String> {
    spawn_mesh_with_key(
        &format!("{}::mesh::{}", obj_id, idx),
        obj_name, position, rotation, scale,
        upload, renderer, out,
    )
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

fn load_mesh_or_fallback(path: &PathBuf) -> MeshUpload {
    if path.exists() {
        match load_fbx(path) {
            Ok(u)  => { tracing::info!(path = %path.display(), "Mesh loaded"); return u; }
            Err(e) => tracing::warn!(path = %path.display(), "Mesh load failed: {e}"),
        }
    } else {
        tracing::debug!(path = %path.display(), "Mesh file not found");
    }
    box_mesh([0.5, 0.5, 0.5])
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

fn make_material(base_color: [f32;4], roughness: f32, metallic: f32) -> GpuMaterial {
    GpuMaterial {
        base_color, emissive: [0.;4],
        roughness_metallic: [roughness, metallic, 1.5, 0.5],
        tex_base_color: GpuMaterial::NO_TEXTURE, tex_normal:    GpuMaterial::NO_TEXTURE,
        tex_roughness:  GpuMaterial::NO_TEXTURE, tex_emissive:  GpuMaterial::NO_TEXTURE,
        tex_occlusion:  GpuMaterial::NO_TEXTURE,
        workflow: 0, flags: 0, _pad: 0,
    }
}

fn prop_f32(props: &HashMap<String, Value>, key: &str, def: f32) -> f32 {
    props.get(key).and_then(|v| v.as_f64()).map(|v| v as f32).unwrap_or(def)
}

fn prop_f32_4(props: &HashMap<String, Value>, key: &str, def: [f32;4]) -> [f32;4] {
    props.get(key).and_then(|v| v.as_array()).and_then(|a| {
        if a.len() >= 4 { Some([a[0].as_f64().unwrap_or(0.) as f32, a[1].as_f64().unwrap_or(0.) as f32,
                                a[2].as_f64().unwrap_or(0.) as f32, a[3].as_f64().unwrap_or(1.) as f32])
        } else if a.len() >= 3 { Some([a[0].as_f64().unwrap_or(0.) as f32, a[1].as_f64().unwrap_or(0.) as f32,
                                       a[2].as_f64().unwrap_or(0.) as f32, 1.0])
        } else { None }
    }).unwrap_or(def)
}

fn build_primitive(mt: MeshType) -> MeshUpload {
    match mt {
        MeshType::Cube     => box_mesh([0.5,0.5,0.5]),
        MeshType::Plane    => plane_mesh(0.5),
        MeshType::Sphere   => sphere_mesh(0.5),
        MeshType::Cylinder => cylinder_mesh(),
        MeshType::Custom   => box_mesh([0.5,0.5,0.5]),
    }
}

// ── Primitives ────────────────────────────────────────────────────────────────

fn box_mesh(half: [f32;3]) -> MeshUpload {
    let e = Vec3::from_array(half);
    let c = [Vec3::new(-e.x,-e.y,e.z),Vec3::new(e.x,-e.y,e.z),Vec3::new(e.x,e.y,e.z),Vec3::new(-e.x,e.y,e.z),
             Vec3::new(-e.x,-e.y,-e.z),Vec3::new(e.x,-e.y,-e.z),Vec3::new(e.x,e.y,-e.z),Vec3::new(-e.x,e.y,-e.z)];
    let faces:[([usize;4],[f32;3],[f32;3]);6]=[
        ([0,1,2,3],[0.,0.,1.],[1.,0.,0.]),([5,4,7,6],[0.,0.,-1.],[-1.,0.,0.]),
        ([4,0,3,7],[-1.,0.,0.],[0.,0.,1.]),([1,5,6,2],[1.,0.,0.],[0.,0.,-1.]),
        ([3,2,6,7],[0.,1.,0.],[1.,0.,0.]),([4,5,1,0],[0.,-1.,0.],[1.,0.,0.]),
    ];
    let mut v=Vec::with_capacity(24); let mut i=Vec::with_capacity(36);
    for (fi,(q,n,t)) in faces.iter().enumerate() {
        let b=(fi*4)as u32; let u=[[0.,1.],[1.,1.],[1.,0.],[0.,0.]];
        for (j,&ci) in q.iter().enumerate() { v.push(PackedVertex::from_components(c[ci].to_array(),*n,u[j],*t,1.0)); }
        i.extend_from_slice(&[b,b+1,b+2,b,b+2,b+3]);
    }
    MeshUpload{vertices:v,indices:i}
}

fn plane_mesh(e: f32) -> MeshUpload {
    let n=[0.,1.,0.]; let t=[1.,0.,0.];
    let p=[[-e,0.,-e],[e,0.,-e],[e,0.,e],[-e,0.,e]];
    let u:[[f32;2];4]=[[0.,0.],[1.,0.],[1.,1.],[0.,1.]];
    let v=p.iter().zip(u.iter()).map(|(p,u)|PackedVertex::from_components(*p,n,*u,t,1.0)).collect();
    MeshUpload{vertices:v,indices:vec![0,2,1,0,3,2]}
}

fn sphere_mesh(r: f32) -> MeshUpload {
    let(la,lo)=(16usize,32usize);
    let mut v=Vec::new(); let mut i=Vec::new();
    for a in 0..=la {
        let phi=std::f32::consts::PI*(a as f32/la as f32);
        let(y,sp)=(phi.cos(),phi.sin());
        for b in 0..=lo {
            let th=2.*std::f32::consts::PI*(b as f32/lo as f32);
            let(x,z)=(sp*th.cos(),sp*th.sin());
            let tan=Vec3::new(-z,0.,x).normalize_or_zero().to_array();
            v.push(PackedVertex::from_components((Vec3::new(x,y,z)*r).to_array(),[x,y,z],
                [b as f32/lo as f32,a as f32/la as f32],tan,1.0));
        }
    }
    for a in 0..la { for b in 0..lo {
        let x=(a*(lo+1)+b)as u32; let y=x+(lo+1)as u32;
        i.extend_from_slice(&[x,x+1,y,y,x+1,y+1]);
    }}
    MeshUpload{vertices:v,indices:i}
}

fn cylinder_mesh() -> MeshUpload {
    let seg=32usize; let pi2=2.*std::f32::consts::PI;
    let mut v=Vec::new(); let mut i=Vec::new();
    for s in 0..=seg {
        let th=pi2*(s as f32/seg as f32); let(sc,cc)=th.sin_cos();
        let u=s as f32/seg as f32;
        v.push(PackedVertex::from_components([sc*0.5,0.5,cc*0.5],[sc,0.,cc],[u,0.],[cc,0.,-sc],1.0));
        v.push(PackedVertex::from_components([sc*0.5,-0.5,cc*0.5],[sc,0.,cc],[u,1.],[cc,0.,-sc],1.0));
    }
    for s in 0..seg { let b=(s*2)as u32; i.extend_from_slice(&[b,b+2,b+1,b+1,b+2,b+3]); }
    let tc=v.len()as u32;
    v.push(PackedVertex::from_components([0.,0.5,0.],[0.,1.,0.],[0.5,0.5],[1.,0.,0.],1.0));
    let tr=v.len()as u32;
    for s in 0..seg { let th=pi2*(s as f32/seg as f32); let(sc,cc)=th.sin_cos();
        v.push(PackedVertex::from_components([sc*0.5,0.5,cc*0.5],[0.,1.,0.],[sc*0.5+0.5,cc*0.5+0.5],[1.,0.,0.],1.0)); }
    for s in 0..seg as u32 { i.extend_from_slice(&[tc,tr+s,tr+(s+1)%seg as u32]); }
    let bc=v.len()as u32;
    v.push(PackedVertex::from_components([0.,-0.5,0.],[0.,-1.,0.],[0.5,0.5],[1.,0.,0.],1.0));
    let br=v.len()as u32;
    for s in 0..seg { let th=pi2*(s as f32/seg as f32); let(sc,cc)=th.sin_cos();
        v.push(PackedVertex::from_components([sc*0.5,-0.5,cc*0.5],[0.,-1.,0.],[sc*0.5+0.5,cc*0.5+0.5],[1.,0.,0.],1.0)); }
    for s in 0..seg as u32 { i.extend_from_slice(&[bc,br+(s+1)%seg as u32,br+s]); }
    MeshUpload{vertices:v,indices:i}
}
