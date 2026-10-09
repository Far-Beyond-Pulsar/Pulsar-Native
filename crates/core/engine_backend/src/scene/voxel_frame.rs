//! SceneDB voxel row projection for renderer-independent voxel sources.
//!
//! This copies configuration and Arc capabilities only. Canonical payload
//! bytes remain in component rows and are selected by the consuming backend.

use helio_component::{VoxelComponent, VoxelTerrainComponent, VoxelWorldShape};
use helio_voxel_data::VoxelEditJournal;
use helio_voxel_data::{
    VoxelBatchRevision, VoxelChunkBatch, VoxelChunkKey, VoxelChunkOp, VoxelChunkPayload,
    VoxelChunkUpdate, VoxelDomain, VoxelGeneratorDescriptor, VoxelPayloadStore, VoxelSourceId,
    VoxelSourceWriter, VoxelTerrainId, VOXEL_CHUNK_ENCODING_RAW, VOXEL_CHUNK_SCHEMA_VERSION,
};
use pulsar_scene_model::attachments;
use pulsar_scenedb::{Entity, World};

use crate::scene::{Transform, Visibility};

/// SceneDB entity bits include its generation; kind distinguishes source rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VoxelEntryId {
    pub entity_bits: u64,
    pub kind: u8,
}

/// CPU description of one live SceneDB voxel source, independent of a renderer.
#[derive(Clone)]
pub struct VoxelSceneEntry {
    pub appearance_parameters: String,
    pub id: VoxelEntryId,
    /// Editor visibility is independent of whether this source owns the
    /// camera environment (for example a planet's atmosphere).
    pub visible: bool,
    pub store: VoxelPayloadStore,
    pub domain: VoxelDomain,
    pub source_revision: u64,
    pub editable: bool,
    /// World position of the source's sample origin. A terrain must sit
    /// unrotated with a uniform scale, which `voxel_size` and the world
    /// form include. A free-standing object ([`Self::is_object`]) may be
    /// rotated and scaled freely: its renderer places it with the owner's
    /// whole transform, so its `voxel_size` is the edge in the owner's
    /// local space.
    pub origin: [f64; 3],
    pub voxel_size: f64,
    /// Logical width of a chunk address at LOD zero, in base voxel units.
    /// The payload format determines how that region is represented.
    pub chunk_edge_voxels: u32,
    pub lod_scale: u32,
    /// Opaque renderer selection, independent of the generation recipe.
    pub renderer_id: String,
    pub material_ids: Vec<u32>,
    pub generator: Option<VoxelGeneratorConfig>,
    pub initial_cube: Option<VoxelCubeInit>,
    /// Form and size of a terrain world (scaled with the entity).
    pub world: VoxelWorldForm,
    /// The terrain's ordered brush journal.
    pub edits: VoxelEditJournal,
}

/// Authored form of a voxel world: shape and size in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelWorldForm {
    pub shape: VoxelWorldShape,
    pub planet_radius: f64,
    pub plane_size: f64,
}

impl Default for VoxelWorldForm {
    fn default() -> Self {
        let terrain = VoxelTerrainComponent::default();
        Self {
            shape: terrain.shape,
            planet_radius: terrain.planet_radius,
            plane_size: terrain.plane_size,
        }
    }
}

/// Authored generator identity and parameters passed to the specialized voxel
/// source scheduler. Executable generator code is registered outside SceneDB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoxelGeneratorConfig {
    pub id: String,
    pub version: u32,
    pub seed: u64,
    pub parameters: String,
}

impl VoxelSceneEntry {
    /// A free-standing `VoxelComponent` (not a terrain): a bounded volume
    /// of live chunks, drawn at and with its owner's transform.
    pub fn is_object(&self) -> bool {
        self.id.kind == 0
    }

