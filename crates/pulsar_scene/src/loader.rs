//! Loads a [`SceneFile`] into a [`helio::Renderer`].
//!
//! ## Mesh resolution order (per object)
//!
//! 1. Read `mesh_asset` from the object's `StaticMeshComponent` / `props`.
//! 2. If the filename matches a known primitive (SM_Cube, SM_Sphere, …), use
//!    our procedural geometry — this is correct and avoids disk I/O.
//! 3. If the file exists on disk and has a recognised extension (`.fbx`, `.gltf`,
//!    `.glb`, `.obj`), load it through `helio-asset-compat` / `solid-fbx`.
//! 4. Fall back to the `object_type` primitive (Cube / Sphere / …).
//!
//! Lights are always built from the scene file data; no file loading needed.

use std::path::Path;

use glam::{Mat4, Quat, Vec3};
use helio::{
    GpuLight, GpuMaterial, LightId, LightType as HelioLightType,
    MaterialId, MeshId, ObjectDescriptor, ObjectId, Renderer,
    SceneActor, MeshUpload, PackedVertex,
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
        let mut loaded = LoadedScene::default();
        let total = scene.objects.len();
        tracing::info!(total_objects = total, "Processing scene objects");

        for obj in &scene.objects {
            tracing::debug!(
                id = %obj.id,
                name = %obj.name,
                type_ = ?obj.object_type,
                visible = obj.visible,
                mesh_asset = ?obj.mesh_asset(),
                "Scene object"
            );

            if !obj.visible {
                continue;
            }

            match obj.object_type {
                ObjectType::Mesh(_) => {
                    match load_mesh(obj, project_root, renderer) {
                        Ok(lm) => {
                            tracing::info!(id = %obj.id, name = %obj.name, "Mesh loaded");
                            loaded.meshes.push(lm);
                        }
                        Err(e) => tracing::warn!(
                            id = %obj.id, name = %obj.name,
                            "Mesh load failed: {e}"
                        ),
                    }
                }
                ObjectType::Light(light_type) => {
                    match load_light(obj, light_type, renderer) {
                        Ok(ll) => {
                            tracing::info!(id = %obj.id, name = %obj.name, "Light loaded");
                            loaded.lights.push(ll);
                        }
                        Err(e) => tracing::warn!(
                            id = %obj.id, name = %obj.name,
                            "Light load failed: {e}"
                        ),
                    }
                }
                ObjectType::Empty | ObjectType::Folder | ObjectType::Camera
                | ObjectType::Unknown => {}
            }
        }

        tracing::info!(
            meshes = loaded.meshes.len(),
            lights = loaded.lights.len(),
            "Scene load complete"
        );
        Ok(loaded)
    }
}

// ── Mesh loading ──────────────────────────────────────────────────────────────

fn load_mesh(
    obj: &SceneObject,
    project_root: &Path,
    renderer: &mut Renderer,
) -> Result<LoadedMesh, String> {
    // --- 1. Resolve geometry ----------------------------------------------------
    let upload = resolve_mesh_upload(obj, project_root);

    // --- 2. Upload mesh ---------------------------------------------------------
    let mesh_id = renderer
        .scene_mut()
        .insert_actor(SceneActor::mesh(upload))
        .as_mesh()
        .ok_or_else(|| "insert_actor returned non-mesh handle".to_string())?;

    // --- 3. Material from scene props -------------------------------------------
    let material = GpuMaterial {
        base_color: obj.mat_base_color(),
        emissive: {
            let e = obj.mat_emissive();
            let s = obj.mat_emissive_strength();
            [e[0], e[1], e[2], s]
        },
        roughness_metallic: [obj.mat_roughness(), obj.mat_metallic(), 1.5, 0.5],
        tex_base_color:  GpuMaterial::NO_TEXTURE,
        tex_normal:      GpuMaterial::NO_TEXTURE,
        tex_roughness:   GpuMaterial::NO_TEXTURE,
        tex_emissive:    GpuMaterial::NO_TEXTURE,
        tex_occlusion:   GpuMaterial::NO_TEXTURE,
        workflow: 0,
        flags:   0,
        _pad:    0,
    };
    let material_id = renderer.scene_mut().insert_material(material);

    // --- 4. Place object --------------------------------------------------------
    let transform = object_transform(obj);
    let radius = obj.world_scale().iter().cloned().fold(0.0_f32, f32::max);

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
            flags:      0,
            groups:     helio::GroupMask::NONE,
            movability: None,
        }))
        .as_object()
        .ok_or_else(|| "insert_actor returned non-object handle".to_string())?;

    Ok(LoadedMesh {
        id:          obj.id.clone(),
        name:        obj.name.clone(),
        mesh_id,
        object_id,
        material_id,
    })
}

