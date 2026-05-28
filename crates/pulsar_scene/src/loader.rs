//! Loads a [`SceneFile`] into a [`helio::Renderer`].
//!
//! Each [`SceneObject`] is translated to one or more Helio scene actors:
//! - `Mesh(*)` → `SceneActor::mesh` + `SceneActor::object` (with material)
//! - `Light(*)` → `SceneActor::light`
//! - `Empty` / `Folder` / `Camera` → skipped (no Helio representation)
//!
//! The returned [`LoadedScene`] holds all Helio handles so game code can
//! update transforms or light properties at runtime without re-parsing the
//! file.
//!
//! # Example
//!
//! ```rust,ignore
//! let loaded = SceneLoader::load_file(
//!     &project_root.join("scenes/default_level.json"),
//!     &project_root,
//!     renderer,
//! )?;
//! // loaded.objects, loaded.lights — helio handles
//! ```

use std::collections::HashMap;
use std::path::Path;

use glam::{Mat4, Quat, Vec3};
use helio::{
    Camera, GpuLight, GpuMaterial, LightId, LightType as HelioLightType,
    MaterialId, MeshId, ObjectDescriptor, ObjectId, Renderer, RendererConfig,
    SceneActor, SceneResult,
    required_wgpu_features, required_wgpu_limits,
    MeshUpload, PackedVertex,
};

use crate::format::{LightType, MeshType, ObjectType, SceneFile, SceneLoadError, SceneObject};

// ── Result type ───────────────────────────────────────────────────────────────

/// A Helio handle for a loaded mesh object.
#[derive(Clone, Debug)]
pub struct LoadedMesh {
    pub id: String,
    pub name: String,
    pub mesh_id: MeshId,
    pub object_id: ObjectId,
    pub material_id: MaterialId,
}

/// A Helio handle for a loaded light.
#[derive(Clone, Debug)]
pub struct LoadedLight {
    pub id: String,
    pub name: String,
    pub light_id: LightId,
}

/// All Helio handles produced by loading a scene file.
///
/// Keep this alive as long as you need to update the scene.
#[derive(Default, Debug)]
pub struct LoadedScene {
    pub meshes: Vec<LoadedMesh>,
    pub lights: Vec<LoadedLight>,
}

// ── Loader ────────────────────────────────────────────────────────────────────

pub struct SceneLoader;

impl SceneLoader {
    /// Parse `path` as a [`SceneFile`] and populate `renderer` with its contents.
    ///
    /// `project_root` is used to resolve relative asset paths (currently unused
    /// for built-in primitives but required for `MeshType::Custom` in the future).
    pub fn load_file(
        path: &Path,
        project_root: &Path,
        renderer: &mut Renderer,
    ) -> Result<LoadedScene, SceneLoadError> {
        let scene = SceneFile::load(path)?;
        Self::load_scene(&scene, project_root, renderer)
    }

    /// Populate `renderer` from an already-parsed [`SceneFile`].
    pub fn load_scene(
        scene: &SceneFile,
        _project_root: &Path,
        renderer: &mut Renderer,
    ) -> Result<LoadedScene, SceneLoadError> {
        let mut loaded = LoadedScene::default();

        for obj in &scene.objects {
            if !obj.visible {
                continue;
            }
            match obj.object_type {
                ObjectType::Mesh(mesh_type) => {
                    match load_mesh(obj, mesh_type, renderer) {
                        Ok(lm) => loaded.meshes.push(lm),
                        Err(e) => tracing::warn!(
                            id = %obj.id, name = %obj.name,
                            "Failed to load mesh: {e}"
                        ),
                    }
                }
                ObjectType::Light(light_type) => {
                    match load_light(obj, light_type, renderer) {
                        Ok(ll) => loaded.lights.push(ll),
                        Err(e) => tracing::warn!(
                            id = %obj.id, name = %obj.name,
                            "Failed to load light: {e}"
                        ),
                    }
                }
                ObjectType::Empty | ObjectType::Folder | ObjectType::Camera => {
                    // No Helio representation.
                }
            }
        }

        tracing::info!(
            meshes = loaded.meshes.len(),
            lights = loaded.lights.len(),
            "Scene loaded"
        );

        Ok(loaded)
    }
}

// ── Per-type loaders ──────────────────────────────────────────────────────────

