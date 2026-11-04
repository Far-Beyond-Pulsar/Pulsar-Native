//! The meshes' SceneDB components and ray-tracing projection for standalone
//! Helio hosts (the plugin's examples, its tests, the web and Android demos):
//! what `helio-components` is for the kinds still in Helio. Engines register
//! their own components (Pulsar's mesh components) and run the derivations
//! the meshes' feature crates provide.
//!
//! - [`MeshComponent`], [`StaticObjectComponent`], [`RenderGroupComponent`]:
//!   the rows the meshes' passes read ([`register_columns`]).
//! - [`ray_tracing`]: the frame's TLAS from these rows, published as the
//!   frame's ray-traced scene (`helio_frame_api::rays::SCENE_RAYS`);
//!   [`ray_proxies`]: virtual-geometry instances traced through proxies.
//! - [`features`]: the meshes' Helio features, for a host's preset
//!   (`helio_presets::Preset::with_features`).
//! - Bake input ([`mesh_upload_to_bake`], [`bake_scene_from_world`]) and
//!   imported scenes' materials ([`upload_scene_materials`]).

pub mod ray_proxies;
pub mod ray_tracing;
/// The meshes' row components (helio-mesh-api's `host_rows`, where
/// their rows are defined).
pub use helio_mesh_api::host_rows::{MeshComponent, RenderGroupComponent, StaticObjectComponent};

/// The meshes' Helio features: the object batch, its culling, the drawers
/// (G-buffer, shadow casters, forward, transparent) and virtual geometry.
pub fn features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new()
        .with(helio_pass_object_batch::ObjectBatchFeature)
        .with(helio_pass_occlusion_cull::MeshCulling)
        .with(helio_pass_gbuffer::GBufferDrawers)
        .with(helio_pass_shadow::ShadowDrawers)
        .with(helio_pass_forward_lit::ForwardDrawers)
        .with(helio_pass_transparent::TransparentDrawers)
        .with(helio_pass_vg::VgFeature)
}

/// Registers the meshes' columns (meshes, objects, render groups) with a
/// store before its mirror is attached: registering up front, not on first
/// insert, so a column's first rows are never lost.
pub fn register_columns(
    store: &mut pulsar_scenedb::gpu::SceneGpuStore,
    device: &std::sync::Arc<wgpu::Device>,
) {
    MeshComponent::register_gpu_columns_growable(store, 4096, device);
    StaticObjectComponent::register_gpu_columns_growable(store, 4096, device);
}

/// Spawn a SceneDB material row for every material in a loaded
/// [`helio_asset_compat::ConvertedScene`], returning one [`pulsar_scenedb::Entity`] per
/// `scene.materials[i]`, in the same order.
///
/// This is the direct SceneDB replacement for the old `Scene`-based
/// `upload_scene_materials`: since a material is now just a
/// `MaterialComponent` row, spawning one *is* the whole
/// job. Its old job also included uploading each material's textures into
/// the (now-deleted) `Scene`'s bindless texture table and patching the
/// resulting indices into the `GpuMaterial` before inserting it -- bindless
/// texture upload has no SceneDB-authored home yet, so `ConvertedMaterial::
/// textures` is not consulted here. `convert_material`'s own `GpuMaterial`
/// already carries `NO_TEXTURE` in every `tex_*` slot (texture info is
/// returned separately, in `textures`), so every spawned material carries
/// its real base color/roughness/metallic/emissive factors but none of its
/// texture maps.
pub fn upload_scene_materials(
    world: &mut pulsar_scenedb::World,
    scene: &helio_asset_compat::ConvertedScene,
) -> Vec<pulsar_scenedb::Entity> {
    scene
        .materials
        .iter()
        .map(|converted| {
            let entity = world.spawn();
            world.insert(
                entity,
                helio_components::MaterialComponent::from(helio_mats::MaterialRow::from(
                    converted.gpu,
                )),
            );
            entity
        })
        .collect()
}

