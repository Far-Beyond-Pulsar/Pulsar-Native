//! Pulsar scene loader — shared implementation used by both the game runtime
//! and the editor engine.
//!
//! This is the **canonical** loading implementation; the engine renderer delegates
//! here instead of maintaining its own inline copy.
//!
//! ## How loading works
//!
//! Every scene object is walked regardless of its `object_type`.  For each object
//! the `__component_instances` array is extracted and dispatched by class name:
//!
//! - `"LightComponent"` → creates a [`GpuLight`] and registers it with the renderer.
//! - `"StaticMeshComponent"` → resolves the mesh asset and creates a [`MeshUpload`].
//!
//! Objects that only carry an explicit `ObjectType::Mesh` / `ObjectType::Light`
//! (i.e. no component instances) are handled by the legacy fallback path so that
//! hand-crafted / v1 scene files continue to work.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::{EulerRot, Mat4, Quat, Vec3};
use helio::{
    GpuLight, GpuMaterial, LightId, LightType as HelioLightType,
    MaterialId, MeshId, MeshUpload, ObjectDescriptor, ObjectId, PackedVertex,
    Renderer, SceneActor,
};
use serde_json::Value;

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

// ── Object input (engine-agnostic view of a scene object) ────────────────────

/// A minimal, engine-agnostic view of a single scene object.
///
/// Both `SceneObject` (game runtime) and `SceneObjectSnapshot` (editor engine)
/// convert into this type so the shared loader can process both.
pub struct SceneObjectView<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub visible: bool,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
    /// Full props map — must contain `"__component_instances"` where applicable.
    pub props: &'a HashMap<String, Value>,
    /// Fallback mesh type when no `StaticMeshComponent` is present.
    pub fallback_mesh: Option<MeshType>,
    /// Fallback light type when no `LightComponent` is present.
    pub fallback_light: Option<LightType>,
}

// ── SceneLoader ───────────────────────────────────────────────────────────────

pub struct SceneLoader;

impl SceneLoader {
    /// Load a scene **file** into a Helio renderer.
    pub fn load_file(
        path: &Path,
        project_root: &Path,
        renderer: &mut Renderer,
    ) -> Result<LoadedScene, SceneLoadError> {
        let scene = SceneFile::load(path)?;
        Self::load_scene(&scene, project_root, renderer)
    }

    /// Load a parsed [`SceneFile`] into a Helio renderer.
    pub fn load_scene(
        scene: &SceneFile,
        project_root: &Path,
        renderer: &mut Renderer,
    ) -> Result<LoadedScene, SceneLoadError> {
        let views: Vec<SceneObjectView> = scene
            .objects
            .iter()
            .map(|obj| SceneObjectView {
                id: &obj.id,
                name: &obj.name,
                visible: obj.visible,
                position: obj.world_position(),
                rotation: obj.world_rotation(),
                scale: obj.world_scale(),
                props: &obj.props,
                fallback_mesh: match obj.object_type {
                    ObjectType::Mesh(mt) => Some(mt),
                    _ => None,
                },
                fallback_light: match obj.object_type {
                    ObjectType::Light(lt) => Some(lt),
                    _ => None,
                },
            })
            .collect();

        Ok(Self::load_views(&views, project_root, renderer))
    }

