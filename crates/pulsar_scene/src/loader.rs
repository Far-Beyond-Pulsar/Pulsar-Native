//! Pulsar scene loader — shared implementation used by both the game runtime
//! and the editor engine.
//!
//! Uses **exactly** the same dispatch path as the editor engine:
//! `pulsar_reflection::apply_runtime_behavior_for_class` → inventory-registered
//! `LightComponent` / `StaticMeshComponent` handlers from `pulsar_rendering`.

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
    apply_runtime_behavior_for_class, ComponentRuntimeContext, RuntimeComponentOwner,
    RuntimeLightDesc, RuntimeLightType, RuntimeMeshDesc,
};

use crate::format::{LightType, MeshType, ObjectType, SceneFile, SceneLoadError, SceneObject};

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

// ── Runtime context — mirrors engine's HelioRuntimeContext, without caching ───

struct SceneLoaderContext<'a> {
    renderer: &'a mut Renderer,
    project_root: &'a Path,
    loaded: &'a mut LoadedScene,
    current_id: String,
    current_name: String,
    current_position: [f32; 3],
    current_rotation: [f32; 3],
    current_scale: [f32; 3],
}

impl<'a> ComponentRuntimeContext for SceneLoaderContext<'a> {
    fn upsert_light(&mut self, desc: RuntimeLightDesc) {
        // Delegate to the shared canonical builder — byte-for-byte identical to engine.
        let gpu_light = build_gpu_light(&desc, self.current_position);

        match self.renderer.scene_mut().insert_actor(SceneActor::light(gpu_light)).as_light() {
            Some(light_id) => {
                tracing::info!(id = %self.current_id, name = %self.current_name, "Light loaded");
                self.loaded.lights.push(LoadedLight {
                    id: desc.actor_key, name: self.current_name.clone(), light_id,
                });
            }
            None => tracing::warn!(id = %self.current_id, "insert_actor returned non-light"),
        }
    }

    fn upsert_mesh(&mut self, desc: RuntimeMeshDesc) {
        let full_path = resolve_asset_path(&desc.mesh_asset, self.project_root);

        let upload = if full_path.exists() {
            match load_fbx_mesh(&full_path) {
                Ok(u)  => { tracing::info!(path = %full_path.display(), "Mesh loaded"); u }
                Err(e) => { tracing::warn!("{e}"); box_mesh([0.5,0.5,0.5]) }
            }
        } else {
            tracing::debug!(path = %full_path.display(), "Not found, using box fallback");
            box_mesh([0.5, 0.5, 0.5])
        };

        let mesh_id = match self.renderer.scene_mut()
            .insert_actor(SceneActor::mesh(upload)).as_mesh()
        {
            Some(id) => id,
            None => return,
        };

        let mat_id = self.renderer.scene_mut()
            .insert_material(make_material([0.6, 0.6, 0.65, 1.0], 0.7, 0.0));

        let transform = build_transform_parts(
            self.current_position, self.current_rotation, self.current_scale,
        );
        let pos = transform.w_axis.truncate();
        let radius = Vec3::from_array(self.current_scale).length() * 0.5;

        let obj_id = match self.renderer.scene_mut()
            .insert_actor(SceneActor::object(ObjectDescriptor {
                mesh: mesh_id, material: mat_id, transform,
                bounds: [pos.x, pos.y, pos.z, radius.max(0.1)],
                flags: 0, groups: helio::GroupMask::NONE, movability: None,
            })).as_object()
        {
            Some(id) => id,
            None => return,
        };

        tracing::info!(id = %self.current_id, name = %self.current_name, "Mesh loaded");
        self.loaded.meshes.push(LoadedMesh {
            id: desc.actor_key, name: self.current_name.clone(),
            mesh_id, object_id: obj_id, material_id: mat_id,
        });
    }

    fn report_error(&mut self, message: String) {
        tracing::warn!(id = %self.current_id, "{}", message);
    }
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

