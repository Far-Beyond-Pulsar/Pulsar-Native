//! A mesh's slot materials as the materials' instances.
//!
//! Each slot of a mesh draws with a material instance (`helio_mats::instance`):
//! an entity of the materials' table, shared by every slot drawing the same
//! thing. The slot's source is its explicit surface override, else the
//! material asset it names (its own assignment or the mesh asset's default),
//! falling back to the mesh's imported surface. [`MeshMaterialInstances`]
//! (a scene subsystem) keeps each mesh's instances in step with its slots:
//! it reads the meshes' change journal, acquires the instances a changed mesh
//! needs, releases the ones it no longer does, and gives the mesh's entity a
//! [`MeshMaterialRows`] (the slots' table rows, mirrored to the GPU, which
//! the scene join reads per section) and a [`MeshMaterials`] (the instances).
//! The mesh component itself is never written: its draw rows name a slot,
//! so assigning or reloading a material costs that small row, not the mesh's
//! geometry and draw rows. The mesh never resolves, binds or uploads a
//! material.

use std::collections::HashMap;

use helio_mats::asset::MATERIAL_ASSET_KIND;
use helio_mats::instance::{MaterialSource, acquire, release, row_of};
use pulsar_scenedb::{ChangeCursor, ChangeRead, ComponentChange, ComponentChangeKind, Entity, World};
use pulsar_scenedb_derive::SceneStore;

use crate::components::{SkeletalMeshComponent, StaticMeshComponent, StaticMeshMaterialSlot};

/// Pool of every mesh's slot table rows; its handle table is
/// `mesh_material_rows::handles`, keyed by the mesh instance entity.
pub const MESH_MATERIAL_ROWS_BUFFER: &str = "mesh_material_rows";

/// A mesh's slots' rows of the materials' table, in slot order: a section
/// draws with `rows[section.material_slot]`. A mesh has none until its
/// instances exist (the frame it was added), and the scene join skips a
/// section without one.
#[derive(SceneStore, Clone, Debug, Default, PartialEq)]
pub struct MeshMaterialRows {
    #[gpu(buffer = "mesh_material_rows")]
    pub rows: Vec<u32>,
}

/// The material instances a mesh's slots hold, in slot order (what
/// [`MeshMaterialRows`] names on the GPU), for what reads the materials on
/// the CPU (whether a mesh's surface animates).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshMaterials(pub Vec<Entity>);

/// What `slot` draws: its surface override, else the material asset it
/// names (declared so the world loads it), with the imported surface as the
/// fallback.
pub fn slot_source(slot: &StaticMeshMaterialSlot) -> MaterialSource {
    if let Some(surface) = slot.surface_override {
        return MaterialSource::surface(surface);
    }
    let asset = helio_component::asset_component::asset_source(slot.effective_material_asset())
        .map(|source| {
            helio_component::asset_component::declared_asset_id(MATERIAL_ASSET_KIND, &source);
            source
        })
        .unwrap_or_default();
    MaterialSource {
        asset,
        surface: None,
        fallback: slot.imported_surface,
    }
}

/// The asset id of the material `path` names, without declaring it.
pub fn material_asset_id_of(path: &str) -> pulsar_scenedb::AssetId {
    helio_component::asset_component::asset_source(path)
        .map_or(pulsar_scenedb::AssetId::ZERO, |source| {
            pulsar_scenedb::AssetId::of(MATERIAL_ASSET_KIND, &source)
        })
}

/// What `slot` draws outside a world (tools such as thumbnails and the asset
/// viewer): its material asset is loaded now, on the caller's thread, with
/// the materials plugin's loader, and resolved in `mirror`'s scene if given.
/// An empty slot, or one whose material cannot be loaded (or no enabled
/// plugin loads), draws its fallback (with a warning for a failed load). In
/// a world, the slot's instance resolves instead.
pub fn resolve_slot_material(
    slot: Option<&StaticMeshMaterialSlot>,
    mirror: Option<&pulsar_scenedb::gpu::GpuMirrorHandle>,
) -> helio_mats::asset::ResolvedMaterial {
    use helio_mats::asset::{MaterialPayload, ResolvedMaterial};
    let source = slot.map_or_else(default_source, slot_source);
    if let Some(surface) = &source.surface {
        return ResolvedMaterial::surface(surface);
    }
    let id = source.asset_id();
    if id.is_zero() {
        return ResolvedMaterial::surface(&source.fallback);
    }
    let loaded = match helio_component::asset_component::load_asset_now(MATERIAL_ASSET_KIND, &source.asset) {
        Ok(payload) => payload.downcast::<MaterialPayload>().ok(),
        Err(error) => {
            tracing::warn!(source = %source.asset, %error, "static mesh material could not be loaded; using the imported material");
            None
        }
    };
    loaded
        .and_then(|material| material.0.resolve(id, 0, mirror, &source.fallback).ok())
        .unwrap_or_else(|| ResolvedMaterial::surface(&source.fallback))
}