    /// Core loader — processes a slice of [`SceneObjectView`]s.
    ///
    /// This is the shared implementation called by both the game runtime (via
    /// [`load_scene`]) and the editor engine (via its own snapshot adapter).
    pub fn load_views(
        objects: &[SceneObjectView<'_>],
        project_root: &Path,
        renderer: &mut Renderer,
    ) -> LoadedScene {
        let mut loaded = LoadedScene::default();

        tracing::info!(total_objects = objects.len(), "Processing scene objects");

        for obj in objects {
            if !obj.visible {
                continue;
            }

            tracing::debug!(
                id = obj.id,
                name = obj.name,
                "Scene object"
            );

            // ── Component-instance dispatch (matches engine's sync_scene) ─────
            let instances = component_instances_from_props(obj.props);
            let mut handled_light = false;
            let mut handled_mesh = false;

            for (component_index, class_name, data) in &instances {
                match class_name.as_str() {
                    "LightComponent" => {
                        handled_light = true;
                        match spawn_light_from_component(
                            obj.id, obj.name, *component_index,
                            obj.position, obj.rotation,
                            &data, renderer,
                        ) {
                            Ok(ll) => {
                                tracing::info!(id = obj.id, name = obj.name, "Light (LightComponent) loaded");
                                loaded.lights.push(ll);
                            }
                            Err(e) => tracing::warn!(
                                id = obj.id, "LightComponent spawn failed: {e}"
                            ),
                        }
                    }
                    "StaticMeshComponent" => {
                        handled_mesh = true;
                        match spawn_mesh_from_component(
                            obj.id, obj.name, *component_index,
                            obj.position, obj.rotation, obj.scale,
                            &data, project_root, renderer,
                        ) {
                            Ok(lm) => {
                                tracing::info!(id = obj.id, name = obj.name, "Mesh (StaticMeshComponent) loaded");
                                loaded.meshes.push(lm);
                            }
                            Err(e) => tracing::warn!(
                                id = obj.id, "StaticMeshComponent spawn failed: {e}"
                            ),
                        }
                    }
                    _ => {} // Unknown component — skip
                }
            }

            // ── Legacy fallback — no component instances ───────────────────────
            if !handled_light {
                if let Some(light_type) = obj.fallback_light {
                    let color = prop_color(obj.props, "color", [1.0, 1.0, 1.0]);
                    let intensity = prop_f32(obj.props, "intensity", 1.0);
                    let range = prop_f32(obj.props, "range", 10.0);
                    match spawn_light(
                        obj.id, obj.name, 0,
                        obj.position, obj.rotation,
                        color, intensity, range, light_type, renderer,
                    ) {
                        Ok(ll) => {
                            tracing::info!(id = obj.id, name = obj.name, "Light (legacy fallback) loaded");
                            loaded.lights.push(ll);
                        }
                        Err(e) => tracing::warn!(id = obj.id, "Legacy light spawn failed: {e}"),
                    }
                }
            }

            if !handled_mesh {
                if let Some(mesh_type) = obj.fallback_mesh {
                    let mesh_asset = obj.props.get("mesh_asset").and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty() && *s != "None")
                        .map(str::to_string);

                    let upload = if let Some(asset) = mesh_asset {
                        let full = resolve_asset_path(&asset, project_root);
                        load_file_mesh_or_primitive(&full, mesh_type)
                    } else {
                        build_primitive(mesh_type)
                    };

                    let transform = build_transform_parts(obj.position, obj.rotation, obj.scale);
                    match spawn_mesh_upload(obj.id, obj.name, 0, upload, transform, renderer) {
                        Ok(lm) => {
                            tracing::info!(id = obj.id, name = obj.name, "Mesh (legacy fallback) loaded");
                            loaded.meshes.push(lm);
                        }
                        Err(e) => tracing::warn!(id = obj.id, "Legacy mesh spawn failed: {e}"),
                    }
                }
            }
        }

        tracing::info!(
            meshes = loaded.meshes.len(),
            lights = loaded.lights.len(),
            "Scene load complete"
        );

        loaded
    }
}

// ── Component-instance extraction (mirrors engine's component_instances_from_snap) ──

/// Extract `(index, class_name, data)` tuples from the `__component_instances`
/// prop — exactly matching the engine's `component_instances_from_snap`.
pub fn component_instances_from_props(
    props: &HashMap<String, Value>,
) -> Vec<(usize, String, Value)> {
    let Some(entries) = props
        .get("__component_instances")
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };

    entries
        .iter()
        .enumerate()
        .filter_map(|(fallback_index, entry)| {
            let obj = entry.as_object()?;
            let index = obj
                .get("index")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
                .unwrap_or(fallback_index);
            let class_name = obj
                .get("class_name")
                .and_then(|v| v.as_str())
                .map(str::to_string)?;
            let data = obj.get("data").cloned().unwrap_or(Value::Null);
            Some((index, class_name, data))
        })
        .collect()
}

// ── LightComponent spawn (matches engine's upsert_light) ─────────────────────