fn load_mesh(
    obj: &SceneObject,
    mesh_type: MeshType,
    renderer: &mut Renderer,
) -> Result<LoadedMesh, String> {
    let upload = build_mesh_upload(mesh_type)
        .ok_or_else(|| format!("Unsupported mesh type: {:?}", mesh_type))?;

    let mesh_id = renderer
        .scene_mut()
        .insert_actor(SceneActor::mesh(upload))
        .as_mesh()
        .ok_or_else(|| "insert_actor returned non-mesh handle".to_string())?;

    let material = GpuMaterial {
        base_color: obj.mat_base_color(),
        emissive: {
            let e = obj.mat_emissive();
            let s = obj.mat_emissive_strength();
            [e[0], e[1], e[2], s]
        },
        roughness_metallic: [obj.mat_roughness(), obj.mat_metallic(), 1.5, 0.5],
        tex_base_color: GpuMaterial::NO_TEXTURE,
        tex_normal: GpuMaterial::NO_TEXTURE,
        tex_roughness: GpuMaterial::NO_TEXTURE,
        tex_emissive: GpuMaterial::NO_TEXTURE,
        tex_occlusion: GpuMaterial::NO_TEXTURE,
        workflow: 0,
        flags: 0,
        _pad: 0,
    };
    let material_id = renderer.scene_mut().insert_material(material);

    let transform = object_transform(obj);
    // Bounding radius: max of the scale components (conservative sphere).
    let radius = obj.scale.iter().cloned().fold(0.0_f32, f32::max);

    let object_id = renderer
        .scene_mut()
        .insert_actor(SceneActor::object(ObjectDescriptor {
            mesh: mesh_id,
            material: material_id,
            transform,
            bounds: [
                transform.w_axis.x,
                transform.w_axis.y,
                transform.w_axis.z,
                radius,
            ],
            flags: 0,
            groups: helio::GroupMask::NONE,
            movability: None,
        }))
        .as_object()
        .ok_or_else(|| "insert_actor returned non-object handle".to_string())?;

    Ok(LoadedMesh {
        id: obj.id.clone(),
        name: obj.name.clone(),
        mesh_id,
        object_id,
        material_id,
    })
}

fn load_light(
    obj: &SceneObject,
    light_type: LightType,
    renderer: &mut Renderer,
) -> Result<LoadedLight, String> {
    let gpu_light = build_gpu_light(obj, light_type);

    let light_id = renderer
        .scene_mut()
        .insert_actor(SceneActor::light(gpu_light))
        .as_light()
        .ok_or_else(|| "insert_actor returned non-light handle".to_string())?;

    Ok(LoadedLight {
        id: obj.id.clone(),
        name: obj.name.clone(),
        light_id,
    })
}

// ── Geometry builders ─────────────────────────────────────────────────────────

fn build_mesh_upload(mesh_type: MeshType) -> Option<MeshUpload> {
    match mesh_type {
        MeshType::Cube     => Some(cube_mesh()),
        MeshType::Plane    => Some(plane_mesh()),
        MeshType::Sphere   => Some(sphere_mesh()),
        MeshType::Cylinder => Some(cylinder_mesh()),
        MeshType::Custom   => {
            tracing::warn!("MeshType::Custom is not yet supported in pulsar_scene loader");
            None
        }
    }
}

fn cube_mesh() -> MeshUpload {
    let corners = [
        [-0.5_f32, -0.5, 0.5], [0.5, -0.5, 0.5], [0.5, 0.5, 0.5], [-0.5, 0.5, 0.5],
        [-0.5, -0.5, -0.5], [0.5, -0.5, -0.5], [0.5, 0.5, -0.5], [-0.5, 0.5, -0.5],
    ];
    let faces: [([usize; 4], [f32; 3], [f32; 3]); 6] = [
        ([0, 1, 2, 3], [0., 0., 1.], [1., 0., 0.]),
        ([5, 4, 7, 6], [0., 0., -1.], [-1., 0., 0.]),
        ([4, 0, 3, 7], [-1., 0., 0.], [0., 0., 1.]),
        ([1, 5, 6, 2], [1., 0., 0.], [0., 0., -1.]),
        ([3, 2, 6, 7], [0., 1., 0.], [1., 0., 0.]),
        ([4, 5, 1, 0], [0., -1., 0.], [1., 0., 0.]),
    ];
    let uvs = [[0.0_f32, 1.0], [1., 1.], [1., 0.], [0., 0.]];
    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (fi, (quad, normal, tangent)) in faces.iter().enumerate() {
        let base = (fi * 4) as u32;
        for (i, ci) in quad.iter().enumerate() {
            vertices.push(PackedVertex::from_components(corners[*ci], *normal, uvs[i], *tangent, 1.0));
        }
        indices.extend_from_slice(&[base, base+1, base+2, base, base+2, base+3]);
    }
    MeshUpload { vertices, indices }
}

fn plane_mesh() -> MeshUpload {
    let normal = [0.0_f32, 1.0, 0.0];
    let tangent = [1.0_f32, 0.0, 0.0];
    let positions: [[f32; 3]; 4] = [[-0.5, 0., -0.5], [0.5, 0., -0.5], [0.5, 0., 0.5], [-0.5, 0., 0.5]];
    let uvs: [[f32; 2]; 4] = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
    let vertices = positions.iter().zip(uvs.iter())
        .map(|(p, uv)| PackedVertex::from_components(*p, normal, *uv, tangent, 1.0))
        .collect();
    MeshUpload { vertices, indices: vec![0, 2, 1, 0, 3, 2] }
}

