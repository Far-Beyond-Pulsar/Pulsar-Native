//! SolidRS Scene → Helio GPU structures conversion orchestrator.

use std::collections::HashMap;

use helio::{TextureUpload};
use helio_mesh_api::{PackedVertex};
use solid_rs::Scene;

use crate::material_converter::{convert_material, ConvertedMaterial, ConvertedTextureRef};
use crate::texture_loader::{load_texture_upload, TextureSemantic};
use crate::{camera_converter, light_converter, mesh_converter, CameraData, Result};

/// Build a high-res checkerboard fallback matching Unreal Engine's style:
///   • 1024×1024, 8×8 grid of 128px tiles, 3px deep grout groove
///   • Tiles curve smoothly down into the groove (cosine easing over 12px)
///   • Normal map encodes the curved depression geometry
///   • Light tiles ~195 (metallic silver), ±3 noise
///   • Dark tiles ~55 (graphite), ±15 fine grain noise
fn fallback_texture(semantic: TextureSemantic) -> helio::TextureUpload {
    let srgb = semantic.is_srgb();

    // Flat neutral for channels that don't need a visual pattern.
    match semantic {
        TextureSemantic::MetallicRoughness
        | TextureSemantic::Occlusion
        | TextureSemantic::SpecularWeight => {
            return helio::TextureUpload::rgba8(
                format!("fallback-{}", semantic.suffix()),
                1,
                1,
                false,
                vec![255, 255, 255, 255],
                helio::TextureSamplerDesc::default(),
            );
        }
        _ => {}
    }

    const SIZE: u32 = 1024;
    const TILE: u32 = 256; // 8×8 grid of 128px tiles
    const GROUT: u32 = 1; // 1px hairline groove (each boundary pixel)
    const CURVE: u32 = 5; // pixels over which tile curves down into groove

    let is_normal = semantic == TextureSemantic::Normal;

    // Deterministic position-keyed hash (no sequential state).
    let hash = |x: u32, y: u32| -> u8 {
        let mut h = x
            .wrapping_mul(2246822519_u32)
            .wrapping_add(y.wrapping_mul(3266489917_u32));
        h ^= h >> 13;
        h = h.wrapping_mul(1274126177_u32);
        h ^= h >> 16;
        (h & 0xFF) as u8
    };

    // Smooth cosine curve: returns 0.0 at groove edge, 1.0 at tile centre.
    // `edge` = pixel distance from nearest tile boundary (clamped to GROUT+CURVE).
    let curve_t = |edge: u32| -> f32 {
        if edge < GROUT {
            return 0.0;
        }
        let d = (edge - GROUT).min(CURVE) as f32 / CURVE as f32;
        // cosine ease-in: starts steep, flattens out
        (1.0 - (d * std::f32::consts::PI).cos()) * 0.5
    };

    // Groove depth factor (0=groove bottom, 1=flat tile centre) used for colour darkening.
    // In the curve zone it follows the same cosine profile.
    let depth_factor = |edge: u32| -> f32 { curve_t(edge) };

    let mut data = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let tx = x % TILE;
            let ty = y % TILE;
            let checker = ((x / TILE) + (y / TILE)) % 2 == 0;

            let edge_x = tx.min(TILE - 1 - tx);
            let edge_y = ty.min(TILE - 1 - ty);
            let edge = edge_x.min(edge_y);

            if is_normal {
                // ── Normal map: curved ramp into groove ───────────────────────
                let pixel: [u8; 4] = if edge < GROUT {
                    // Groove floor — angled strongly downward.
                    [128, 128, 100, 255]
                } else if edge < GROUT + CURVE {
                    // Curved slope: derivative of cosine ease = sin, gives the
                    // tangent angle at this point on the ramp.
                    let d = (edge - GROUT) as f32 / CURVE as f32;
                    // slope steepness: sin(π·d) peaks at midpoint, zero at ends.
                    let slope = (d * std::f32::consts::PI).sin() * 0.85;
                    let (nx, ny) = if edge_x <= edge_y {
                        let sign = if tx < TILE / 2 { 1.0_f32 } else { -1.0 };
                        (sign * slope, 0.0_f32)
                    } else {
                        let sign = if ty < TILE / 2 { 1.0_f32 } else { -1.0 };
                        (0.0_f32, sign * slope)
                    };
                    let nz = (1.0 - nx * nx - ny * ny).max(0.001).sqrt();
                    [
                        (128.0 + nx * 127.0).clamp(0.0, 255.0) as u8,
                        (128.0 + ny * 127.0).clamp(0.0, 255.0) as u8,
                        (nz * 255.0).clamp(0.0, 255.0) as u8,
                        255,
                    ]
                } else {
                    [128, 128, 255, 255] // flat tile centre
                };
                data.extend_from_slice(&pixel);
            } else {
                // ── Base colour / emissive / specular colour ──────────────────
                let df = depth_factor(edge);

                let v: u8 = if edge < GROUT {
                    // Groove — very dark (near-black pit).
                    5
                } else {
                    let flat_v: i16 = if checker {
                        // Light tile: metallic silver, minimal noise.
                        let n = (hash(x, y) & 0x07) as i16 - 3;
                        195 + n
                    } else {
                        // Dark tile: graphite with fine grain.
                        let n = (hash(x ^ 0xA5A5, y ^ 0x5A5A) & 0x1F) as i16 - 15;
                        55 + n
                    };
                    // Darken toward groove by depth factor.
                    let darkened = (flat_v as f32 * df + 5.0 * (1.0 - df)) as i16;
                    darkened.clamp(0, 255) as u8
                };
                data.extend_from_slice(&[v, v, v, 255]);
            }
        }
    }

    helio::TextureUpload::rgba8(
        format!("fallback-{}", semantic.suffix()),
        SIZE,
        SIZE,
        srgb,
        data,
        helio::TextureSamplerDesc::default(),
    )
}

