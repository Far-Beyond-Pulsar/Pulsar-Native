//! SceneDB's GPU-mirror attachment seam for Helio 3.0.
///
/// SceneDB owns all active world and asset state. Helio receives the
/// cloneable GPU mirror during renderer construction and consumes the mirror's
/// component buffers directly. This module contains no renderer scene, frame
/// projection, material table, subscription cache, or per-frame input
/// assembly.
use std::{collections::HashSet, sync::Arc};

use crate::scene::material_textures::register_graph_texture;
use helio_component::components::StaticMeshComponent;
use pulsar_scenedb::gpu::{
    BufferKey, EngineGpuContext, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig, SceneGpuStore,
};

use crate::scene::{
    material_graph::{compile_material_graph, texture_assets},
    Transform, Visibility,
};

/// Arm the change-time projection once for the currently live scene. Future
/// transform/material/visibility edits are delivered as entity-specific
/// events, so the renderer can update only the affected derived row.
pub fn arm_render_row_subscriptions(world: &mut pulsar_scenedb::World) {
    let mesh_entities: Vec<_> = world
        .query::<&StaticMeshComponent>()
        .map(|(entity, _)| entity)
        .collect();
    for entity in mesh_entities {
        arm_render_row_subscriptions_for_entity(world, entity);
    }
    let light_entities: Vec<_> = world
        .query::<&helio_component::components::LightComponent>()
        .map(|(entity, _)| entity)
        .collect();
    for entity in light_entities {
        arm_render_row_subscriptions_for_entity(world, entity);
    }
}

/// Arm subscriptions for one newly-created entity without revisiting the rest
/// of the scene. Structural editor commands call this immediately after spawn.
pub fn arm_render_row_subscriptions_for_entity(
    world: &mut pulsar_scenedb::World,
    entity: pulsar_scenedb::Entity,
) {
    if world.get::<StaticMeshComponent>(entity).is_some() {
        let _ = world.subscribe::<StaticMeshComponent>(entity);
        let _ = world.subscribe::<Transform>(entity);
        let _ = world.subscribe::<Visibility>(entity);
    }
    if world
        .get::<helio_component::components::LightComponent>(entity)
        .is_some()
    {
        let _ = world.subscribe::<helio_component::components::LightComponent>(entity);
        let _ = world.subscribe::<Transform>(entity);
        let _ = world.subscribe::<Visibility>(entity);
    }
}

/// Report `entity`'s render-relevant components as changed to the render-row
/// subscriptions (#935), as a property edit through `Mut` would: its light
/// and mesh rows are re-derived at the renderer's next sync. Use after
/// components were (re)inserted before the entity's subscriptions were
/// armed (a class instance rebuilt from its class), which records no
/// change event. Also refreshes every registered class's GPU mirror.
pub fn mark_render_components_changed(
    world: &mut pulsar_scenedb::World,
    entity: pulsar_scenedb::Entity,
) {
    if !world.is_alive(entity) {
        return;
    }
    let classes: Vec<&'static str> =
        pulsar_world_registry::registered_world_component_classes().collect();
    for class_name in classes {
        if pulsar_world_registry::world_component_present_for_class(class_name, world, entity) {
            pulsar_world_registry::refresh_world_component_gpu_mirror_for_class(
                class_name, world, entity,
            );
        }
    }
    if let Some(mut light) = world.get_mut::<helio_component::components::LightComponent>(entity) {
        std::ops::DerefMut::deref_mut(&mut light);
    }
    if let Some(mut mesh) = world.get_mut::<StaticMeshComponent>(entity) {
        std::ops::DerefMut::deref_mut(&mut mesh);
    }
    if let Some(mut transform) = world.get_mut::<Transform>(entity) {
        std::ops::DerefMut::deref_mut(&mut transform);
    }
}

struct EditorMeshRow;

/// Renderer-only draw row for a non-first material section. It lives in the
/// SceneDB world so the existing GPU object-batch path can consume the same
/// reflected `StaticObjectComponent` schema for every section.
#[derive(Clone, Copy)]
struct MeshSectionDraw {
    owner: pulsar_scenedb::Entity,
    section_index: usize,
}