fn spawn_light_from_component(
    obj_id: &str,
    obj_name: &str,
    component_index: usize,
    position: [f32; 3],
    rotation: [f32; 3],
    data: &Value,
    renderer: &mut Renderer,
) -> Result<LoadedLight, String> {
    // Parse component data — matches LightComponent::from_component_data
    let color = if let Some(arr) = data.get("color").and_then(|v| v.as_array()) {
        if arr.len() >= 4 {
            [
                arr[0].as_f64().unwrap_or(1.0) as f32,
                arr[1].as_f64().unwrap_or(1.0) as f32,
                arr[2].as_f64().unwrap_or(1.0) as f32,
            ]
        } else if arr.len() >= 3 {
            [
                arr[0].as_f64().unwrap_or(1.0) as f32,
                arr[1].as_f64().unwrap_or(1.0) as f32,
                arr[2].as_f64().unwrap_or(1.0) as f32,
            ]
        } else {
            [1.0, 1.0, 1.0]
        }
    } else {
        [1.0, 1.0, 1.0]
    };

    let intensity = data.get("intensity").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
    let range     = data.get("range").and_then(|v| v.as_f64()).unwrap_or(10.0) as f32;

    // Light type from component (default Point — matches LightComponent::default)
    let light_type = data.get("light_type")
        .and_then(|v| v.as_str())
        .map(|s| match s {
            "Directional" => LightType::Directional,
            "Spot"        => LightType::Spot,
            _             => LightType::Point,
        })
        .unwrap_or(LightType::Point);

    spawn_light(
        obj_id, obj_name, component_index,
        position, rotation,
        color, intensity, range, light_type,
        renderer,
    )
}

fn spawn_light(
    obj_id: &str,
    obj_name: &str,
    component_index: usize,
    position: [f32; 3],
    rotation: [f32; 3],
    color: [f32; 3],
    intensity: f32,
    range: f32,
    light_type: LightType,
    renderer: &mut Renderer,
) -> Result<LoadedLight, String> {
    let quat = Quat::from_euler(
        EulerRot::YXZ,
        rotation[1].to_radians(),
        rotation[0].to_radians(),
        rotation[2].to_radians(),
    );
    let forward = (quat * Vec3::NEG_Z).to_array();

    // Matches engine's upsert_light — shadow_index: 0 (engine default)
    let gpu_light = match light_type {
        LightType::Directional => GpuLight {
            position_range:  [0., 0., 0., f32::MAX],
            direction_outer: [forward[0], forward[1], forward[2], 0.],
            color_intensity: [color[0], color[1], color[2], intensity],
            shadow_index:    0,
            light_type:      HelioLightType::Directional as u32,
            inner_angle:     0.,
            _pad:            0,
        },
        LightType::Point => GpuLight {
            position_range:  [position[0], position[1], position[2], range],
            direction_outer: [0., -1., 0., 0.],
            color_intensity: [color[0], color[1], color[2], intensity],
            shadow_index:    0,
            light_type:      HelioLightType::Point as u32,
            inner_angle:     0.,
            _pad:            0,
        },
        LightType::Spot => GpuLight {
            position_range:  [position[0], position[1], position[2], range],
            direction_outer: [forward[0], forward[1], forward[2], 0.],
            color_intensity: [color[0], color[1], color[2], intensity],
            shadow_index:    0,
            light_type:      HelioLightType::Spot as u32,
            inner_angle:     0.,
            _pad:            0,
        },
    };

    let light_id = renderer
        .scene_mut()
        .insert_actor(SceneActor::light(gpu_light))
        .as_light()
        .ok_or_else(|| "insert_actor returned non-light handle".to_string())?;

    Ok(LoadedLight {
        id:   format!("{}::light::{}", obj_id, component_index),
        name: obj_name.to_string(),
        light_id,
    })
}

// ── StaticMeshComponent spawn (matches engine's upsert_mesh) ─────────────────