/// An image file one of a source material's texture slots names, located on
/// disk.
#[derive(Debug, Clone)]
pub struct MaterialImage {
    pub semantic: TextureSemantic,
    pub path: std::path::PathBuf,
}

/// A source material: its name and the image files its texture slots name.
#[derive(Debug, Clone)]
pub struct MaterialSource {
    pub name: String,
    pub images: Vec<MaterialImage>,
}

/// The name and located image files of each source material, parallel to
/// `scene.materials`. Slots whose image is embedded or cannot be found are
/// left out.
fn collect_material_sources(scene: &Scene, base_dir: &std::path::Path) -> Vec<MaterialSource> {
    scene
        .materials
        .iter()
        .map(|material| MaterialSource {
            name: material.name.clone(),
            images: [
                (&material.base_color_texture, TextureSemantic::BaseColor),
                (&material.normal_texture, TextureSemantic::Normal),
                (
                    &material.metallic_roughness_texture,
                    TextureSemantic::MetallicRoughness,
                ),
                (&material.occlusion_texture, TextureSemantic::Occlusion),
                (&material.emissive_texture, TextureSemantic::Emissive),
            ]
            .into_iter()
            .filter_map(|(slot, semantic)| {
                let texture = slot.as_ref()?;
                let path = crate::texture_loader::locate_texture_file(
                    scene,
                    texture.texture_index,
                    base_dir,
                )?;
                Some(MaterialImage { semantic, path })
            })
            .collect(),
        })
        .collect()
}

/// Converted scene data ready for GPU upload.
pub struct ConvertedScene {
    pub name: String,
    pub meshes: Vec<ConvertedMesh>,
    pub textures: Vec<TextureUpload>,
    pub materials: Vec<ConvertedMaterial>,
    /// The source name and image files of each of `materials`.
    pub material_sources: Vec<MaterialSource>,
    pub lights: Vec<helio::GpuLight>,
    pub cameras: Vec<CameraData>,
    /// Populated instead of `meshes` when [`crate::LoadConfig::merge_meshes`] is true.
    /// One section per unique material; all sections share one vertex buffer.
    pub sectioned_mesh: Option<ConvertedSectionedMesh>,
    /// Populated alongside `sectioned_mesh` when merging: the same geometry
    /// split by source mesh and node instance, each part with its node
    /// transform baked in and its own sections. A scene with several meshes
    /// has several parts; their sections index the shared `materials`.
    pub parts: Vec<ConvertedSectionedMesh>,
    /// The skeleton, skin weights and animations, when the source has a skinned
    /// mesh. The static `meshes` / `sectioned_mesh` above are produced as
    /// before either way.
    pub rigs: Vec<crate::ImportedRig>,
}

/// One material section of a sectioned (multi-material) merged mesh.
pub struct ConvertedMeshSection {
    /// Triangle indices into the shared vertex buffer.
    pub indices: Vec<u32>,
    /// Index into [`ConvertedScene::materials`], or `None` if the section has no material.
    pub material_index: Option<usize>,
}