fn section_draw_entities(
    world: &pulsar_scenedb::World,
    owner: pulsar_scenedb::Entity,
) -> Vec<(pulsar_scenedb::Entity, usize)> {
    let mut draws: Vec<_> = world
        .query::<&MeshSectionDraw>()
        .filter(|(_, draw)| draw.owner == owner)
        .map(|(entity, draw)| (entity, draw.section_index))
        .collect();
    draws.sort_by_key(|(_, index)| *index);
    draws
}

fn sync_section_draw_entities(
    world: &mut pulsar_scenedb::World,
    owner: pulsar_scenedb::Entity,
    section_count: usize,
) -> Vec<pulsar_scenedb::Entity> {
    let draws = section_draw_entities(world, owner);
    let wanted = section_count.saturating_sub(1);
    let mut entities = Vec::with_capacity(wanted);
    for section_index in 1..section_count {
        if let Some((entity, _)) = draws
            .iter()
            .find(|(_, existing_index)| *existing_index == section_index)
        {
            entities.push(*entity);
        } else {
            let entity = world.spawn();
            world.insert(
                entity,
                MeshSectionDraw {
                    owner,
                    section_index,
                },
            );
            entities.push(entity);
        }
    }
    for (entity, _) in draws {
        if !entities.contains(&entity) {
            world.despawn(entity);
        }
    }
    debug_assert_eq!(entities.len(), wanted);
    entities
}

fn retire_section_draw_entities(world: &mut pulsar_scenedb::World, owner: pulsar_scenedb::Entity) {
    for (entity, _) in section_draw_entities(world, owner) {
        world.despawn(entity);
    }
}

#[derive(Clone)]
struct ResolvedSlotMaterial {
    surface: helio_component::mesh_cache::ImportedSurfaceMaterial,
    material_class: u32,
    graph_hash: u64,
}

fn graph_material_cache() -> &'static std::sync::Mutex<
    std::collections::HashMap<std::path::PathBuf, (u64, Result<(u64, String), String>)>,
> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<
            std::collections::HashMap<std::path::PathBuf, (u64, Result<(u64, String), String>)>,
        >,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn graph_material_source(
    path: &std::path::Path,
    project_root: &std::path::Path,
    mirror: &GpuMirrorHandle,
) -> Result<(u64, String), String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    use std::hash::{Hash, Hasher};
    let mut fingerprint_hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut fingerprint_hasher);
    let mut fingerprint = fingerprint_hasher.finish();
    let result = (|| {
        let text = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
        // Shader graph saves may carry a line comment before their JSON body.
        let json_start = text
            .find('{')
            .ok_or_else(|| "shader graph JSON object is missing".to_string())?;
        let document: serde_json::Value = serde_json::from_str(&text[json_start..])
            .map_err(|error| format!("invalid shader graph JSON: {error}"))?;
        let graph_value = document
            .get("main_graph")
            .ok_or_else(|| "shader graph asset has no main_graph".to_string())?;
        let graph: psgc::GraphDescription = serde_json::from_value(graph_value.clone())
            .map_err(|error| format!("invalid main_graph: {error}"))?;
        let mut texture_bindings = std::collections::HashMap::new();
        for asset in texture_assets(&graph)? {
            let texture_path = if std::path::Path::new(&asset).is_absolute() {
                std::path::PathBuf::from(&asset)
            } else {
                project_root.join(&asset)
            };
            let slot = register_graph_texture(&texture_path, mirror)
                .map_err(|error| format!("texture '{}': {error}", texture_path.display()))?;
            // Scene-local slots are part of the compiled shader identity.
            asset.hash(&mut fingerprint_hasher);
            slot.hash(&mut fingerprint_hasher);
            texture_bindings.insert(asset, slot);
        }
        fingerprint = fingerprint_hasher.finish();
        if let Ok(cache) = graph_material_cache().lock() {
            if let Some((cached_fingerprint, cached_result)) = cache.get(path) {
                if *cached_fingerprint == fingerprint {
                    return cached_result.clone();
                }
            }
        }
        let snippet = compile_material_graph(&graph, &texture_bindings)?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        snippet.hash(&mut hasher);
        let hash = hasher.finish().max(1);
        Ok((hash, snippet))
    })();
    if let Ok(mut cache) = graph_material_cache().lock() {
        cache.insert(path.to_path_buf(), (fingerprint, result.clone()));
    }
    result
}