/// What a mesh with no slots draws.
fn default_source() -> MaterialSource {
    MaterialSource::surface(Default::default())
}

/// The sources a static mesh's slots draw, one per slot (one default for a
/// mesh without slots, whose single section uses slot 0).
fn static_sources(mesh: &StaticMeshComponent) -> Vec<MaterialSource> {
    if mesh.material_slots.slots.is_empty() {
        return vec![default_source()];
    }
    mesh.material_slots.slots.iter().map(slot_source).collect()
}


/// The meshes' instances, kept in step with their slots.
#[derive(Default)]
pub struct MeshMaterialInstances {
    cursors: Option<(ChangeCursor, ChangeCursor)>,
    scratch: Vec<ComponentChange>,
    /// Per mesh instance entity: the sources it holds and their instances.
    held: HashMap<Entity, (Vec<MaterialSource>, Vec<Entity>)>,
}

impl MeshMaterialInstances {
    pub const NAME: &'static str = "pulsar.meshes.material_instances";

    pub fn step(&mut self, world: &mut World) {
        let rescan = match &mut self.cursors {
            None => {
                self.cursors = Some((
                    world.open_change_cursor::<StaticMeshComponent>(),
                    world.open_change_cursor::<SkeletalMeshComponent>(),
                ));
                true
            }
            Some((statics, skeletals)) => {
                self.scratch.clear();
                let a = world.read_changes(statics, &mut self.scratch);
                let b = world.read_changes(skeletals, &mut self.scratch);
                matches!(a, ChangeRead::Overflowed) || matches!(b, ChangeRead::Overflowed)
            }
        };
        let changed: Vec<(Entity, bool)> = if rescan {
            let mut all: Vec<(Entity, bool)> = world
                .query::<&StaticMeshComponent>()
                .map(|(entity, _)| (entity, true))
                .chain(
                    world
                        .query::<&SkeletalMeshComponent>()
                        .map(|(entity, _)| (entity, true)),
                )
                .collect();
            // Meshes gone since: release what they held.
            all.extend(
                self.held
                    .keys()
                    .filter(|entity| !world.is_alive(**entity))
                    .map(|entity| (*entity, false)),
            );
            all
        } else {
            self.scratch
                .iter()
                .map(|change| (change.entity, change.kind != ComponentChangeKind::Removed))
                .collect()
        };
        for (entity, present) in changed {
            self.sync(world, entity, present);
        }
    }

    fn sync(&mut self, world: &mut World, entity: Entity, present: bool) {
        let sources = if !present {
            None
        } else if let Some(mesh) = world.get::<StaticMeshComponent>(entity) {
            Some(static_sources(mesh))
        } else if world.get::<SkeletalMeshComponent>(entity).is_some() {
            // Skeletal meshes have no slots yet: every section draws the
            // default surface.
            Some(vec![default_source()])
        } else {
            None
        };
        let Some(sources) = sources else {
            if let Some((_, instances)) = self.held.remove(&entity) {
                for instance in instances {
                    release(world, instance);
                }
            }
            // A removed mesh component leaves its entity drawing nothing.
            if world.is_alive(entity) {
                world.remove::<MeshMaterialRows>(entity);
                world.remove::<MeshMaterials>(entity);
            }
            return;
        };
        if self
            .held
            .get(&entity)
            .is_some_and(|(held, instances)| *held == sources && instances.iter().all(|i| world.is_alive(*i)))
        {
            return;
        }
        let instances: Vec<Entity> = sources.iter().map(|source| acquire(world, source)).collect();
        if let Some((_, old)) = self.held.insert(entity, (sources, instances.clone())) {
            for instance in old {
                release(world, instance);
            }
        }
        let rows = MeshMaterialRows {
            rows: instances.iter().map(|instance| row_of(*instance)).collect(),
        };
        if world.get::<MeshMaterialRows>(entity) != Some(&rows) {
            world.insert(entity, rows);
        }
        if world.get::<MeshMaterials>(entity).map(|held| &held.0) != Some(&instances) {
            world.insert(entity, MeshMaterials(instances));
        }
    }
}

impl pulsar_scenedb::Subsystem for MeshMaterialInstances {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn simulate_a(&mut self, world: &mut World, _witness: &pulsar_scenedb::gpu::SimulateA) {
        self.step(world);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

pulsar_world_registry::inventory::submit! {
    pulsar_world_registry::SceneSubsystemRegistration {
        origin: pulsar_plugins::origin!(),
        name: MeshMaterialInstances::NAME,
        install: |scene| scene.register_subsystem(MeshMaterialInstances::default()),
    }
}