    /// Core loader — identical inner loop to engine's `sync_scene`.
    pub fn load_views(
        objects: &[SceneObjectView<'_>],
        project_root: &Path,
        renderer: &mut Renderer,
    ) -> LoadedScene {
        let mut loaded = LoadedScene::default();
        tracing::info!(total = objects.len(), "Processing scene objects");

        let mut ctx = SceneLoaderContext {
            renderer, project_root, loaded: &mut loaded,
            current_id: String::new(), current_name: String::new(),
            current_position: [0.0;3], current_rotation: [0.0;3], current_scale: [1.0;3],
        };

        for obj in objects {
            if !obj.visible { continue; }
            tracing::debug!(id = obj.id, name = obj.name, "Scene object");

            ctx.current_id       = obj.id.to_string();
            ctx.current_name     = obj.name.to_string();
            ctx.current_position = obj.position;
            ctx.current_rotation = obj.rotation;
            ctx.current_scale    = obj.scale;

            let owner = RuntimeComponentOwner {
                scene_object_id: obj.id,
                position: obj.position,
                rotation: obj.rotation,
                scale:    obj.scale,
                props:    obj.props,
            };

            // Exact same loop as engine's sync_scene.
            let instances = component_instances_from_props(obj.props);
            let mut had_component = false;

            for (component_index, class_name, data) in &instances {
                if apply_runtime_behavior_for_class(
                    class_name.as_str(), &owner, *component_index, data, &mut ctx,
                ) {
                    had_component = true;
                }
            }

            // Legacy fallback for v1 scenes with no component instances.
            if !had_component {
                if let Some(lt) = obj.fallback_light {
                    let color     = prop_arr4(obj.props, "color", [1.0,1.0,1.0,1.0]);
                    let intensity = prop_f32(obj.props, "intensity", 1.0);
                    let range     = prop_f32(obj.props, "range", 10.0);
                    ctx.upsert_light(RuntimeLightDesc {
                        actor_key: format!("{}::light::0", obj.id),
                        light_type: match lt {
                            LightType::Directional => RuntimeLightType::Directional,
                            LightType::Point       => RuntimeLightType::Point,
                            LightType::Spot        => RuntimeLightType::Spot,
                        },
                        color, intensity, range,
                        inner_cone_angle_deg: prop_f32(obj.props, "inner_angle", 30.0),
                        outer_cone_angle_deg: prop_f32(obj.props, "outer_angle", 45.0),
                    });
                }
                if let Some(mt) = obj.fallback_mesh {
                    let asset = obj.props.get("mesh_asset")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty() && *s != "None")
                        .unwrap_or("");
                    if asset.is_empty() {
                        let upload = build_primitive(mt);
                        let transform = build_transform_parts(obj.position, obj.rotation, obj.scale);
                        let pos = transform.w_axis.truncate();
                        if let Some(mesh_id) = ctx.renderer.scene_mut()
                            .insert_actor(SceneActor::mesh(upload)).as_mesh()
                        {
                            let mat_id = ctx.renderer.scene_mut()
                                .insert_material(make_material([0.6,0.6,0.65,1.0],0.7,0.0));
                            if let Some(obj_id) = ctx.renderer.scene_mut()
                                .insert_actor(SceneActor::object(ObjectDescriptor {
                                    mesh: mesh_id, material: mat_id, transform,
                                    bounds: [pos.x,pos.y,pos.z,0.5],
                                    flags:0, groups:helio::GroupMask::NONE, movability:None,
                                })).as_object()
                            {
                                ctx.loaded.meshes.push(LoadedMesh {
                                    id: format!("{}::mesh::0", obj.id),
                                    name: obj.name.to_string(),
                                    mesh_id, object_id: obj_id, material_id: mat_id,
                                });
                            }
                        }
                    } else {
                        ctx.upsert_mesh(RuntimeMeshDesc {
                            actor_key:  format!("{}::mesh::0", obj.id),
                            mesh_asset: asset.to_string(),
                        });
                    }
                }
            }
        }

        tracing::info!(meshes = loaded.meshes.len(), lights = loaded.lights.len(), "Scene load complete");
        loaded
    }
}