fn material_surface_for_slot(
    slot: Option<&helio_component::components::StaticMeshMaterialSlot>,
    entity: pulsar_scenedb::Entity,
    mirror: &GpuMirrorHandle,
) -> ResolvedSlotMaterial {
    let imported = || ResolvedSlotMaterial {
        surface: slot.map_or_else(Default::default, |slot| {
            slot.surface_override.unwrap_or(slot.imported_surface)
        }),
        material_class: helio_mats::MATERIAL_CLASS_DEFAULT,
        graph_hash: 0,
    };
    let Some(slot) = slot else {
        return imported();
    };
    if let Some(override_surface) = slot.surface_override {
        return ResolvedSlotMaterial {
            surface: override_surface,
            material_class: helio_mats::MATERIAL_CLASS_DEFAULT,
            graph_hash: 0,
        };
    }
    if slot.material_asset.trim().is_empty() {
        return imported();
    }
    let Some(project_root) = engine_state::get_project_path() else {
        return imported();
    };
    let path = helio_component::subsystems::resolve_asset_path(
        std::path::Path::new(&project_root),
        &slot.material_asset,
    );
    let graph_file = if path.is_dir() {
        Some(path.join("shader_graph_save.json"))
    } else if path
        .file_name()
        .is_some_and(|name| name == "shader_graph_save.json")
    {
        Some(path.clone())
    } else {
        None
    };
    if let Some(graph_file) = graph_file.filter(|file| file.is_file()) {
        match graph_material_source(&graph_file, std::path::Path::new(&project_root), mirror) {
            Ok((hash, source)) => {
                helio_mats::register_graph_source(hash, source);
                return ResolvedSlotMaterial {
                    surface: slot.imported_surface,
                    material_class: helio_mats::MATERIAL_CLASS_CUSTOM,
                    graph_hash: hash,
                };
            }
            Err(error) => {
                tracing::warn!(entity = entity.index(), path = %graph_file.display(), %error, "could not compile Blueprint material graph; using imported FBX material")
            }
        }
    }
    let loaded = std::fs::read(&path).ok().and_then(|bytes| {
        serde_json::from_slice::<helio_component::components::SurfaceMaterialAsset>(&bytes).ok()
    });
    match loaded {
        Some(material) if material.version == 1 => ResolvedSlotMaterial {
            surface: helio_component::mesh_cache::ImportedSurfaceMaterial {
                base_color: material.base_color,
                roughness: material.roughness,
                metallic: material.metallic,
                emissive: material.emissive_color,
                emissive_intensity: material.emissive_intensity,
                alpha: material.alpha,
            },
            material_class: helio_mats::MATERIAL_CLASS_DEFAULT,
            graph_hash: 0,
        },
        _ => {
            tracing::warn!(
                entity = entity.index(),
                path = %path.display(),
                "static mesh material asset could not be loaded; using the imported FBX material"
            );
            imported()
        }
    }
}

/// Keep SceneDB's `helio::Movability` on `entity` equal to its authored
/// `movability` (Pulsar-Native#837), written only on change so an idle
/// frame stays clean. Passes read the SceneDB component, never the
/// authored property. A mesh's value wins over a light's on the same
/// entity: the mesh is what the caches that read it describe.
pub(crate) fn project_movability(
    world: &mut pulsar_scenedb::World,
    entity: pulsar_scenedb::Entity,
) {
    let authored = world
        .get::<StaticMeshComponent>(entity)
        .map(|mesh| mesh.movability)
        .or_else(|| {
            world
                .get::<helio_component::components::LightComponent>(entity)
                .map(|light| light.general.movability)
        });
    match authored {
        Some(authored) => {
            let promised = helio::Movability::from(authored);
            if world.get::<helio::Movability>(entity) != Some(&promised) {
                world.insert(entity, promised);
            }
        }
        None => {
            world.remove::<helio::Movability>(entity);
        }
    }
}