/// Resolve what geometry to use for a mesh object, in priority order.
fn resolve_mesh_upload(obj: &SceneObject, project_root: &Path) -> MeshUpload {
    // 1. mesh_asset path from scene props
    if let Some(asset_path) = obj.mesh_asset() {
        // 1a. Is it a known primitive by filename?
        if let Some(prim) = primitive_from_asset_name(asset_path) {
            tracing::debug!(
                id = %obj.id,
                asset = asset_path,
                prim = ?prim,
                "mesh_asset resolved to built-in primitive"
            );
            if let Some(upload) = build_mesh_upload(prim) {
                return upload;
            }
        }

        // 1b. Try to load from disk (FBX / GLTF / OBJ).
        let full_path = if std::path::Path::new(asset_path).is_absolute() {
            std::path::PathBuf::from(asset_path)
        } else {
            project_root.join(asset_path)
        };

        if full_path.exists() {
            tracing::info!(
                id = %obj.id,
                path = %full_path.display(),
                "Loading mesh from file"
            );
            if let Some(upload) = load_file_mesh(&full_path) {
                return upload;
            }
        } else {
            tracing::debug!(
                id = %obj.id,
                path = %full_path.display(),
                "Mesh asset file not found — falling back to primitive"
            );
        }
    }

    // 2. Fall back to the object_type primitive.
    let fallback_prim = match obj.object_type {
        ObjectType::Mesh(mt) => mt,
        _ => MeshType::Cube,
    };
    tracing::debug!(id = %obj.id, prim = ?fallback_prim, "Using object_type primitive fallback");
    build_mesh_upload(fallback_prim).unwrap_or_else(cube_mesh)
}

/// Map a mesh asset filename to a built-in primitive type.
///
/// Matches by file stem, case-insensitive.  Returns `None` for genuine custom
/// assets (e.g. `"monkey.fbx"`, `"character.glb"`).
fn primitive_from_asset_name(path: &str) -> Option<MeshType> {
    let stem = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_lowercase();

    if stem.contains("cube")     { Some(MeshType::Cube)     }
    else if stem.contains("sphere")   { Some(MeshType::Sphere)   }
    else if stem.contains("cylinder") { Some(MeshType::Cylinder) }
    else if stem.contains("plane")    { Some(MeshType::Plane)    }
    else { None }
}

/// Load a mesh file via `helio-asset-compat` (FBX, GLTF, OBJ …).
///
/// All sub-meshes are merged into a single `MeshUpload`.  Node transforms
/// from the file are **ignored** — the scene object's own transform is applied
/// by the caller via `ObjectDescriptor::transform`.
fn load_file_mesh(path: &std::path::Path) -> Option<MeshUpload> {
    use helio_asset_compat::{load_scene_file_with_config, LoadConfig};

    // FBX uses DirectX UV convention (top-left origin) — flip V for Helio.
    let flip_uv = path.extension()
        .and_then(|e| e.to_str())
        .map(|e| matches!(e.to_lowercase().as_str(), "fbx"))
        .unwrap_or(false);

    let config = LoadConfig::default().with_uv_flip(flip_uv);

    match load_scene_file_with_config(path, config) {
        Ok(scene) => {
            let mut vertices: Vec<PackedVertex> = Vec::new();
            let mut indices:  Vec<u32>          = Vec::new();

            for mesh in &scene.meshes {
                let base = vertices.len() as u32;
                vertices.extend_from_slice(&mesh.vertices);
                for &idx in &mesh.indices {
                    indices.push(base + idx);
                }
            }

            if vertices.is_empty() {
                tracing::warn!(path = %path.display(), "Mesh file loaded but has no geometry");
                return None;
            }

            tracing::info!(
                path = %path.display(),
                vertices = vertices.len(),
                triangles = indices.len() / 3,
                "Mesh file loaded OK"
            );
            Some(MeshUpload { vertices, indices })
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), "Failed to load mesh file: {e}");
            None
        }
    }
}

fn build_mesh_upload(mesh_type: MeshType) -> Option<MeshUpload> {
    match mesh_type {
        MeshType::Cube     => Some(cube_mesh()),
        MeshType::Plane    => Some(plane_mesh()),
        MeshType::Sphere   => Some(sphere_mesh()),
        MeshType::Cylinder => Some(cylinder_mesh()),
        MeshType::Custom   => {
            tracing::warn!("MeshType::Custom has no asset path — no geometry");
            None
        }
    }
}