    /// Build the CPU generator input from the reflected terrain configuration.
    /// The registered generator interprets `parameters` and produces a
    /// format-tagged payload; no engine-side material-cell conversion occurs.
    pub fn generator_descriptor(&self) -> Option<VoxelGeneratorDescriptor> {
        let generator = self.generator.as_ref()?;
        Some(VoxelGeneratorDescriptor {
            id: generator.id.clone(),
            version: generator.version,
            seed: generator.seed,
            domain: self.domain,
            origin: self.origin,
            voxel_size: self.voxel_size,
            chunk_edge_voxels: self.chunk_edge_voxels,
            lod_scale: self.lod_scale,
            parameters: generator.parameters.clone(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VoxelCubeInit {
    pub dimensions: [u32; 3],
    pub material_slot: u8,
}

/// The LOD-zero chunks of an initial cube: `dimensions` samples from the
/// origin filled with `material_slot`, edge chunks padded with air.
pub fn initial_cube_chunks(init: VoxelCubeInit) -> Vec<(VoxelChunkKey, [u8; 512])> {
    let mut chunks = Vec::new();
    for z in 0..init.dimensions[2].div_ceil(8) {
        for y in 0..init.dimensions[1].div_ceil(8) {
            for x in 0..init.dimensions[0].div_ceil(8) {
                let mut samples = [0u8; 512];
                for lz in 0..8 {
                    for ly in 0..8 {
                        for lx in 0..8 {
                            if x * 8 + lx < init.dimensions[0]
                                && y * 8 + ly < init.dimensions[1]
                                && z * 8 + lz < init.dimensions[2]
                            {
                                samples[(lz * 64 + ly * 8 + lx) as usize] = init.material_slot;
                            }
                        }
                    }
                }
                chunks.push((
                    VoxelChunkKey::new(i64::from(x), i64::from(y), i64::from(z), 0),
                    samples,
                ));
            }
        }
    }
    chunks
}

/// Initialize a deserialized object before source edits. This runs off-frame.
pub fn initialize_empty_cube(entry: &VoxelSceneEntry) -> Result<bool, String> {
    let Some(init) = entry.initial_cube else {
        return Ok(false);
    };
    let state = entry
        .store
        .read()
        .map_err(|_| "voxel store lock poisoned".to_string())?;
    let uninitialized = state.0 == 0 && state.1.is_empty();
    drop(state);
    if !uninitialized {
        return Ok(false);
    }
    let writer = VoxelSourceWriter::new(
        VoxelTerrainId(u128::from(entry.id.entity_bits)),
        VoxelSourceId(0),
        entry.store.clone(),
    );
    if init.dimensions.iter().any(|&size| size == 0 || size > 256)
        || init.material_slot == 0
        || usize::from(init.material_slot) > entry.material_ids.len()
    {
        return Err("initial cube dimensions or material slot are invalid".into());
    }
    let chunks = initial_cube_chunks(init);
    let ops: Vec<_> = chunks
        .iter()
        .map(|(key, samples)| {
            VoxelChunkOp::Upsert(VoxelChunkUpdate {
                key: *key,
                payload: VoxelChunkPayload {
                    encoding: VOXEL_CHUNK_ENCODING_RAW,
                    schema_version: VOXEL_CHUNK_SCHEMA_VERSION,
                    bytes: samples,
                },
            })
        })
        .collect();
    match writer.publish_batch(&VoxelChunkBatch {
        terrain: VoxelTerrainId(u128::from(entry.id.entity_bits)),
        source: VoxelSourceId(0),
        revision: VoxelBatchRevision {
            expected: 0,
            publish: 1,
        },
        domain: entry.domain,
        ops: &ops,
    }) {
        Ok(_) => Ok(true),
        Err(error) => {
            let state = entry
                .store
                .read()
                .map_err(|_| "voxel store lock poisoned".to_string())?;
            if state.0 > 0 || !state.1.is_empty() {
                Ok(false)
            } else {
                Err(format!("initial cube publication failed: {error:?}"))
            }
        }
    }
}

/// Fingerprint of the ground a terrain entry's generator makes (0 without
/// a generator): its edits belong to it (`VoxelEditJournal::made_on`).
pub fn terrain_fingerprint(entry: &VoxelSceneEntry) -> u64 {
    let Some(generator) = &entry.generator else { return 0 };
    helio_component::voxel_world::world_recipe(
        entry.world.shape,
        entry.world.planet_radius,
        entry.world.plane_size,
        entry.voxel_size,
        helio_pass_voxel_planet::TerrainSource {
            generator: generator.id.clone(),
            version: generator.version,
            seed: generator.seed,
            settings: generator.parameters.clone(),
        },
    )
    .fingerprint()
}

/// Edits belong to the terrain they were made on: a terrain whose form,
/// generator, seed or settings changed drops the edits made on the old one
/// (and saves without them). Returns how many edits were dropped.
pub fn sync_edit_journals(world: &mut World) -> usize {
    let stale: Vec<(Entity, u64)> = attachments::enabled_components::<VoxelTerrainComponent>(world)
        .filter_map(|(instance, _, component)| {
            let fingerprint = terrain_fingerprint(&terrain_entry(world, instance, component).ok()?);
            (fingerprint != 0 && component.edits.terrain() != fingerprint).then_some((instance, fingerprint))
        })
        .collect();
    let mut dropped = 0;
    for (entity, fingerprint) in stale {
        if let Some(mut terrain) = world.get_mut::<VoxelTerrainComponent>(entity) {
            let count = terrain.edits.made_on(fingerprint);
            if count > 0 {
                tracing::info!("Voxel terrain {}: its ground changed; {count} edits made on the old ground were dropped", entity.bits());
                terrain.source_revision = terrain.source_revision.wrapping_add(1);
            }
            dropped += count;
        }
    }
    dropped
}

/// Runs [`sync_edit_journals`] when a terrain or its layer settings changed:
/// change cursors over both, so frames that change neither cost nothing.
pub struct EditJournalSync {
    cursors: [pulsar_scenedb::ChangeCursor; 2],
    /// The world revision at the last poll; a smaller one means the world
    /// was replaced, and the cursors with it.
    revision: u64,
    synced: bool,
    scratch: Vec<pulsar_scenedb::ComponentChange>,
}

impl EditJournalSync {
    pub fn new(world: &World) -> Self {
        Self {
            cursors: [
                world.open_change_cursor::<VoxelTerrainComponent>(),
                world.open_change_cursor::<helio_component::VoxelTerrainLayersComponent>(),
            ],
            revision: world.revision(),
            synced: false,
            scratch: Vec::new(),
        }
    }

    /// Drops edits made on ground a terrain no longer has, when anything
    /// that shapes the ground changed since the last poll. Returns how many
    /// edits were dropped.
    pub fn poll(&mut self, world: &mut World) -> usize {
        if world.revision() < self.revision {
            *self = Self::new(world);
        }
        self.revision = world.revision();
        let mut changed = !self.synced;
        for cursor in &mut self.cursors {
            self.scratch.clear();
            changed |= world.read_changes(cursor, &mut self.scratch)
                == pulsar_scenedb::ChangeRead::Overflowed
                || !self.scratch.is_empty();
        }
        if !changed {
            return 0;
        }
        self.synced = true;
        sync_edit_journals(world)
    }
}

pub fn project_voxel_entries(world: &World) -> (Vec<VoxelSceneEntry>, Vec<String>) {
    profiling::profile_scope!("voxel_project_entries");
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    for (instance, owner, component) in attachments::enabled_components::<VoxelComponent>(world) {
        if !component.enabled {
            continue;
        }
        match object_entry(world, instance, component) {
            Ok(mut entry) => {
                entry.visible = world.get::<Visibility>(owner).is_none_or(|v| v.visible);
                entries.push(entry);
            }
            Err(error) => errors.push(format!("voxel object {}: {error}", instance.bits())),
        }
    }
    for (instance, owner, component) in
        attachments::enabled_components::<VoxelTerrainComponent>(world)
    {
        if !component.enabled {
            continue;
        }
        match terrain_entry(world, instance, component) {
            Ok(mut entry) => {
                entry.visible = world.get::<Visibility>(owner).is_none_or(|v| v.visible);
                entries.push(entry);
            }
            Err(error) => errors.push(format!("voxel terrain {}: {error}", instance.bits())),
        }
    }
    (entries, errors)
}

/// World origin and uniform scale of `instance`'s owner object.
fn origin_scale(world: &World, instance: Entity) -> Result<([f64; 3], f64), &'static str> {
    let transform = attachments::owner_component::<Transform>(world, instance)
        .copied()
        .unwrap_or_default();
    if transform
        .rotation
        .iter()
        .any(|v| !v.is_finite() || v.abs() > 1.0e-5)
    {
        return Err("rotated voxel source transforms are unsupported");
    }
    let [sx, sy, sz] = transform.scale;
    if !sx.is_finite() || sx <= 0.0 || (sx - sy).abs() > 1.0e-5 || (sx - sz).abs() > 1.0e-5 {
        return Err("voxel transforms require finite positive uniform scale");
    }
    if transform.position.iter().any(|v| !v.is_finite()) {
        return Err("voxel transform position must be finite");
    }
    Ok((transform.position.map(f64::from), f64::from(sx)))
}

/// World position of `instance`'s owner object. Its rotation and scale
/// (finite, positive on every axis) place a free-standing volume too, but
/// the renderer applies those itself.
fn object_origin(world: &World, instance: Entity) -> Result<[f64; 3], &'static str> {
    let transform = attachments::owner_component::<Transform>(world, instance)
        .copied()
        .unwrap_or_default();
    if transform.rotation.iter().any(|v| !v.is_finite()) {
        return Err("voxel object rotation must be finite");
    }
    if transform.scale.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return Err("voxel object scale must be finite and positive on every axis");
    }
    if transform.position.iter().any(|v| !v.is_finite()) {
        return Err("voxel transform position must be finite");
    }
    Ok(transform.position.map(f64::from))
}

pub(super) fn object_entry(
    world: &World,
    entity: Entity,
    component: &VoxelComponent,
) -> Result<VoxelSceneEntry, &'static str> {
    let origin = object_origin(world, entity)?;
    if component.dimensions.iter().any(|&size| size == 0) {
        return Err("dimensions must be positive");
    }
    if component.dimensions.iter().any(|&size| size > 256) {
        return Err("cube dimensions exceed the supported 256 voxels per axis");
    }
    if !component.voxel_size.is_finite() || component.voxel_size <= 0.0 {
        return Err("voxel_size must be finite and positive");
    }
    if component.material_ids.len() > 255 {
        return Err("voxel material palette exceeds 255 IDs");
    }
    if component.default_material_slot == 0
        || component.default_material_slot as usize > component.material_ids.len()
    {
        return Err("default_material_slot must name a material in the palette");
    }
    let max = component.dimensions.map(|size| i64::from((size - 1) / 8));
    Ok(VoxelSceneEntry {
        id: VoxelEntryId {
            entity_bits: entity.bits(),
            kind: 0,
        },
        visible: true,
        store: component.payload_store(),
        domain: VoxelDomain::Bounded {
            min: [0; 3],
            max,
            max_lod: 0,
        },
        source_revision: 0,
        editable: component.editable,
        origin,
        voxel_size: component.voxel_size,
        chunk_edge_voxels: 8,
        lod_scale: 1,
        renderer_id: component.renderer_id.clone(),
        material_ids: component.material_ids.clone(),
        generator: None,
        initial_cube: Some(VoxelCubeInit {
            dimensions: component.dimensions,
            material_slot: u8::try_from(component.default_material_slot)
                .map_err(|_| "default_material_slot must fit in one byte")?,
        }),
        world: VoxelWorldForm::default(),
        appearance_parameters: String::new(),
        edits: VoxelEditJournal::default(),
    })
}

pub(super) fn terrain_entry(
    world: &World,
    entity: Entity,
    component: &VoxelTerrainComponent,
) -> Result<VoxelSceneEntry, &'static str> {
    let (origin, scale) = origin_scale(world, entity)?;
    if !component.voxel_size.is_finite() || component.voxel_size <= 0.0 {
        return Err("voxel_size must be finite and positive");
    }
    if component.chunk_edge_voxels == 0 {
        return Err("chunk_edge_voxels must be positive");
    }
    if component.lod_scale == 0 {
        return Err("lod_scale must be positive");
    }
    let max_lod = u8::try_from(component.max_chunk_lod)
        .map_err(|_| "max_chunk_lod must fit in a chunk key")?;
    let voxel_size = component.voxel_size * scale;
    let chunk_size = voxel_size * f64::from(component.chunk_edge_voxels);
    if !chunk_size.is_finite() || chunk_size <= 0.0 {
        return Err("chunk span must be finite and positive");
    }
    let domain = match component.domain_mode {
        0 => {
            let min_world = [
                component.bounds_min_x,
                component.bounds_min_y,
                component.bounds_min_z,
            ];
            let max_world = [
                component.bounds_max_x,
                component.bounds_max_y,
                component.bounds_max_z,
            ];
            if (0..3).any(|axis| {
                !min_world[axis].is_finite()
                    || !max_world[axis].is_finite()
                    || min_world[axis] >= max_world[axis]
            }) {
                return Err("bounded terrain requires finite increasing bounds");
            }
            let min = std::array::from_fn(|axis| {
                ((min_world[axis] - origin[axis]) / chunk_size).floor() as i64
            });
            let max = std::array::from_fn(|axis| {
                (((max_world[axis] - origin[axis]) / chunk_size).ceil() as i64).saturating_sub(1)
            });
            VoxelDomain::BoundedBase {
                min,
                max,
                max_lod,
                lod_scale: component.lod_scale,
            }
        }
        1 => VoxelDomain::Unbounded { max_lod },
        _ => return Err("domain_mode must be bounded (0) or unbounded (1)"),
    };
    Ok(VoxelSceneEntry {
        id: VoxelEntryId {
            entity_bits: entity.bits(),
            kind: 1,
        },
        visible: true,
        store: component.payload_store(),
        domain,
        source_revision: component.source_revision,
        editable: component.editable,
        origin,
        voxel_size,
        chunk_edge_voxels: component.chunk_edge_voxels,
        lod_scale: component.lod_scale,
        renderer_id: component.renderer_id.clone(),
        material_ids: component.material_ids.clone(),
        generator: (!component.generator.id.is_empty()).then(|| VoxelGeneratorConfig {
            id: component.generator.id.clone(),
            version: component.generator.version,
            seed: component.seed,
            parameters: helio_component::voxel_world::generator_settings(world, entity, component),
        }),
        initial_cube: None,
        world: VoxelWorldForm {
            shape: component.shape,
            planet_radius: component.planet_radius * scale,
            plane_size: component.plane_size * scale,
        },
        appearance_parameters: component.appearance_parameters.clone(),
        edits: component.edits.clone(),
    })
    .map(|mut entry| {
        // Edits made on other ground are not this terrain's (the scene step
        // drops them from the journal: `sync_edit_journals`).
        let fingerprint = terrain_fingerprint(&entry);
        if entry.edits.terrain() != 0 && entry.edits.terrain() != fingerprint {
            entry.edits = VoxelEditJournal::default();
            entry.edits.made_on(fingerprint);
        }
        entry
    })
}