/// Object-row flags for `mesh`'s authored movability: a movable mesh draws
/// into the dynamic shadow atlas, anything else into the cached static one.
fn object_row_flags(mesh: &StaticMeshComponent) -> u32 {
    if helio::Movability::from(mesh.movability).can_move() {
        helio::INSTANCE_FLAG_MOVABLE
    } else {
        0
    }
}

/// Remove `entity`'s `StaticObjectComponent` row so it stops being drawn.
/// SceneDB zeroes a component's GPU row when the component is removed (or
/// its entity despawns), which the object-batch pass's `mesh_generation != 0`
/// liveness check then treats as dead.
fn retire_static_object_row(world: &mut pulsar_scenedb::World, entity: pulsar_scenedb::Entity) {
    world.remove::<helio_pass_gbuffer::StaticObjectComponent>(entity);
}

/// Retire the renderer rows derived for `entity`. `World::despawn` also
/// clears every `#[gpu]` row the entity carries, so calling this first is
/// no longer required for correctness; it remains the explicit hook for
/// dropping derived rows from an entity that stays alive.
pub fn retire_gpu_rows_for_entity(
    world: &mut pulsar_scenedb::World,
    entity: pulsar_scenedb::Entity,
) {
    retire_static_object_row(world, entity);
}

/// Author the GPU draw rows directly in SceneDB from the live mesh entities.
/// The object-batch pass reads these rows and the mesh ranges from the same
/// SceneDB mirror; no renderer object table or CPU frame cache is involved.
pub fn sync_static_mesh_rows(
    scene_db: &mut pulsar_scenedb::SceneDb,
    dirty: Option<&HashSet<pulsar_scenedb::Entity>>,
) {
    profiling::profile_scope!("HelioBridge::sync_static_mesh_rows");
    if dirty.is_none() {
        let stale: Vec<_> = scene_db
            .world
            .query::<&EditorMeshRow>()
            .filter(|(entity, _)| scene_db.world.get::<StaticMeshComponent>(*entity).is_none())
            .map(|(entity, _)| entity)
            .collect();
        for entity in stale {
            retire_section_draw_entities(&mut scene_db.world, entity);
            retire_static_object_row(&mut scene_db.world, entity);
            scene_db.world.remove::<EditorMeshRow>(entity);
            project_movability(&mut scene_db.world, entity);
        }
        let stale_sections: Vec<_> = scene_db
            .world
            .query::<&MeshSectionDraw>()
            .filter(|(_, draw)| {
                !scene_db.world.is_alive(draw.owner)
                    || scene_db
                        .world
                        .get::<StaticMeshComponent>(draw.owner)
                        .is_none()
            })
            .map(|(entity, _)| entity)
            .collect();
        for entity in stale_sections {
            scene_db.world.despawn(entity);
        }
    }
    tracing::debug!(
        mesh_components = scene_db.world.query::<&StaticMeshComponent>().count(),
        object_rows = scene_db
            .world
            .query::<&helio_pass_gbuffer::StaticObjectComponent>()
            .count(),
        "[SceneDB render diagnostics] static sync entered"
    );
    let Some(mirror) = scene_db.world.gpu_mirror().cloned() else {
        return;
    };
    let entities: Vec<_> = dirty.map_or_else(
        || {
            scene_db
                .world
                .query::<&StaticMeshComponent>()
                .map(|(entity, _)| entity)
                .collect()
        },
        |dirty| dirty.iter().copied().collect(),
    );
    for entity in entities {
        if scene_db.world.get::<StaticMeshComponent>(entity).is_none() {
            retire_section_draw_entities(&mut scene_db.world, entity);
            retire_static_object_row(&mut scene_db.world, entity);
            if scene_db.world.get::<EditorMeshRow>(entity).is_some() {
                scene_db.world.remove::<EditorMeshRow>(entity);
                project_movability(&mut scene_db.world, entity);
            }
            continue;
        }
        project_movability(&mut scene_db.world, entity);
        tracing::debug!(
            entity = entity.index(),
            "[SceneDB render diagnostics] evaluating StaticMeshComponent"
        );
        let Some(transform) = scene_db.world.get::<Transform>(entity).copied() else {
            retire_section_draw_entities(&mut scene_db.world, entity);
            retire_static_object_row(&mut scene_db.world, entity);
            continue;
        };
        if scene_db
            .world
            .get::<Visibility>(entity)
            .is_some_and(|v| !v.visible)
        {
            retire_section_draw_entities(&mut scene_db.world, entity);
            retire_static_object_row(&mut scene_db.world, entity);
            continue;
        }
        // Real, geometry-derived local bounds (see `bounds_local`'s doc) --
        // computed once at hydrate time from the mesh's actual vertex
        // positions, not guessed from the transform's scale.
        let (bounds_local, flags, mesh_sections, material_slots) = scene_db
            .world
            .get::<StaticMeshComponent>(entity)
            .map(|c| {
                (
                    c.bounds_local,
                    object_row_flags(c),
                    c.mesh_sections.clone(),
                    c.material_slots.slots.clone(),
                )
            })
            .unwrap_or(([0.0, 0.0, 0.0, 0.5], 0, Vec::new(), Vec::new()));
        let Some(vertices) =
            StaticMeshComponent::vertices_gpu_handle(mirror.store(), entity.index())
                .filter(|r| r.count != 0)
        else {
            retire_section_draw_entities(&mut scene_db.world, entity);
            retire_static_object_row(&mut scene_db.world, entity);
            continue;
        };
        tracing::debug!(
            entity = entity.index(),
            vertex_offset = vertices.offset,
            vertex_count = vertices.count,
            "[SceneDB render diagnostics] vertex range resolved"
        );
        let Some(indices) = StaticMeshComponent::indices_gpu_handle(mirror.store(), entity.index())
            .filter(|r| r.count != 0)
        else {
            retire_section_draw_entities(&mut scene_db.world, entity);
            retire_static_object_row(&mut scene_db.world, entity);
            continue;
        };
        let model = glam::Mat4::from_scale_rotation_translation(
            glam::Vec3::from_array(transform.scale),
            glam::Quat::from_euler(
                glam::EulerRot::YXZ,
                transform.rotation[1].to_radians(),
                transform.rotation[0].to_radians(),
                transform.rotation[2].to_radians(),
            ),
            glam::Vec3::from_array(transform.position),
        );
        // World-space bounding sphere: transform the mesh's local-space
        // bounds center through `model`, and scale the local radius by the
        // largest axis scale factor -- conservative under non-uniform scale
        // (the scaled ellipsoid's farthest extent along its longest axis is
        // `radius * max_scale_component`, so a sphere of that radius fully
        // contains it, even though it isn't the tightest possible bound).
        let local_center =
            glam::Vec3::from_array([bounds_local[0], bounds_local[1], bounds_local[2]]);
        let world_center = model.transform_point3(local_center);
        let world_radius =
            bounds_local[3] * glam::Vec3::from_array(transform.scale).abs().max_element();
        let world_radius = world_radius.max(0.0);
        let sections = if mesh_sections.is_empty() {
            vec![helio_component::mesh_cache::MeshSection {
                first_index: 0,
                index_count: indices.count,
                material_slot: 0,
            }]
        } else {
            mesh_sections
        };
        let section_entities =
            sync_section_draw_entities(&mut scene_db.world, entity, sections.len());
        let render_entities = std::iter::once(entity).chain(section_entities);
        for (section_index, render_entity) in render_entities.enumerate() {
            let Some(section) = sections.get(section_index) else {
                continue;
            };
            let material = material_surface_for_slot(
                material_slots.get(section.material_slot as usize),
                entity,
                &mirror,
            );
            let material_row = helio_pass_gbuffer::MaterialComponent::from_surface(
                [
                    material.surface.base_color[0],
                    material.surface.base_color[1],
                    material.surface.base_color[2],
                ],
                material.surface.alpha,
                material.surface.roughness,
                material.surface.metallic,
                material.surface.emissive,
                material.surface.emissive_intensity,
            );
            if scene_db
                .world
                .get::<helio_pass_gbuffer::MaterialComponent>(render_entity)
                != Some(&material_row)
            {
                scene_db.world.insert(render_entity, material_row);
            }

            let object_row = helio_pass_gbuffer::StaticObjectComponent::new(
                entity.index(),
                entity.generation().wrapping_add(1),
                render_entity.index(),
                render_entity.generation().wrapping_add(1),
                model,
                [world_center.x, world_center.y, world_center.z, world_radius],
                section.index_count,
                indices.offset.saturating_add(section.first_index),
                vertices.offset as i32,
                material.material_class,
                material.graph_hash,
                flags,
            );
            // Write only on change. Every `insert` bumps the SceneDB revision.
            if scene_db
                .world
                .get::<helio_pass_gbuffer::StaticObjectComponent>(render_entity)
                != Some(&object_row)
            {
                scene_db.world.insert(render_entity, object_row);
            }
        }
        if scene_db.world.get::<EditorMeshRow>(entity).is_none() {
            scene_db.world.insert(entity, EditorMeshRow);
        }
    }
}