// ── Light loading ─────────────────────────────────────────────────────────────

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
        id:   obj.id.clone(),
        name: obj.name.clone(),
        light_id,
    })
}

/// Convert a single sRGB channel (0–1 range) to linear light.
///
/// Scene files store colours as normalised sRGB (0–1).  Helio's WGSL shader
/// expects linear RGB, so we must un-gamma the values before handing them off.
#[inline]
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn build_gpu_light(obj: &SceneObject, light_type: LightType) -> GpuLight {
    let srgb      = obj.light_color();
    // Convert sRGB → linear for correct physically-based lighting.
    let color     = [
        srgb_to_linear(srgb[0]),
        srgb_to_linear(srgb[1]),
        srgb_to_linear(srgb[2]),
    ];
    let intensity = obj.light_intensity();
    let range     = obj.light_range();
    let pos       = obj.world_position();

    let rot  = obj.world_rotation();
    let quat = Quat::from_euler(
        glam::EulerRot::YXZ,
        rot[1].to_radians(),
        rot[0].to_radians(),
        rot[2].to_radians(),
    );
    let forward = (quat * Vec3::NEG_Z).to_array();

    // u32::MAX means "no shadow map" in Helio (documented as -1u32 = no shadow).
    // Using 0 would reference an uninitialised shadow slot and corrupt lighting.
    const NO_SHADOW: u32 = u32::MAX;

    match light_type {
        LightType::Directional => GpuLight {
            position_range:  [0., 0., 0., f32::MAX],
            direction_outer: [forward[0], forward[1], forward[2], 0.],
            color_intensity: [color[0], color[1], color[2], intensity],
            shadow_index:    NO_SHADOW,
            light_type:      HelioLightType::Directional as u32,
            inner_angle:     0.,
            _pad:            0,
        },
        LightType::Point => GpuLight {
            position_range:  [pos[0], pos[1], pos[2], range],
            direction_outer: [0., 0., -1., 0.],
            color_intensity: [color[0], color[1], color[2], intensity],
            shadow_index:    NO_SHADOW,
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
                shadow_index:    NO_SHADOW,
                light_type:      HelioLightType::Spot as u32,
                inner_angle:     inner,
                _pad:            0,
            }
        }
    }
}

// ── Transform helper ──────────────────────────────────────────────────────────

fn object_transform(obj: &SceneObject) -> Mat4 {
    let p = Vec3::from_array(obj.world_position());
    let s = Vec3::from_array(obj.world_scale());
    let rot = obj.world_rotation();
    let q = Quat::from_euler(
        glam::EulerRot::YXZ,
        rot[1].to_radians(),
        rot[0].to_radians(),
        rot[2].to_radians(),
    );
    Mat4::from_scale_rotation_translation(s, q, p)
}

// ── Procedural geometry ───────────────────────────────────────────────────────

fn cube_mesh() -> MeshUpload {
    let corners = [
        [-0.5_f32, -0.5, 0.5], [0.5, -0.5, 0.5], [0.5, 0.5, 0.5], [-0.5, 0.5, 0.5],
        [-0.5, -0.5, -0.5], [0.5, -0.5, -0.5], [0.5, 0.5, -0.5], [-0.5, 0.5, -0.5],
    ];
    // (quad indices, outward normal, tangent along U axis)
    let faces: [([usize; 4], [f32; 3], [f32; 3]); 6] = [
        ([0, 1, 2, 3], [ 0.,  0.,  1.], [ 1.,  0.,  0.]),  // +Z front
        ([5, 4, 7, 6], [ 0.,  0., -1.], [-1.,  0.,  0.]),  // -Z back
        ([4, 0, 3, 7], [-1.,  0.,  0.], [ 0.,  0.,  1.]),  // -X left
        ([1, 5, 6, 2], [ 1.,  0.,  0.], [ 0.,  0., -1.]),  // +X right
        ([3, 2, 6, 7], [ 0.,  1.,  0.], [ 1.,  0.,  0.]),  // +Y top
        ([4, 5, 1, 0], [ 0., -1.,  0.], [ 1.,  0.,  0.]),  // -Y bottom
    ];
    let uvs = [[0.0_f32, 1.0], [1., 1.], [1., 0.], [0., 0.]];
    let mut vertices = Vec::with_capacity(24);
    let mut indices  = Vec::with_capacity(36);

    for (fi, (quad, normal, tangent)) in faces.iter().enumerate() {
        let base = (fi * 4) as u32;
        for (i, &ci) in quad.iter().enumerate() {
            vertices.push(PackedVertex::from_components(
                corners[ci], *normal, uvs[i], *tangent, 1.0,
            ));
        }
        // CCW winding, viewed from outside (consistent with counter-clockwise front-face)
        indices.extend_from_slice(&[base, base+1, base+2, base, base+2, base+3]);
    }
    MeshUpload { vertices, indices }
}