/// A multi-material merged mesh (produced when [`crate::LoadConfig::merge_meshes`] is true).
///
/// Mirrors Unreal Engine's Static Mesh sections: all sections share one vertex buffer.
/// Use [`helio::Scene::insert_sectioned_mesh`] + [`helio::Scene::insert_sectioned_object`]
/// to submit this to the GPU.
pub struct ConvertedSectionedMesh {
    /// Skin on the source mesh node; `None` for unskinned instances.
    pub skin: Option<usize>,
    pub name: String,
    /// Shared vertex array — all sections index into this.
    pub vertices: Vec<PackedVertex>,
    /// Per-material sections (one per unique `material_index` group).
    pub sections: Vec<ConvertedMeshSection>,
}

pub struct ConvertedMesh {
    pub skin: Option<usize>,
    pub name: String,
    pub vertices: Vec<PackedVertex>,
    pub indices: Vec<u32>,
    pub material_index: Option<usize>,
    /// Index of the source mesh this primitive belongs to.
    pub source_mesh: usize,
    /// Which node instance of that mesh this is (0 for the first).
    pub instance: usize,
    /// The source mesh's name (`Mesh_<n>` when it has none), without a
    /// primitive suffix.
    pub source_name: String,
    /// World-space transform accumulated from the scene node hierarchy.
    /// Identity when the source format has no scene graph (or the mesh is
    /// not attached to any node).
    pub node_transform: glam::Mat4,
}