/// A fresh object with one attached, enabled `class_name` instance (value
/// not yet inserted); returns the instance entity.
#[cfg(test)]
pub(super) fn spawn_test_instance(world: &mut World, class_name: &str) -> Entity {
    let owner = world.spawn();
    attachments::spawn_instance(
        world,
        owner,
        pulsar_scene_model::NewInstance::new(class_name),
    )
    .expect("spawn test instance")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_projection_preserves_source_contract_and_uses_authored_chunk_span() {
        let mut world = World::new();
        let entity = world.spawn();
        let mut component = VoxelTerrainComponent::default();
        component.domain_mode = 0;
        component.bounds_max_x = 64.0;
        component.bounds_max_y = 64.0;
        component.bounds_max_z = 64.0;
        component.voxel_size = 1.0;
        component.chunk_edge_voxels = 32;
        component.max_chunk_lod = 4;
        component.lod_scale = 3;
        component.renderer_id = "test.renderer".into();
        component.generator.id = "test.world".into();
        component.generator.version = 7;
        component.seed = 42;
        component.generator_parameters = "{\"biome\":1}".into();
        component.material_ids = vec![0; 300];
        world.insert(entity, component);

        let entry = terrain_entry(&world, entity, world.get(entity).unwrap()).unwrap();
        assert_eq!(entry.chunk_edge_voxels, 32);
        assert_eq!(entry.renderer_id, "test.renderer");
        assert_eq!(
            entry.domain,
            VoxelDomain::BoundedBase {
                min: [0; 3],
                max: [1; 3],
                max_lod: 4,
                lod_scale: 3,
            }
        );
        assert!(entry
            .domain
            .validate_key(VoxelChunkKey::new(0, 0, 0, 1))
            .is_ok());
        assert!(entry
            .domain
            .validate_key(VoxelChunkKey::new(1, 0, 0, 1))
            .is_err());
        assert_eq!(
            entry.generator,
            Some(VoxelGeneratorConfig {
                id: "test.world".into(),
                version: 7,
                seed: 42,
                parameters: "{\"biome\":1}".into(),
            })
        );
        let descriptor = entry.generator_descriptor().unwrap();
        assert_eq!(descriptor.id, "test.world");
        assert_eq!(descriptor.chunk_edge_voxels, 32);
        assert_eq!(descriptor.lod_scale, 3);
        assert_eq!(descriptor.parameters, "{\"biome\":1}");

        {
            let mut component = world.get_mut::<VoxelTerrainComponent>(entity).unwrap();
            component.chunk_edge_voxels = 2;
            component.max_chunk_lod = 0;
        }
        let entry = terrain_entry(&world, entity, world.get(entity).unwrap()).unwrap();
        assert_eq!(entry.chunk_edge_voxels, 2);
        assert_eq!(
            entry.domain,
            VoxelDomain::BoundedBase {
                min: [0; 3],
                max: [31; 3],
                max_lod: 0,
                lod_scale: 3,
            }
        );
    }

    #[test]
    fn projects_multiple_rows_with_generation_identity_and_rejects_bad_domains() {
        let mut world = World::new();
        let object = spawn_test_instance(&mut world, "VoxelComponent");
        world.insert(object, VoxelComponent::default());
        let terrain = spawn_test_instance(&mut world, "VoxelTerrainComponent");
        world.insert(terrain, VoxelTerrainComponent::default());
        let (entries, errors) = project_voxel_entries(&world);
        assert!(errors.is_empty());
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|entry| entry.id
            == VoxelEntryId {
                entity_bits: object.bits(),
                kind: 0
            }));
        assert!(entries.iter().any(|entry| entry.id
            == VoxelEntryId {
                entity_bits: terrain.bits(),
                kind: 1
            }));
        world
            .get_mut::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .domain_mode = 99;
        let (entries, errors) = project_voxel_entries(&world);
        assert_eq!(entries.len(), 1);
        assert_eq!(errors.len(), 1);
        world
            .get_mut::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .domain_mode = 1;
        world
            .get_mut::<VoxelComponent>(object)
            .unwrap()
            .default_material_slot = 0;
        let (entries, errors) = project_voxel_entries(&world);
        assert_eq!(entries.len(), 1);
        assert!(errors[0].contains("default_material_slot"));
    }

    /// Edits belong to the ground they were made on: changing the seed or
    /// the layer stack drops them; the form's unused size does not.
    #[test]
    fn edits_are_dropped_when_the_ground_changes() {
        use helio_component::{VoxelLayerKind, VoxelTerrainLayer, VoxelTerrainLayersComponent};
        let mut world = World::new();
        let object = world.spawn();
        let entity =
            pulsar_world_registry::attach_value(&mut world, object, VoxelTerrainComponent::planet(1_000.0))
                .unwrap();
        let layers =
            pulsar_world_registry::attach_value(&mut world, object, VoxelTerrainLayersComponent::default())
                .unwrap();
        let edit = helio_voxel_data::VoxelBrushEdit {
            center: [0.0, 1_000.0, 0.0],
            radius: 1.0,
            shape: helio_voxel_data::VoxelBrushShape::Sphere,
            op: helio_voxel_data::VoxelBrushOp::Remove,
            material: 0,
        };
        let edits = |world: &World| world.get::<VoxelTerrainComponent>(entity).unwrap().edits.len();
        let projected = |world: &World| project_voxel_entries(world).0[0].edits.len();
        world.get_mut::<VoxelTerrainComponent>(entity).unwrap().edits.push(edit);
        // A new journal adopts the terrain it is on.
        assert_eq!(sync_edit_journals(&mut world), 0);
        assert_eq!((edits(&world), projected(&world)), (1, 1));
        world.get_mut::<VoxelTerrainComponent>(entity).unwrap().plane_size = 77.0;
        assert_eq!(sync_edit_journals(&mut world), 0, "a planet does not use the plane size");
        // Another seed: other ground. The projection already shows none.
        world.get_mut::<VoxelTerrainComponent>(entity).unwrap().seed += 1;
        assert_eq!(projected(&world), 0);
        assert_eq!(sync_edit_journals(&mut world), 1);
        assert_eq!(edits(&world), 0);
        // So does another layer stack.
        world.get_mut::<VoxelTerrainComponent>(entity).unwrap().edits.push(edit);
        assert_eq!(sync_edit_journals(&mut world), 0);
        world.get_mut::<VoxelTerrainLayersComponent>(layers).unwrap().stack.layers.push(VoxelTerrainLayer::new(VoxelLayerKind::Craters));
        assert_eq!(sync_edit_journals(&mut world), 1);
        assert_eq!((edits(&world), projected(&world)), (0, 0));
    }

    #[test]
    fn journals_are_checked_only_when_a_terrain_or_its_layers_change() {
        use helio_component::{VoxelLayerKind, VoxelTerrainLayer, VoxelTerrainLayersComponent};
        let mut world = World::new();
        let object = world.spawn();
        let terrain =
            pulsar_world_registry::attach_value(&mut world, object, VoxelTerrainComponent::planet(1_000.0))
                .unwrap();
        let layers =
            pulsar_world_registry::attach_value(&mut world, object, VoxelTerrainLayersComponent::default())
                .unwrap();
        let mut sync = EditJournalSync::new(&world);
        let edit = helio_voxel_data::VoxelBrushEdit {
            center: [0.0, 1_000.0, 0.0],
            radius: 1.0,
            shape: helio_voxel_data::VoxelBrushShape::Sphere,
            op: helio_voxel_data::VoxelBrushOp::Remove,
            material: 0,
        };
        world.get_mut::<VoxelTerrainComponent>(terrain).unwrap().edits.push(edit);
        assert_eq!(sync.poll(&mut world), 0, "the journal adopts its ground");
        assert_eq!(sync.poll(&mut world), 0);
        // Another stack is other ground: its edits go.
        world.get_mut::<VoxelTerrainLayersComponent>(layers).unwrap().stack.layers.push(VoxelTerrainLayer::new(VoxelLayerKind::Craters));
        assert_eq!(sync.poll(&mut world), 1);
        assert_eq!(world.get::<VoxelTerrainComponent>(terrain).unwrap().edits.len(), 0);
        // Dropping them wrote the terrain; the re-check finds nothing more.
        assert_eq!(sync.poll(&mut world), 0);
        world.get_mut::<VoxelTerrainComponent>(terrain).unwrap().edits.push(edit);
        assert_eq!(sync.poll(&mut world), 0, "edits on the current ground stay");
    }

    #[test]
    fn default_terrain_has_a_registered_generator_and_valid_parameters() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, VoxelTerrainComponent::default());

        let entry = terrain_entry(&world, entity, world.get(entity).unwrap()).unwrap();
        let descriptor = entry.generator_descriptor().unwrap();
        assert_eq!(descriptor.id, helio_voxel_data::VOXEL_TERRAIN_GENERATOR);
        assert_eq!(
            descriptor.version,
            helio_voxel_data::VOXEL_TERRAIN_GENERATOR_VERSION
        );
        assert_eq!(entry.world.shape, VoxelWorldShape::Plane);
        assert_eq!(entry.voxel_size, 0.1);
    }

    #[test]
    fn presets_set_the_world_form() {
        let mut world = World::new();
        for (terrain, shape) in [
            (
                VoxelTerrainComponent::planet(1_000.0),
                VoxelWorldShape::Sphere,
            ),
            (VoxelTerrainComponent::plane(512.0), VoxelWorldShape::Plane),
            (
                VoxelTerrainComponent::infinite_plane(),
                VoxelWorldShape::InfinitePlane,
            ),
        ] {
            let entity = world.spawn();
            world.insert(entity, terrain);
            let entry = terrain_entry(&world, entity, world.get(entity).unwrap()).unwrap();
            assert_eq!(entry.world.shape, shape);
            assert_eq!(
                entry.generator_descriptor().unwrap().id,
                helio_voxel_data::VOXEL_TERRAIN_GENERATOR
            );
        }
    }
}