fn plane_mesh() -> MeshUpload {
    let normal  = [0.0_f32, 1.0, 0.0];
    let tangent = [1.0_f32, 0.0, 0.0];
    let positions: [[f32; 3]; 4] = [
        [-0.5, 0., -0.5], [0.5, 0., -0.5], [0.5, 0., 0.5], [-0.5, 0., 0.5],
    ];
    let uvs: [[f32; 2]; 4] = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
    let vertices = positions.iter().zip(uvs.iter())
        .map(|(p, uv)| PackedVertex::from_components(*p, normal, *uv, tangent, 1.0))
        .collect();
    // CCW winding viewed from +Y (looking down)
    MeshUpload { vertices, indices: vec![0, 2, 1, 0, 3, 2] }
}

fn sphere_mesh() -> MeshUpload {
    let (lat, lon) = (24usize, 48usize);
    let mut vertices = Vec::new();
    let mut indices  = Vec::new();

    for i in 0..=lat {
        let phi     = std::f32::consts::PI * (i as f32 / lat as f32);
        let y       = phi.cos();
        let sin_phi = phi.sin();
        for j in 0..=lon {
            let theta = 2.0 * std::f32::consts::PI * (j as f32 / lon as f32);
            let x = sin_phi * theta.cos();
            let z = sin_phi * theta.sin();
            let pos    = [x * 0.5, y * 0.5, z * 0.5];
            let normal = [x, y, z];
            let uv     = [j as f32 / lon as f32, i as f32 / lat as f32];
            // Tangent along longitude direction (∂pos/∂θ, normalised)
            let tangent = Vec3::new(-theta.sin(), 0., theta.cos()).normalize_or_zero().to_array();
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

    // ── Side rings (top at y=0.5, bottom at y=-0.5) ──────────────────────────
    for i in 0..=segments {
        let theta      = two_pi * (i as f32 / segments as f32);
        let (s, c)     = theta.sin_cos();
        let normal     = [s, 0.0, c];
        let tangent    = [c, 0.0, -s];  // ∂pos/∂θ direction
        let u          = i as f32 / segments as f32;
        vertices.push(PackedVertex::from_components([s*0.5, 0.5, c*0.5],  normal, [u, 0.0], tangent, 1.0));
        vertices.push(PackedVertex::from_components([s*0.5, -0.5, c*0.5], normal, [u, 1.0], tangent, 1.0));
    }
    for i in 0..segments {
        let b = (i * 2) as u32;
        indices.extend_from_slice(&[b, b+2, b+1, b+1, b+2, b+3]);
    }

    // ── Top cap ───────────────────────────────────────────────────────────────
    let top_center = vertices.len() as u32;
    vertices.push(PackedVertex::from_components(
        [0., 0.5, 0.], [0., 1., 0.], [0.5, 0.5], [1., 0., 0.], 1.0,
    ));
    let top_ring_start = vertices.len() as u32;
    for i in 0..segments {
        let theta  = two_pi * (i as f32 / segments as f32);
        let (s, c) = theta.sin_cos();
        vertices.push(PackedVertex::from_components(
            [s*0.5, 0.5, c*0.5], [0., 1., 0.],
            [s*0.5+0.5, c*0.5+0.5], [1., 0., 0.], 1.0,
        ));
    }
    for i in 0..segments as u32 {
        indices.extend_from_slice(&[
            top_center,
            top_ring_start + i,
            top_ring_start + (i + 1) % segments as u32,
        ]);
    }

    // ── Bottom cap (reversed winding so normal points -Y) ─────────────────────
    let bot_center = vertices.len() as u32;
    vertices.push(PackedVertex::from_components(
        [0., -0.5, 0.], [0., -1., 0.], [0.5, 0.5], [1., 0., 0.], 1.0,
    ));
    let bot_ring_start = vertices.len() as u32;
    for i in 0..segments {
        let theta  = two_pi * (i as f32 / segments as f32);
        let (s, c) = theta.sin_cos();
        vertices.push(PackedVertex::from_components(
            [s*0.5, -0.5, c*0.5], [0., -1., 0.],
            [s*0.5+0.5, c*0.5+0.5], [1., 0., 0.], 1.0,
        ));
    }
    for i in 0..segments as u32 {
        // Flipped winding vs top cap
        indices.extend_from_slice(&[
            bot_center,
            bot_ring_start + (i + 1) % segments as u32,
            bot_ring_start + i,
        ]);
    }

    MeshUpload { vertices, indices }
}