pub fn convert_scene(
    scene: &Scene,
    base_dir: &std::path::Path,
    config: &crate::LoadConfig,
) -> Result<ConvertedScene> {
    let rigs = crate::extract_rigs(scene, config)?;
    log::info!(
        "Converting SolidRS scene '{}' with {} meshes, {} materials, {} textures",
        scene.name,
        scene.meshes.len(),
        scene.materials.len(),
        scene.textures.len()
    );

    let mut textures = Vec::new();
    let mut texture_cache = HashMap::<(usize, TextureSemantic), usize>::new();
    // Track which uploaded texture indices came from a fallback (missing file).
    let mut fallback_texture_indices = std::collections::HashSet::<usize>::new();
    // Track which material indices used at least one fallback texture.
    let mut used_fallback_for_mat = vec![false; scene.materials.len()];
    let mut materials: Vec<ConvertedMaterial> = Vec::with_capacity(scene.materials.len());
    for (mat_idx, material) in scene.materials.iter().enumerate() {
        let mat = convert_material(material, |texture_ref, semantic| {
            let key = (texture_ref.texture_index, semantic);
            let converted_index = if let Some(&index) = texture_cache.get(&key) {
                // A cache hit on a previously-failed texture still counts as a fallback.
                if fallback_texture_indices.contains(&index) {
                    used_fallback_for_mat[mat_idx] = true;
                }
                index
            } else {
                let upload =
                    match load_texture_upload(scene, texture_ref.texture_index, semantic, base_dir)
                    {
                        Ok(u) => u,
                        Err(e) => {
                            log::warn!(
                                "Texture {} ({:?}) not found, using fallback: {}",
                                texture_ref.texture_index,
                                semantic,
                                e
                            );
                            used_fallback_for_mat[mat_idx] = true;
                            fallback_texture_indices.insert(textures.len());
                            fallback_texture(semantic)
                        }
                    };
                let index = textures.len();
                textures.push(upload);
                texture_cache.insert(key, index);
                index
            };

            Ok(ConvertedTextureRef {
                texture_index: converted_index,
                ..texture_ref
            })
        })?;
        materials.push(mat);
    }

    // Inject the checkerboard normal map ONLY for materials that actually used
    // a fallback texture (i.e. their source textures were missing).  Materials
    // whose textures loaded successfully keep their own normals — or no normal
    // at all — rather than getting the debug grid baked on top.
    let fallback_normal_index = textures.len();
    let mut normal_fallback_pushed = false;
    for (idx, mat) in materials.iter_mut().enumerate() {
        if mat.textures.normal.is_none() && used_fallback_for_mat[idx] {
            if !normal_fallback_pushed {
                textures.push(fallback_texture(TextureSemantic::Normal));
                normal_fallback_pushed = true;
            }
            mat.textures.normal = Some(crate::material_converter::ConvertedTextureRef {
                texture_index: fallback_normal_index,
                uv_channel: 0,
                transform: Default::default(),
            });
            mat.textures.normal_scale = 1.0;
        }
    }

    // Walk the scene node DAG to compute per-mesh world-space transforms.
    // A mesh may be referenced by multiple nodes (instancing); we collect all
    // transforms so each instance becomes its own ConvertedMesh entry.
    let mut mesh_world_transforms: HashMap<usize, Vec<(glam::Mat4, Option<usize>)>> =
        HashMap::new();
    {
        // Iterative DFS: (node_id, parent_world_mat)
        let mut stack: Vec<(solid_rs::scene::NodeId, glam::Mat4)> = scene
            .roots
            .iter()
            .map(|&id| (id, glam::Mat4::IDENTITY))
            .collect();
        while let Some((node_id, parent_world)) = stack.pop() {
            let Some(node) = scene.node(node_id) else {
                continue;
            };
            // solid_rs may link a different glam version than the workspace.
            // Convert via the column array (same memory layout across versions).
            let node_mat = node.transform.to_matrix();
            let node_mat_ws = glam::Mat4::from_cols_array(&node_mat.to_cols_array());
            let world = parent_world * node_mat_ws;
            if let Some(mesh_idx) = node.mesh {
                mesh_world_transforms
                    .entry(mesh_idx)
                    .or_default()
                    .push((world, node.skin));
            }
            for &child_id in &node.children {
                stack.push((child_id, world));
            }
        }
    }

    let mut meshes = Vec::new();
    for (mesh_idx, mesh) in scene.meshes.iter().enumerate() {
        if mesh.primitives.is_empty() {
            log::warn!("Mesh '{}' has no primitives, skipping", mesh.name);
            continue;
        }

        // Get the list of world transforms for this mesh (one per node instance).
        // Fall back to identity when the mesh is not attached to any node.
        let world_transforms = mesh_world_transforms
            .get(&mesh_idx)
            .map(|v| v.as_slice())
            .unwrap_or(&[(glam::Mat4::IDENTITY, None)]);

        if world_transforms.len() > 1 {
            log::debug!(
                "Mesh '{}' is instanced {} times via scene nodes",
                mesh.name,
                world_transforms.len()
            );
        } else if !world_transforms[0]
            .0
            .abs_diff_eq(glam::Mat4::IDENTITY, 1e-6)
        {
            log::debug!("Mesh '{}' has non-identity node transform", mesh.name);
        }

        // Pre-multiply each node transform by the import scale so all vertex
        // positions are expressed in the caller's chosen unit system.
        let scale_mat = glam::Mat4::from_scale(config.import_scale);

        let source_name = if mesh.name.is_empty() {
            format!("Mesh_{}", mesh_idx)
        } else {
            mesh.name.clone()
        };
        for (instance, &(raw_node_transform, skin)) in world_transforms.iter().enumerate() {
            let node_transform = scale_mat * raw_node_transform;
            for (prim_idx, primitive) in mesh.primitives.iter().enumerate() {
                let (vertices, indices) =
                    mesh_converter::convert_primitive(mesh, primitive, config)?;

                let mesh_name = if mesh.name.is_empty() {
                    if mesh.primitives.len() > 1 {
                        format!("Mesh_{}_{}", mesh_idx, prim_idx)
                    } else {
                        format!("Mesh_{}", mesh_idx)
                    }
                } else if mesh.primitives.len() > 1 {
                    format!("{}_{}", mesh.name, prim_idx)
                } else {
                    mesh.name.clone()
                };

                meshes.push(ConvertedMesh {
                    skin,
                    name: mesh_name,
                    vertices,
                    indices,
                    material_index: primitive.material_index,
                    source_mesh: mesh_idx,
                    instance,
                    source_name: source_name.clone(),
                    node_transform,
                });
            }
        }
    }

    let parts = if config.merge_meshes || !rigs.is_empty() {
        // The same geometry per source mesh and node instance, in the order
        // they first appear.
        let mut groups: Vec<((usize, usize), Vec<&ConvertedMesh>)> = Vec::new();
        for sub in &meshes {
            let key = (sub.source_mesh, sub.instance);
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, subs)) => subs.push(sub),
                None => groups.push((key, vec![sub])),
            }
        }
        groups
            .iter()
            .map(|(_, subs)| {
                let first = subs[0];
                let name = if first.instance == 0 {
                    first.source_name.clone()
                } else {
                    format!("{}_{}", first.source_name, first.instance + 1)
                };
                let mut part = merge_sectioned(name, subs);
                part.skin = first.skin;
                part
            })
            .collect()
    } else {
        Vec::new()
    };

    // Merge all sub-meshes into a single flat mesh when requested.
    // Each mesh's node_transform is baked into its vertex positions/normals/
    // tangents so the merged result can live at IDENTITY.
    if config.merge_meshes && !meshes.is_empty() {
        let all: Vec<&ConvertedMesh> = meshes.iter().collect();
        let sectioned = merge_sectioned(scene.name.clone(), &all);

        let lights = scene
            .lights
            .iter()
            .filter_map(light_converter::convert_light)
            .collect::<Vec<_>>();
        let cameras = scene
            .cameras
            .iter()
            .map(camera_converter::extract_camera_data)
            .collect::<Vec<_>>();

        return Ok(ConvertedScene {
            name: scene.name.clone(),
            meshes: vec![],
            textures,
            materials,
            material_sources: collect_material_sources(scene, base_dir),
            lights,
            cameras,
            sectioned_mesh: Some(sectioned),
            parts,
            rigs,
        });
    }

    let lights = scene
        .lights
        .iter()
        .filter_map(light_converter::convert_light)
        .collect::<Vec<_>>();
    let cameras = scene
        .cameras
        .iter()
        .map(camera_converter::extract_camera_data)
        .collect::<Vec<_>>();

    Ok(ConvertedScene {
        name: scene.name.clone(),
        meshes,
        textures,
        materials,
        material_sources: collect_material_sources(scene, base_dir),
        lights,
        cameras,
        sectioned_mesh: None,
        parts,
        rigs,
    })
}