/// A mesh with a world-space transform as the bake's input
/// ([`helio_bake::BakeMesh`]): the meshes feed the baker through its own
/// geometry type, so the baker knows no mesh layout.
///
/// Positions are pre-multiplied by `transform` so the baker receives world-space
/// geometry.  Normals are rotated by the inverse-transpose to handle non-uniform
/// scaling. Use `SceneGeometry::add_mesh` to add the returned mesh to your scene.
///
/// # Example
/// ```rust,ignore
/// let mut scene = SceneGeometry::new();
/// scene.add_mesh(mesh_upload_to_bake(&box_mesh([0.0,0.0,0.0], [5.0,0.1,5.0]),
///                                    glam::Mat4::IDENTITY, None));
/// bake(&mut renderer, scene, BakeConfig::fast("my_scene"));
/// ```
pub fn mesh_upload_to_bake(
    upload: &helio_mesh_api::MeshUpload,
    transform: glam::Mat4,
    mesh_slot: Option<u32>,
) -> helio_bake::BakeMesh {
    fn unpack_snorm8(b: u8) -> f32 {
        (b as i8) as f32 / 127.0
    }
    let normal_mat = glam::Mat3::from_mat4(transform).inverse().transpose();

    let id = helio_bake::bake_mesh_id(mesh_slot);

    // Select which UV channel to pass to Nebula for baking.
    //
    // If the mesh has a dedicated lightmap UV channel (UV1, non-overlapping [0,1]),
    // pass it explicitly so the bake uses the same coordinates the runtime shader
    // will use.  Detection: at least one vertex must have a clearly non-zero UV1
    // value (|u| or |v| > 1e-4).
    //
    // If UV1 is absent (all-zero — the common case when a mesh ships with only one
    // UV channel), fall back to UV0.  The runtime shader also falls back to UV0
    // (clamped to [0,1]) in this case, so both sides agree.
    let has_uv1 = upload
        .vertices
        .iter()
        .any(|v| v.tex_coords1[0].abs() > 1e-4 || v.tex_coords1[1].abs() > 1e-4);
    let lightmap_uvs = if has_uv1 {
        Some(
            upload
                .vertices
                .iter()
                .map(|v| v.tex_coords1)
                .collect::<Vec<_>>(),
        )
    } else {
        // Explicitly pass UV0 so Nebula bakes to it.  Passing None might
        // cause some Nebula versions to auto-generate UVs that the runtime
        // cannot recover, leading to a UV mismatch and zero lightmap effect.
        Some(
            upload
                .vertices
                .iter()
                .map(|v| v.tex_coords0)
                .collect::<Vec<_>>(),
        )
    };

    helio_bake::BakeMesh {
        id,
        positions: upload
            .vertices
            .iter()
            .map(|v| {
                transform
                    .transform_point3(glam::Vec3::from_array(v.position))
                    .to_array()
            })
            .collect(),
        normals: upload
            .vertices
            .iter()
            .map(|v| {
                let p = v.normal;
                let n = glam::Vec3::new(
                    unpack_snorm8(p as u8),
                    unpack_snorm8((p >> 8) as u8),
                    unpack_snorm8((p >> 16) as u8),
                );
                (normal_mat * n).normalize_or_zero().to_array()
            })
            .collect(),
        uvs: upload.vertices.iter().map(|v| v.tex_coords0).collect(),
        lightmap_uvs,
        indices: upload.indices.clone(),
        material_ids: vec![0u32; upload.indices.len() / 3],
        world_transform: Default::default(),
    }
}

/// Project a SceneDB world into the [`helio_bake::SceneGeometry`] a bake reads
/// (Helio#256): every drawn object (`StaticObjectRow` and the
/// `MeshComponent` it references, in world space) and every live light
/// (`LightRow`). This is the explicit CPU projection
/// `helio_components::bake` takes; the renderer itself never traverses scene entities.
///
/// Call it after the scene is populated, and hand the result to `helio_components::bake`. Objects whose mesh row is gone (a stale
/// generation) are skipped.
pub fn bake_scene_from_world(world: &pulsar_scenedb::World) -> helio_bake::SceneGeometry {
    use helio_bake::{LightSource, LightSourceKind};
    use helio_components::LightComponent;
    use helio_mesh_api::MeshUpload;
    use helio_pass_light_cull::{GpuLight, LightType};

    let meshes: std::collections::HashMap<u32, (u32, &MeshComponent)> = world
        .query::<&MeshComponent>()
        .map(|(entity, mesh)| (entity.index(), (entity.generation(), mesh)))
        .collect();
    let mut scene = helio_bake::SceneGeometry::new();
    for (_, object) in world.query::<&StaticObjectComponent>() {
        let Some(&(generation, mesh)) = meshes.get(&object.mesh_slot) else {
            continue;
        };
        // Object rows store `generation + 1` so zero means "never written".
        if generation.wrapping_add(1) != object.mesh_generation {
            continue;
        }
        let upload = MeshUpload {
            vertices: mesh.vertices.clone(),
            indices: mesh.indices.clone(),
        };
        let transform = glam::Mat4::from_cols_array_2d(&object.transform);
        scene.add_mesh(mesh_upload_to_bake(
            &upload,
            transform,
            Some(object.mesh_slot),
        ));
    }
    for (_, light) in world.query::<&LightComponent>() {
        let light = GpuLight::from(helio_pass_light_cull::LightRow::from(*light));
        if light.color_intensity[3] <= 0.0 {
            continue;
        }
        let [x, y, z, range] = light.position_range;
        let direction = [
            light.direction_outer[0],
            light.direction_outer[1],
            light.direction_outer[2],
        ];
        let kind = match light.light_type {
            t if t == LightType::Directional as u32 => LightSourceKind::Directional { direction },
            // GpuLight stores cosines; Nebula takes radians.
            t if t == LightType::Spot as u32 => LightSourceKind::Spot {
                position: [x, y, z],
                direction,
                range,
                inner_angle: light.inner_angle.clamp(-1.0, 1.0).acos(),
                outer_angle: light.direction_outer[3].clamp(-1.0, 1.0).acos(),
            },
            _ => LightSourceKind::Point {
                position: [x, y, z],
                range,
            },
        };
        scene.add_light(LightSource {
            kind,
            color: [
                light.color_intensity[0],
                light.color_intensity[1],
                light.color_intensity[2],
            ],
            intensity: light.color_intensity[3],
            bake_enabled: true,
            casts_shadows: light.shadow_index != u32::MAX,
        });
    }
    scene
}