fn spawn_mesh_from_component(
    obj_id: &str,
    obj_name: &str,
    component_index: usize,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
    data: &Value,
    project_root: &Path,
    renderer: &mut Renderer,
) -> Result<LoadedMesh, String> {
    let mesh_asset = data
        .as_object()
        .and_then(|obj| obj.get("mesh_asset"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "None")
        .map(str::to_string)
        .ok_or_else(|| format!("StaticMeshComponent on '{}' has no mesh_asset", obj_id))?;

    let full_path = resolve_asset_path(&mesh_asset, project_root);
    let upload = load_file_mesh_or_primitive(&full_path, MeshType::Cube);
    let transform = build_transform_parts(position, rotation, scale);

    spawn_mesh_upload(obj_id, obj_name, component_index, upload, transform, renderer)
}

fn spawn_mesh_upload(
    obj_id: &str,
    obj_name: &str,
    component_index: usize,
    upload: MeshUpload,
    transform: Mat4,
    renderer: &mut Renderer,
) -> Result<LoadedMesh, String> {
    let mesh_id = renderer
        .scene_mut()
        .insert_actor(SceneActor::mesh(upload))
        .as_mesh()
        .ok_or_else(|| "insert_actor returned non-mesh handle".to_string())?;

    let material = make_material([0.6, 0.6, 0.65, 1.0], 0.7, 0.0);
    let material_id = renderer.scene_mut().insert_material(material);

    let pos = transform.w_axis.truncate();
    let radius = transform.x_axis.length().max(transform.y_axis.length()).max(transform.z_axis.length()) * 0.5;

    let object_id = renderer
        .scene_mut()
        .insert_actor(SceneActor::object(ObjectDescriptor {
            mesh: mesh_id,
            material: material_id,
            transform,
            bounds: [pos.x, pos.y, pos.z, radius.max(0.1)],
            flags: 0,
            groups: helio::GroupMask::NONE,
            movability: None,
        }))
        .as_object()
        .ok_or_else(|| "insert_actor returned non-object handle".to_string())?;

    Ok(LoadedMesh {
        id:          format!("{}::mesh::{}", obj_id, component_index),
        name:        obj_name.to_string(),
        mesh_id,
        object_id,
        material_id,
    })
}

// ── Asset resolution (matches engine's mesh_key_from_asset_path) ─────────────

fn resolve_asset_path(asset: &str, project_root: &Path) -> PathBuf {
    let normalized = asset.replace('\\', "/");
    let p = Path::new(&normalized);
    if p.is_absolute() && p.exists() {
        return p.to_path_buf();
    }
    let proj = project_root.join(&normalized);
    if proj.exists() {
        return proj;
    }
    proj // return anyway; caller handles missing
}

/// Load a mesh from file if it exists; fall back to built-in primitive.
/// Matches engine's `load_fbx_mesh` via `helio_asset_compat`.
fn load_file_mesh_or_primitive(path: &PathBuf, fallback: MeshType) -> MeshUpload {
    if path.exists() {
        match load_fbx_mesh(path) {
            Ok(upload) => {
                tracing::info!(path = %path.display(), "Mesh loaded from file");
                return upload;
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), "Mesh load failed: {e}");
            }
        }
    } else {
        tracing::debug!(path = %path.display(), "Mesh file not found — using primitive fallback");
    }
    build_primitive(fallback)
}

/// Load a mesh file via `helio-asset-compat` — matches engine's `load_fbx_mesh`.
fn load_fbx_mesh(path: &Path) -> Result<MeshUpload, String> {
    let cfg = helio_asset_compat::LoadConfig {
        flip_uv_y: true,
        merge_meshes: false,
        import_scale: glam::Vec3::ONE,
    };
    let scene = helio_asset_compat::load_scene_file_with_config(path, cfg)
        .map_err(|e| format!("Failed to load mesh \"{}\": {}", path.display(), e))?;
    scene
        .meshes
        .into_iter()
        .next()
        .map(|mesh| MeshUpload { vertices: mesh.vertices, indices: mesh.indices })
        .ok_or_else(|| format!("Mesh \"{}\" has no geometry", path.display()))
}

// ── Transform ─────────────────────────────────────────────────────────────────

/// Build a `Mat4` from separate position/rotation/scale — matches engine's `build_transform`.
pub fn build_transform_parts(position: [f32; 3], rotation: [f32; 3], scale: [f32; 3]) -> Mat4 {
    let pos = Vec3::from_array(position);
    let scl = Vec3::from_array(scale);
    let quat = Quat::from_euler(
        EulerRot::YXZ,
        rotation[1].to_radians(),
        rotation[0].to_radians(),
        rotation[2].to_radians(),
    );
    Mat4::from_scale_rotation_translation(scl, quat, pos)
}

// ── Material helper (matches engine's make_material) ─────────────────────────