// ── Shared exports (used by engine_backend) ───────────────────────────────────

/// Extract `(index, class_name, data)` from `__component_instances`.
/// Mirrors engine's `component_instances_from_snap` exactly.
pub fn component_instances_from_props(
    props: &HashMap<String, Value>,
) -> Vec<(usize, String, Value)> {
    let Some(entries) = props.get("__component_instances").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    entries.iter().enumerate().filter_map(|(fallback_index, entry)| {
        let obj = entry.as_object()?;
        let index = obj.get("index").and_then(|v| v.as_u64()).map(|v| v as usize)
            .unwrap_or(fallback_index);
        let class_name = obj.get("class_name").and_then(|v| v.as_str()).map(str::to_string)?;
        let data = obj.get("data").cloned().unwrap_or(Value::Null);
        Some((index, class_name, data))
    }).collect()
}

/// Build transform — mirrors engine's `build_transform` exactly.
pub fn build_transform_parts(position: [f32;3], rotation: [f32;3], scale: [f32;3]) -> Mat4 {
    let quat = Quat::from_euler(EulerRot::YXZ,
        rotation[1].to_radians(), rotation[0].to_radians(), rotation[2].to_radians());
    Mat4::from_scale_rotation_translation(Vec3::from_array(scale), quat, Vec3::from_array(position))
}

// ── Internal ──────────────────────────────────────────────────────────────────

fn resolve_asset_path(asset: &str, project_root: &Path) -> PathBuf {
    if asset.is_empty() { return PathBuf::new(); }
    let norm = asset.replace('\\', "/");
    let p = Path::new(&norm);
    if p.is_absolute() && p.exists() { return p.to_path_buf(); }
    let proj = project_root.join(&norm);
    if proj.exists() { return proj; }
    proj
}

fn load_fbx_mesh(path: &Path) -> Result<MeshUpload, String> {
    let cfg = helio_asset_compat::LoadConfig { flip_uv_y: true, merge_meshes: false, import_scale: glam::Vec3::ONE };
    helio_asset_compat::load_scene_file_with_config(path, cfg)
        .map_err(|e| format!("load \"{}\": {}", path.display(), e))?
        .meshes.into_iter().next()
        .map(|m| MeshUpload { vertices: m.vertices, indices: m.indices })
        .ok_or_else(|| format!("\"{}\" has no geometry", path.display()))
}

fn make_material(base_color: [f32;4], roughness: f32, metallic: f32) -> GpuMaterial {
    GpuMaterial {
        base_color, emissive: [0.;4],
        roughness_metallic: [roughness, metallic, 1.5, 0.5],
        tex_base_color: GpuMaterial::NO_TEXTURE, tex_normal: GpuMaterial::NO_TEXTURE,
        tex_roughness:  GpuMaterial::NO_TEXTURE, tex_emissive: GpuMaterial::NO_TEXTURE,
        tex_occlusion:  GpuMaterial::NO_TEXTURE,
        workflow: 0, flags: 0, _pad: 0,
    }
}

fn prop_f32(props: &HashMap<String, Value>, key: &str, default: f32) -> f32 {
    props.get(key).and_then(|v| v.as_f64()).map(|v| v as f32).unwrap_or(default)
}