fn sphere_mesh() -> MeshUpload {
    let (lat, lon) = (16usize, 32usize);
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for i in 0..=lat {
        let phi = std::f32::consts::PI * (i as f32 / lat as f32);
        let y = phi.cos();
        let sin_phi = phi.sin();
        for j in 0..=lon {
            let theta = 2.0 * std::f32::consts::PI * (j as f32 / lon as f32);
            let x = sin_phi * theta.cos();
            let z = sin_phi * theta.sin();
            let pos = [x * 0.5, y * 0.5, z * 0.5];
            let normal = [x, y, z];
            let uv = [j as f32 / lon as f32, i as f32 / lat as f32];
            let tv = Vec3::new(-z, 0., x).normalize_or_zero();
            let tangent = tv.to_array();
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
    // Simple approximation: two end-caps + side quads.
    let segments = 24usize;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let two_pi = 2.0 * std::f32::consts::PI;

    // Side ring verts (top + bottom)
    for i in 0..=segments {
        let theta = two_pi * (i as f32 / segments as f32);
        let (s, c) = theta.sin_cos();
        let normal = [s, 0.0, c];
        let tangent = [c, 0.0, -s];
        let u = i as f32 / segments as f32;
        vertices.push(PackedVertex::from_components([s * 0.5, 0.5, c * 0.5], normal, [u, 0.0], tangent, 1.0));
        vertices.push(PackedVertex::from_components([s * 0.5, -0.5, c * 0.5], normal, [u, 1.0], tangent, 1.0));
    }
    let ring_verts = (segments + 1) * 2;
    for i in 0..segments {
        let b = (i * 2) as u32;
        indices.extend_from_slice(&[b, b+2, b+1, b+1, b+2, b+3]);
    }

    // Top cap
    let top_center = vertices.len() as u32;
    vertices.push(PackedVertex::from_components([0., 0.5, 0.], [0., 1., 0.], [0.5, 0.5], [1., 0., 0.], 1.0));
    let top_ring_start = vertices.len() as u32;
    for i in 0..segments {
        let theta = two_pi * (i as f32 / segments as f32);
        let (s, c) = theta.sin_cos();
        vertices.push(PackedVertex::from_components([s*0.5, 0.5, c*0.5], [0.,1.,0.], [s*0.5+0.5, c*0.5+0.5], [1.,0.,0.], 1.0));
    }
    for i in 0..segments as u32 {
        indices.extend_from_slice(&[top_center, top_ring_start + i, top_ring_start + (i+1) % segments as u32]);
    }

    // Bottom cap
    let bot_center = vertices.len() as u32;
    vertices.push(PackedVertex::from_components([0., -0.5, 0.], [0., -1., 0.], [0.5, 0.5], [1., 0., 0.], 1.0));
    let bot_ring_start = vertices.len() as u32;
    for i in 0..segments {
        let theta = two_pi * (i as f32 / segments as f32);
        let (s, c) = theta.sin_cos();
        vertices.push(PackedVertex::from_components([s*0.5, -0.5, c*0.5], [0.,-1.,0.], [s*0.5+0.5, c*0.5+0.5], [1.,0.,0.], 1.0));
    }
    for i in 0..segments as u32 {
        // Reverse winding for bottom face
        indices.extend_from_slice(&[bot_center, bot_ring_start + (i+1) % segments as u32, bot_ring_start + i]);
    }

    MeshUpload { vertices, indices }
}

// ── Light builder ─────────────────────────────────────────────────────────────

fn build_gpu_light(obj: &SceneObject, light_type: LightType) -> GpuLight {
    let color = obj.light_color();
    let intensity = obj.light_intensity();
    let range = obj.light_range();
    let pos = obj.position;

    // Compute forward direction from Euler rotation (YXZ degrees).
    let rot = obj.rotation;
    let quat = Quat::from_euler(
        glam::EulerRot::YXZ,
        rot[1].to_radians(),
        rot[0].to_radians(),
        rot[2].to_radians(),
    );
    let forward = (quat * Vec3::NEG_Z).to_array();

    match light_type {
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
            position_range:  [pos[0], pos[1], pos[2], range],
            direction_outer: [0., 0., -1., 0.],
            color_intensity: [color[0], color[1], color[2], intensity],
            shadow_index:    0,
            light_type:      HelioLightType::Point as u32,
            inner_angle:     0.,
            _pad:            0,
        },
        LightType::Spot => {
            let inner = obj.light_inner_angle().to_radians().cos();
            let outer = obj.light_outer_angle().to_radians().cos();
            GpuLight {
                position_range:  [pos[0], pos[1], pos[2], range],
                direction_outer: [forward[0], forward[1], forward[2], outer],
                color_intensity: [color[0], color[1], color[2], intensity],
                shadow_index:    0,
                light_type:      HelioLightType::Spot as u32,
                inner_angle:     inner,
                _pad:            0,
            }
        }
    }
}

// ── Transform helper ──────────────────────────────────────────────────────────

fn object_transform(obj: &SceneObject) -> Mat4 {
    let p = Vec3::from_array(obj.position);
    let s = Vec3::from_array(obj.scale);
    let rot = obj.rotation;
    let q = Quat::from_euler(
        glam::EulerRot::YXZ,
        rot[1].to_radians(),
        rot[0].to_radians(),
        rot[2].to_radians(),
    );
    Mat4::from_scale_rotation_translation(s, q, p)
}