/// Ensure that the authoritative SceneDB world has one GPU mirror for Helio's
/// renderer and return the shared handle used by RendererBuilder.
///
/// The mirror is attached before renderer construction. Component writes made
/// after attachment are mirrored by SceneDB itself; this helper does not keep
/// a second world representation or perform per-frame synchronization.
///
/// When a world already has a mirror, the existing handle is returned without
/// replacing its device, queue, pools, or residency state. This is required
/// when multiple renderer views share one SceneDB world.
///
/// A world populated before GPU initialization must be re-dispatched once
/// after attachment because SceneDB intentionally does not replay historical
/// writes into a mirror that did not exist yet. The values are copied only for
/// that one-time re-dispatch; they are never retained by the bridge.
pub fn ensure_gpu_mirror(
    scene_db: &mut pulsar_scenedb::SceneDb,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
) -> GpuMirrorHandle {
    if let Some(existing) = scene_db.world.gpu_mirror() {
        return existing.clone();
    }

    let existing_lights: Vec<_> = scene_db
        .world
        .query::<&helio_pass_forward_lit::LightComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();
    let existing_billboards: Vec<_> = scene_db
        .world
        .query::<&helio_pass_billboard::BillboardComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();
    let existing_transforms: Vec<_> = scene_db
        .world
        .query::<&Transform>()
        .map(|(entity, component)| (entity, *component))
        .collect();
    let existing_materials: Vec<_> = scene_db
        .world
        .query::<&helio_pass_gbuffer::MaterialComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();

    let existing_static_meshes: Vec<_> = scene_db
        .world
        .query::<&StaticMeshComponent>()
        .map(|(entity, component)| (entity, component.clone()))
        .collect();
    let existing_decals: Vec<_> = scene_db
        .world
        .query::<&helio_pass_decal::DecalComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();
    let existing_water_volumes: Vec<_> = scene_db
        .world
        .query::<&helio_pass_water_sim::WaterVolumeComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();
    let existing_water_hitboxes: Vec<_> = scene_db
        .world
        .query::<&helio_pass_water_sim::WaterHitboxComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();
    let existing_groups: Vec<_> = scene_db
        .world
        .query::<&helio_pass_gbuffer::RenderGroupComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();
    let existing_sublevels: Vec<_> = scene_db
        .world
        .query::<&helio_pass_gbuffer::SublevelComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();
    let existing_sectioned_objects: Vec<_> = scene_db
        .world
        .query::<&helio_pass_gbuffer::SectionedObjectComponent>()
        .map(|(entity, component)| (entity, *component))
        .collect();

    let ctx = EngineGpuContext::new(device.clone(), queue.clone());
    let gpu_cfg = SceneGpuConfig {
        classes: vec![RegionClassConfig {
            capacity: 256,
            max_resident_cells: 4,
        }],
        tombstone_headroom: 8,
        max_cells_metadata: 16,
    };
    let mut gpu_store = SceneGpuStore::new(&ctx, gpu_cfg);
    // Register before the first write, as in the Cathedral/Billboard demos.
    helio_pass_forward_lit::LightComponent::register_gpu_columns_growable(
        &mut gpu_store,
        helio_pass_forward_lit::MAX_LIGHTS,
        &device,
    );
    helio_pass_billboard::BillboardComponent::register_gpu_columns_growable(
        &mut gpu_store,
        1024,
        &device,
    );

    // These are SceneDB component columns. Capacities are initial capacities
    // only; growable registration remains the sole owner of GPU storage and
    // deduplication behavior.
    StaticMeshComponent::register_gpu_columns_growable(&mut gpu_store, 4096, &device);
    helio_pass_decal::DecalComponent::register_gpu_columns_growable(&mut gpu_store, 256, &device);
    helio_pass_water_sim::WaterVolumeComponent::register_gpu_columns_growable(
        &mut gpu_store,
        64,
        &device,
    );
    helio_pass_water_sim::WaterHitboxComponent::register_gpu_columns_growable(
        &mut gpu_store,
        256,
        &device,
    );
    helio_pass_gbuffer::RenderGroupComponent::register_gpu_columns_growable(
        &mut gpu_store,
        256,
        &device,
    );
    helio_pass_gbuffer::SublevelComponent::register_gpu_columns_growable(
        &mut gpu_store,
        64,
        &device,
    );
    helio_pass_gbuffer::SectionedObjectComponent::register_gpu_columns_growable(
        &mut gpu_store,
        256,
        &device,
    );
    helio_pass_gbuffer::StaticObjectComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        &device,
    );
    helio_pass_gbuffer::MaterialComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4096,
        &device,
    );
    crate::scene::Transform::register_gpu_columns_growable(&mut gpu_store, 1024, &device);
    // The editor viewport's post-process baseline; see `editor_postprocess`.
    helio_pass_postprocess::CameraPostProcessComponent::register_gpu_columns_growable(
        &mut gpu_store,
        4,
        &device,
    );

    // SceneDB owns residency budgets and tier configuration. The bridge only
    // installs project settings while constructing the shared store.
    {
        let streaming = |key: &str| -> Option<engine_state::settings::ConfigValue> {
            engine_state::settings::global_config()
                .get(engine_state::settings::NS_PROJECT, "streaming", key)
                .ok()
        };
        let int_of = |value: Option<engine_state::settings::ConfigValue>| match value {
            Some(engine_state::settings::ConfigValue::Int(value)) => Some(value),
            _ => None,
        };
        let pool_bytes = int_of(streaming("texture_stream_pool_mb"))
            .unwrap_or(512)
            .clamp(64, 16_384) as u64
            * 1024
            * 1024;
        match gpu_store.configure_tiers(
            pulsar_scenedb::gpu::TierConfig {
                vram_budget_bytes: pool_bytes,
                ram_budget_bytes: pool_bytes.max(256 * 1024 * 1024),
            },
            &[],
        ) {
            Ok(()) => tracing::info!(
                "SceneDB tiers configured: VRAM budget {} MiB",
                pool_bytes / 1024 / 1024
            ),
            Err(error) => tracing::warn!("SceneDB tier configuration failed: {error}"),
        }
    }

    let material_texture_limit =
        helio_mats::MaterialBindingConfig::for_device(&device).max_textures;
    let texture_store = Arc::new(std::sync::RwLock::new(
        pulsar_scenedb::gpu::TextureStore::new(material_texture_limit as u32),
    ));
    let mirror = GpuMirrorHandle::new(Arc::new(gpu_store), queue)
        .with_texture_store(texture_store)
        .expect("register SceneDB material texture store");
    scene_db.world.attach_gpu_mirror(mirror.clone());
    crate::scene::install_scenedb_inspector(&mut scene_db.world);

    for (entity, component) in existing_lights {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_billboards {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_transforms {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_materials {
        scene_db.world.insert(entity, component);
    }

    // Re-dispatch existing typed rows exactly once so the newly attached
    // mirror receives their component data. No row is retained after
    // insertion, and no Helio-side copy is created.
    for (entity, component) in existing_static_meshes {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_decals {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_water_volumes {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_water_hitboxes {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_groups {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_sublevels {
        scene_db.world.insert(entity, component);
    }
    for (entity, component) in existing_sectioned_objects {
        scene_db.world.insert(entity, component);
    }

    tracing::info!("SceneDB GPU mirror attached for Helio 3.0");
    mirror
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_scene_does_not_have_a_gpu_mirror() {
        let scene_db = pulsar_scenedb::SceneDb::new();
        assert!(!scene_db.world.has_gpu_mirror());
    }
}