#[cfg(test)]
mod bake_scene_tests {
    use super::*;
    use helio_bake::LightSourceKind;
    use helio_components::WorldRows;
    use helio_mesh_api::PackedVertex;
    use helio_mesh_api::StaticObjectRow;
    use helio_pass_light_cull::LightRow;
    use helio_pass_light_cull::{GpuLight, LightType};

    fn vertex(position: [f32; 3]) -> PackedVertex {
        PackedVertex {
            position,
            ..Default::default()
        }
    }

    /// Helio#256: the bake input comes from SceneDB rows, in world space,
    /// and skips what is not really there.
    #[test]
    fn projects_objects_and_live_lights_from_the_world() {
        let mut world = pulsar_scenedb::World::new();
        let mesh = world.spawn();
        world.insert(
            mesh,
            MeshComponent {
                vertices: vec![
                    vertex([0.0, 0.0, 0.0]),
                    vertex([1.0, 0.0, 0.0]),
                    vertex([0.0, 1.0, 0.0]),
                ],
                indices: vec![0, 1, 2],
            },
        );
        let object = |mesh_generation: u32, offset: f32| {
            StaticObjectRow::new(
                mesh.index(),
                mesh_generation,
                0,
                1,
                glam::Mat4::from_translation(glam::Vec3::new(offset, 0.0, 0.0)),
                [offset, 0.0, 0.0, 1.0],
                3,
                0,
                0,
                0,
                0,
                0,
            )
        };
        let live = world.spawn();
        world.insert_row(live, object(mesh.generation().wrapping_add(1), 10.0));
        // A row pointing at a mesh generation that no longer exists.
        let stale = world.spawn();
        world.insert_row(stale, object(mesh.generation().wrapping_add(2), 20.0));

        let mut sun = GpuLight::default();
        sun.light_type = LightType::Directional as u32;
        sun.direction_outer = [0.0, -1.0, 0.0, 0.0];
        sun.color_intensity = [1.0, 0.9, 0.8, 3.0];
        let sun_entity = world.spawn();
        world.insert_row(sun_entity, LightRow::from(sun));
        let vacant = world.spawn();
        world.insert_row(
            vacant,
            LightRow::from(GpuLight {
                color_intensity: [0.0; 4],
                ..GpuLight::default()
            }),
        );

        let scene = bake_scene_from_world(&world);
        assert_eq!(scene.meshes.len(), 1, "stale object rows are skipped");
        assert_eq!(
            scene.meshes[0].positions[1],
            [11.0, 0.0, 0.0],
            "positions are world space"
        );
        assert_eq!(scene.meshes[0].indices, vec![0, 1, 2]);
        assert_eq!(scene.lights.len(), 1, "dark rows are not lights");
        assert!(
            matches!(scene.lights[0].kind, LightSourceKind::Directional { direction } if direction == [0.0, -1.0, 0.0])
        );
        assert_eq!(scene.lights[0].intensity, 3.0);
        assert!(
            !scene.lights[0].casts_shadows,
            "GpuLight::default() requests no shadow"
        );
    }
}

/// Helio's presets over the meshes' features ([`features`]): what a
/// standalone host builds a renderer with to draw meshes
/// (`RendererBuilder::with_pass_build_context(Box::new(presets::deferred))`).
pub mod presets {
    use helio::PassBuildContext;
    use helio_core::RenderGraph;
    use helio_presets::{Preset, PresetGraph};

    /// `preset` over the meshes' features.
    pub fn preset(preset: Preset) -> PresetGraph {
        preset.with_features(&super::features())
    }

    /// `preset` over the meshes' features and `extra` (another plugin's).
    pub fn with(preset: Preset, extra: &helio_feature_api::FeatureSet) -> PresetGraph {
        preset.with_features(&super::features().extended(extra))
    }

    pub fn deferred(ctx: PassBuildContext<'_>) -> RenderGraph {
        preset(Preset::Deferred).build(ctx)
    }

    pub fn deferred_fxaa(ctx: PassBuildContext<'_>) -> RenderGraph {
        preset(Preset::DeferredFxaa).build(ctx)
    }

    pub fn hlfs(ctx: PassBuildContext<'_>) -> RenderGraph {
        preset(Preset::Hlfs).build(ctx)
    }

    pub fn hlfs_fxaa(ctx: PassBuildContext<'_>) -> RenderGraph {
        preset(Preset::HlfsFxaa).build(ctx)
    }

    pub fn forward(ctx: PassBuildContext<'_>) -> RenderGraph {
        preset(Preset::Forward).build(ctx)
    }
}