fn make_material(base_color: [f32; 4], roughness: f32, metallic: f32) -> GpuMaterial {
    GpuMaterial {
        base_color,
        emissive:          [0.0, 0.0, 0.0, 0.0],
        roughness_metallic: [roughness, metallic, 1.5, 0.5],
        tex_base_color:    GpuMaterial::NO_TEXTURE,
        tex_normal:        GpuMaterial::NO_TEXTURE,
        tex_roughness:     GpuMaterial::NO_TEXTURE,
        tex_emissive:      GpuMaterial::NO_TEXTURE,
        tex_occlusion:     GpuMaterial::NO_TEXTURE,
        workflow: 0,
        flags:    0,
        _pad:     0,
    }
}

// ── Prop helpers ──────────────────────────────────────────────────────────────

fn prop_f32(props: &HashMap<String, Value>, key: &str, default: f32) -> f32 {
    props.get(key).and_then(|v| v.as_f64()).map(|v| v as f32).unwrap_or(default)
}

fn prop_color(props: &HashMap<String, Value>, key: &str, default: [f32; 3]) -> [f32; 3] {
    props.get(key)
        .and_then(|v| v.as_array())
        .and_then(|a| {
            if a.len() >= 3 {
                Some([
                    a[0].as_f64().unwrap_or(default[0] as f64) as f32,
                    a[1].as_f64().unwrap_or(default[1] as f64) as f32,
                    a[2].as_f64().unwrap_or(default[2] as f64) as f32,
                ])
            } else {
                None
            }
        })
        .unwrap_or(default)
}

// ── Built-in primitives (matches engine's box_mesh / plane_mesh / sphere_mesh) ─

fn build_primitive(mesh_type: MeshType) -> MeshUpload {
    match mesh_type {
        MeshType::Cube     => box_mesh([0.5, 0.5, 0.5]),
        MeshType::Plane    => plane_mesh(0.5),
        MeshType::Sphere   => sphere_mesh(0.5),
        MeshType::Cylinder => cylinder_mesh(),
        MeshType::Custom   => box_mesh([0.5, 0.5, 0.5]), // fallback
    }
}

