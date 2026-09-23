//! SceneDB voxel row projection for renderer-independent voxel sources.
//!
//! This copies configuration and Arc capabilities only. Canonical payload
//! bytes remain in component rows and are selected by the consuming backend.

use helio_component::{VoxelComponent, VoxelTerrainComponent};
use helio_voxel_data::{
    VoxelBatchRevision, VoxelChunkBatch, VoxelChunkKey, VoxelChunkOp, VoxelChunkPayload,
    VoxelChunkUpdate, VoxelDomain, VoxelPayloadStore, VoxelSourceId, VoxelSourceWriter,
    VoxelTerrainId, VOXEL_CHUNK_ENCODING_RAW, VOXEL_CHUNK_SCHEMA_VERSION,
};
use pulsar_scenedb::{Entity, World};

use crate::scene::Transform;

/// SceneDB entity bits include its generation; kind distinguishes source rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VoxelEntryId {
    pub entity_bits: u64,
    pub kind: u8,
}

/// CPU description of one live SceneDB voxel source, independent of a renderer.
#[derive(Clone)]
pub struct VoxelSceneEntry {
    pub id: VoxelEntryId,
    pub store: VoxelPayloadStore,
    pub domain: VoxelDomain,
    pub source_revision: u64,
    pub origin: [f64; 3],
    pub voxel_size: f64,
    pub material_ids: Vec<u32>,
    pub initial_cube: Option<VoxelCubeInit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VoxelCubeInit {
    pub dimensions: [u32; 3],
    pub material_slot: u8,
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

pub fn project_voxel_entries(world: &World) -> (Vec<VoxelSceneEntry>, Vec<String>) {
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    for (entity, component) in world.query::<&VoxelComponent>() {
        if !component.enabled {
            continue;
        }
        match object_entry(world, entity, component) {
            Ok(entry) => entries.push(entry),
            Err(error) => errors.push(format!("voxel object {}: {error}", entity.bits())),
        }
    }
    for (entity, component) in world.query::<&VoxelTerrainComponent>() {
        if !component.enabled {
            continue;
        }
        match terrain_entry(world, entity, component) {
            Ok(entry) => entries.push(entry),
            Err(error) => errors.push(format!("voxel terrain {}: {error}", entity.bits())),
        }
    }
    (entries, errors)
}

fn origin_scale(world: &World, entity: Entity) -> Result<([f64; 3], f64), &'static str> {
    let transform = world.get::<Transform>(entity).copied().unwrap_or_default();
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

pub(super) fn object_entry(
    world: &World,
    entity: Entity,
    component: &VoxelComponent,
) -> Result<VoxelSceneEntry, &'static str> {
    let (origin, scale) = origin_scale(world, entity)?;
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
        store: component.payload_store(),
        domain: VoxelDomain::Bounded {
            min: [0; 3],
            max,
            max_lod: 0,
        },
        source_revision: 0,
        origin,
        voxel_size: component.voxel_size * scale,
        material_ids: component.material_ids.clone(),
        initial_cube: Some(VoxelCubeInit {
            dimensions: component.dimensions,
            material_slot: u8::try_from(component.default_material_slot)
                .map_err(|_| "default_material_slot must fit in one byte")?,
        }),
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
    if component.material_ids.len() > 255 {
        return Err("voxel material palette exceeds 255 IDs");
    }
    if component.shape_mode > 1 {
        return Err("shape_mode must be plane (0) or planet (1)");
    }
    if component.shape_mode == 1
        && (!component.planet_radius.is_finite() || component.planet_radius <= 0.0)
    {
        return Err("planet_radius must be finite and positive");
    }
    let voxel_size = component.voxel_size * scale;
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
            let chunk_size = voxel_size * 8.0;
            let min = std::array::from_fn(|axis| {
                ((min_world[axis] - origin[axis]) / chunk_size).floor() as i64
            });
            let max = std::array::from_fn(|axis| {
                (((max_world[axis] - origin[axis]) / chunk_size).ceil() as i64).saturating_sub(1)
            });
            VoxelDomain::Bounded {
                min,
                max,
                max_lod: 16,
            }
        }
        1 => VoxelDomain::Unbounded { max_lod: 16 },
        _ => return Err("domain_mode must be bounded (0) or unbounded (1)"),
    };
    Ok(VoxelSceneEntry {
        id: VoxelEntryId {
            entity_bits: entity.bits(),
            kind: 1,
        },
        store: component.payload_store(),
        domain,
        source_revision: component.source_revision,
        origin,
        voxel_size,
        material_ids: component.material_ids.clone(),
        initial_cube: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_multiple_rows_with_generation_identity_and_rejects_bad_modes() {
        let mut world = World::new();
        let object = world.spawn();
        world.insert(object, VoxelComponent::default());
        let terrain = world.spawn();
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
            .shape_mode = 99;
        let (entries, errors) = project_voxel_entries(&world);
        assert_eq!(entries.len(), 1);
        assert_eq!(errors.len(), 1);
        world
            .get_mut::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .shape_mode = 0;
        world
            .get_mut::<VoxelComponent>(object)
            .unwrap()
            .default_material_slot = 0;
        let (entries, errors) = project_voxel_entries(&world);
        assert_eq!(entries.len(), 1);
        assert!(errors[0].contains("default_material_slot"));
    }
}