/// Weld `subs` into one sectioned mesh: a section per material, in the order
/// materials are first met, with each sub-mesh's node transform baked into its
/// vertex positions, normals and tangents.
fn merge_sectioned(name: String, subs: &[&ConvertedMesh]) -> ConvertedSectionedMesh {
    let mut material_order: Vec<Option<usize>> = Vec::new();
    let mut group_verts: Vec<Vec<PackedVertex>> = Vec::new();
    let mut group_indices: Vec<Vec<u32>> = Vec::new();

    for sub in subs {
        // Find or create the bucket for this material.
        let bucket = material_order
            .iter()
            .position(|&m| m == sub.material_index)
            .unwrap_or_else(|| {
                let idx = material_order.len();
                material_order.push(sub.material_index);
                group_verts.push(Vec::new());
                group_indices.push(Vec::new());
                idx
            });

        let base = group_verts[bucket].len() as u32;
        let xform = sub.node_transform;
        let normal_mat = glam::Mat3::from_mat4(xform.inverse().transpose());
        let tangent_mat = glam::Mat3::from_mat4(xform);
        let handedness_sign = if xform.determinant() < 0.0 {
            -1.0_f32
        } else {
            1.0
        };

        for v in &sub.vertices {
            let pos = xform.transform_point3(glam::Vec3::from(v.position));
            let norm_raw = unpack_snorm4x8(v.normal);
            let tan_raw = unpack_snorm4x8(v.tangent);
            let norm = (normal_mat * glam::Vec3::new(norm_raw[0], norm_raw[1], norm_raw[2]))
                .normalize_or_zero();
            let tan = (tangent_mat * glam::Vec3::new(tan_raw[0], tan_raw[1], tan_raw[2]))
                .normalize_or_zero();
            let mut mv = PackedVertex::from_components(
                pos.into(),
                norm.into(),
                v.tex_coords0,
                tan.into(),
                v.bitangent_sign * handedness_sign,
            );
            mv.tex_coords1 = v.tex_coords1;
            group_verts[bucket].push(mv);
        }
        for &idx in &sub.indices {
            group_indices[bucket].push(base + idx);
        }
    }

    // One shared vertex buffer (the union of the buckets), with every
    // section's indices absolute into it.
    let mut shared_vertices: Vec<PackedVertex> = Vec::new();
    let mut sections: Vec<ConvertedMeshSection> = Vec::new();
    for (i, mat_idx) in material_order.into_iter().enumerate() {
        let base = shared_vertices.len() as u32;
        let abs_indices: Vec<u32> = group_indices[i].iter().map(|&idx| base + idx).collect();
        shared_vertices.extend_from_slice(&group_verts[i]);
        sections.push(ConvertedMeshSection {
            indices: abs_indices,
            material_index: mat_idx,
        });
    }
    ConvertedSectionedMesh {
        skin: None,
        name,
        vertices: shared_vertices,
        sections,
    }
}

/// Unpack a `pack_snorm4x8` value back to four f32 components in [-1, 1].
fn unpack_snorm4x8(packed: u32) -> [f32; 4] {
    let bytes = packed.to_le_bytes();
    [
        bytes[0] as i8 as f32 / 127.0,
        bytes[1] as i8 as f32 / 127.0,
        bytes[2] as i8 as f32 / 127.0,
        bytes[3] as i8 as f32 / 127.0,
    ]
}