/// Box mesh — matches engine's `box_mesh` exactly.
fn box_mesh(half_extents: [f32; 3]) -> MeshUpload {
    let e = Vec3::from_array(half_extents);
    let corners = [
        Vec3::new(-e.x, -e.y,  e.z),
        Vec3::new( e.x, -e.y,  e.z),
        Vec3::new( e.x,  e.y,  e.z),
        Vec3::new(-e.x,  e.y,  e.z),
        Vec3::new(-e.x, -e.y, -e.z),
        Vec3::new( e.x, -e.y, -e.z),
        Vec3::new( e.x,  e.y, -e.z),
        Vec3::new(-e.x,  e.y, -e.z),
    ];
    let faces: [([usize; 4], [f32; 3], [f32; 3]); 6] = [
        ([0, 1, 2, 3], [ 0.,  0.,  1.], [ 1.,  0.,  0.]),
        ([5, 4, 7, 6], [ 0.,  0., -1.], [-1.,  0.,  0.]),
        ([4, 0, 3, 7], [-1.,  0.,  0.], [ 0.,  0.,  1.]),
        ([1, 5, 6, 2], [ 1.,  0.,  0.], [ 0.,  0., -1.]),
        ([3, 2, 6, 7], [ 0.,  1.,  0.], [ 1.,  0.,  0.]),
        ([4, 5, 1, 0], [ 0., -1.,  0.], [ 1.,  0.,  0.]),
    ];
    let mut vertices = Vec::with_capacity(24);
    let mut indices  = Vec::with_capacity(36);
    for (fi, (quad, normal, tangent)) in faces.iter().enumerate() {
        let base = (fi * 4) as u32;
        let uvs = [[0.0_f32, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        for (i, &ci) in quad.iter().enumerate() {
            vertices.push(PackedVertex::from_components(
                corners[ci].to_array(), *normal, uvs[i], *tangent, 1.0,
            ));
        }
        indices.extend_from_slice(&[base, base+1, base+2, base, base+2, base+3]);
    }
    MeshUpload { vertices, indices }
}

/// Plane mesh — matches engine's `plane_mesh` exactly.
fn plane_mesh(half_extent: f32) -> MeshUpload {
    let e = half_extent;
    let normal  = [0.0_f32, 1.0, 0.0];
    let tangent = [1.0_f32, 0.0, 0.0];
    let positions = [[-e, 0.0, -e], [e, 0.0, -e], [e, 0.0, e], [-e, 0.0, e]];
    let uvs: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let vertices = positions.iter().zip(uvs.iter())
        .map(|(p, uv)| PackedVertex::from_components(*p, normal, *uv, tangent, 1.0))
        .collect();
    MeshUpload { vertices, indices: vec![0, 2, 1, 0, 3, 2] }
}

/// Sphere mesh — matches engine's `sphere_mesh` exactly.
fn sphere_mesh(radius: f32) -> MeshUpload {
    let (lat, lon) = (16usize, 32usize);
    let mut vertices = Vec::new();
    let mut indices  = Vec::new();
    for i in 0..=lat {
        let phi     = std::f32::consts::PI * (i as f32 / lat as f32);
        let y       = phi.cos();
        let sin_phi = phi.sin();
        for j in 0..=lon {
            let theta   = 2.0 * std::f32::consts::PI * (j as f32 / lon as f32);
            let x       = sin_phi * theta.cos();
            let z       = sin_phi * theta.sin();
            let pos     = (Vec3::new(x, y, z) * radius).to_array();
            let normal  = [x, y, z];
            let uv      = [j as f32 / lon as f32, i as f32 / lat as f32];
            let tangent = Vec3::new(-z, 0.0, x).normalize_or_zero().to_array();
            vertices.push(PackedVertex::from_components(pos, normal, uv, tangent, 1.0));
        }
    }
    for i in 0..lat {
        for j in 0..lon {
            let a = (i * (lon + 1) + j) as u32;
            let b = a + (lon + 1) as u32;
            indices.extend_from_slice(&[a, a+1, b, b, a+1, b+1]);
        }
    }
    MeshUpload { vertices, indices }
}

fn cylinder_mesh() -> MeshUpload {
    let segments = 32usize;
    let two_pi   = 2.0 * std::f32::consts::PI;
    let mut vertices = Vec::new();
    let mut indices  = Vec::new();

    for i in 0..=segments {
        let theta      = two_pi * (i as f32 / segments as f32);
        let (s, c)     = theta.sin_cos();
        let normal     = [s, 0.0, c];
        let tangent    = [c, 0.0, -s];
        let u          = i as f32 / segments as f32;
        vertices.push(PackedVertex::from_components([s*0.5,  0.5, c*0.5], normal, [u, 0.0], tangent, 1.0));
        vertices.push(PackedVertex::from_components([s*0.5, -0.5, c*0.5], normal, [u, 1.0], tangent, 1.0));
    }
    for i in 0..segments {
        let b = (i * 2) as u32;
        indices.extend_from_slice(&[b, b+2, b+1, b+1, b+2, b+3]);
    }

    let top_center = vertices.len() as u32;
    vertices.push(PackedVertex::from_components([0., 0.5, 0.], [0., 1., 0.], [0.5, 0.5], [1., 0., 0.], 1.0));
    let top_ring = vertices.len() as u32;
    for i in 0..segments {
        let theta = two_pi * (i as f32 / segments as f32);
        let (s, c) = theta.sin_cos();
        vertices.push(PackedVertex::from_components([s*0.5, 0.5, c*0.5], [0., 1., 0.], [s*0.5+0.5, c*0.5+0.5], [1., 0., 0.], 1.0));
    }
    for i in 0..segments as u32 {
        indices.extend_from_slice(&[top_center, top_ring+i, top_ring+(i+1)%segments as u32]);
    }

    let bot_center = vertices.len() as u32;
    vertices.push(PackedVertex::from_components([0., -0.5, 0.], [0., -1., 0.], [0.5, 0.5], [1., 0., 0.], 1.0));
    let bot_ring = vertices.len() as u32;
    for i in 0..segments {
        let theta = two_pi * (i as f32 / segments as f32);
        let (s, c) = theta.sin_cos();
        vertices.push(PackedVertex::from_components([s*0.5, -0.5, c*0.5], [0., -1., 0.], [s*0.5+0.5, c*0.5+0.5], [1., 0., 0.], 1.0));
    }
    for i in 0..segments as u32 {
        indices.extend_from_slice(&[bot_center, bot_ring+(i+1)%segments as u32, bot_ring+i]);
    }

    MeshUpload { vertices, indices }
}