fn prop_arr4(props: &HashMap<String, Value>, key: &str, default: [f32;4]) -> [f32;4] {
    props.get(key).and_then(|v| v.as_array()).and_then(|a| {
        if a.len() >= 4 { Some([a[0].as_f64().unwrap_or(0.) as f32, a[1].as_f64().unwrap_or(0.) as f32,
                                a[2].as_f64().unwrap_or(0.) as f32, a[3].as_f64().unwrap_or(1.) as f32])
        } else if a.len() >= 3 { Some([a[0].as_f64().unwrap_or(0.) as f32, a[1].as_f64().unwrap_or(0.) as f32,
                                       a[2].as_f64().unwrap_or(0.) as f32, 1.0])
        } else { None }
    }).unwrap_or(default)
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
        let b=(fi*4) as u32;
        let u=[[0.,1.],[1.,1.],[1.,0.],[0.,0.]];
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
    let (la,lo)=(16usize,32usize);
    let mut v=Vec::new(); let mut i=Vec::new();
    for a in 0..=la {
        let phi=std::f32::consts::PI*(a as f32/la as f32);
        let (y,sp)=(phi.cos(),phi.sin());
        for b in 0..=lo {
            let th=2.*std::f32::consts::PI*(b as f32/lo as f32);
            let (x,z)=(sp*th.cos(),sp*th.sin());
            let tan=Vec3::new(-z,0.,x).normalize_or_zero().to_array();
            v.push(PackedVertex::from_components((Vec3::new(x,y,z)*r).to_array(),[x,y,z],
                [b as f32/lo as f32,a as f32/la as f32],tan,1.0));
        }
    }
    for a in 0..la { for b in 0..lo {
        let x=(a*(lo+1)+b) as u32; let y=x+(lo+1) as u32;
        i.extend_from_slice(&[x,x+1,y,y,x+1,y+1]);
    }}
    MeshUpload{vertices:v,indices:i}
}

fn cylinder_mesh() -> MeshUpload {
    let seg=32usize; let pi2=2.*std::f32::consts::PI;
    let mut v=Vec::new(); let mut i=Vec::new();
    for s in 0..=seg { let th=pi2*(s as f32/seg as f32); let(sc,cc)=th.sin_cos();
        v.push(PackedVertex::from_components([sc*0.5,0.5,cc*0.5],[sc,0.,cc],[s as f32/seg as f32,0.],[cc,0.,-sc],1.0));
        v.push(PackedVertex::from_components([sc*0.5,-0.5,cc*0.5],[sc,0.,cc],[s as f32/seg as f32,1.],[cc,0.,-sc],1.0)); }
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

// ── Shared GpuLight builder — the ONE place both engine and game construct lights ──

/// Build a [`GpuLight`] from a [`RuntimeLightDesc`] and world position.
///
/// This is the **canonical** implementation.  Both the editor engine
/// (`HelioRuntimeContext::upsert_light`) and the game runtime
/// (`SceneLoaderContext::upsert_light`) call this function so that the
/// resulting GPU data is byte-for-byte identical.
///
/// Construction matches the engine's `upsert_light` exactly:
/// - `direction_outer.xyz` = `[0, -1, 0]` (fixed; spot direction not yet wired)
/// - `direction_outer.w`   = outer angle in **radians** (matches engine convention)
/// - `inner_angle`         = inner angle in **radians** (matches engine convention)
/// - `shadow_index`        = `u32::MAX`  (no shadow — matches engine)
pub fn build_gpu_light(
    desc: &RuntimeLightDesc,
    position: [f32; 3],
) -> GpuLight {
    let light_type = match desc.light_type {
        RuntimeLightType::Directional => HelioLightType::Directional,
        RuntimeLightType::Point       => HelioLightType::Point,
        RuntimeLightType::Spot        => HelioLightType::Spot,
        RuntimeLightType::Area        => HelioLightType::Point, // same fallback as engine
    };

    GpuLight {
        position_range:  [position[0], position[1], position[2], desc.range],
        direction_outer: [0.0, -1.0, 0.0, desc.outer_cone_angle_deg.to_radians()],
        color_intensity: [desc.color[0], desc.color[1], desc.color[2], desc.intensity],
        shadow_index:    u32::MAX,
        light_type:      light_type as u32,
        inner_angle:     desc.inner_cone_angle_deg.to_radians(),
        _pad:            0,
    }
}

/// Load a mesh file and return a [`MeshUpload`].
///
/// Canonical implementation shared by engine and game — delegates to
/// `helio_asset_compat` exactly as the engine's `load_fbx_mesh` does.
pub fn load_mesh_upload(path: &Path) -> Result<MeshUpload, String> {
    load_fbx_mesh(path)
}
