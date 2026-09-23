//! Runtime owner for bounded, off-frame terrain generation.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use helio_component::VoxelTerrainComponent;
use helio_pass_voxel_mesh::{
    VoxelChunkGenerator, VoxelChunkKey, VoxelDomain, VoxelEntryId, VoxelGenerationJob,
    VoxelGenerationTicket, VoxelGenerationTicketState, VoxelGenerationWorker,
    VoxelGeneratorDescriptor, VoxelGeneratorRegistry, VoxelSourceId, VoxelSourceWriter,
    VoxelTerrainId,
};
use pulsar_scenedb::World;
use serde::Deserialize;

use super::voxel_frame::terrain_entry;

const RECENT_KEYS_LIMIT: usize = 4096;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Parameters {
    base_height: f64,
    amplitude: f64,
    wavelength: f64,
    material_slot: u8,
}

impl Default for Parameters {
    fn default() -> Self {
        Self {
            base_height: 0.0,
            amplitude: 0.0,
            wavelength: 16.0,
            material_slot: 1,
        }
    }
}

struct Runtime {
    descriptor: VoxelGeneratorDescriptor,
    source_revision: u64,
    store_ptr: usize,
    regenerate_existing: bool,
    worker: VoxelGenerationWorker,
    pending: Option<(VoxelGenerationTicket, Vec<VoxelChunkKey>)>,
    recent: HashSet<VoxelChunkKey>,
    recent_order: VecDeque<VoxelChunkKey>,
    error: Option<String>,
}

impl Runtime {
    fn accept_completed(&mut self) -> bool {
        let Some((ticket, keys)) = self.pending.take() else {
            return false;
        };
        match ticket.state() {
            VoxelGenerationTicketState::Pending => {
                self.pending = Some((ticket, keys));
                true
            }
            VoxelGenerationTicketState::Published(_) => {
                for key in keys {
                    if self.recent.insert(key) {
                        self.recent_order.push_back(key);
                    }
                }
                while self.recent_order.len() > RECENT_KEYS_LIMIT {
                    if let Some(key) = self.recent_order.pop_front() {
                        self.recent.remove(&key);
                    }
                }
                false
            }
            VoxelGenerationTicketState::Failed { error, .. } => {
                // Another source may have advanced the same canonical map.
                // A fresh request uses the new revision; all other failures
                // stay visible until the configuration changes.
                if !error.contains("StaleRevision") {
                    self.error = Some(error);
                }
                false
            }
            VoxelGenerationTicketState::Discarded => {
                self.error = Some("terrain generation request was discarded".into());
                false
            }
        }
    }
}

/// Owns one generation worker per live SceneDB terrain row. Workers contain
/// no GPU resources. Removing/replacing a row drops its worker and its queued
/// jobs; an already running job can complete only into its captured store,
/// and revision checks prevent it from overwriting a newer publication.
#[derive(Default)]
pub struct VoxelTerrainGenerationManager {
    registry: VoxelGeneratorRegistry,
    entries: HashMap<VoxelEntryId, Runtime>,
    errors: Vec<String>,
}

impl VoxelTerrainGenerationManager {
    pub fn register_adapter(
        &mut self,
        id: impl Into<String>,
        version: u32,
        adapter: Arc<dyn VoxelChunkGenerator>,
    ) -> Result<(), String> {
        self.registry.register(id, version, adapter)?;
        self.entries.clear();
        Ok(())
    }

    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// The renderer calls this before its idle decision. It copies at most
    /// 125 chunk keys per terrain row and only tries the component's read lock.
    pub fn reconcile(&mut self, world: &World, camera: [f64; 3]) -> bool {
        let mut live = HashSet::new();
        let mut errors = Vec::new();
        let mut pending = false;
        for (entity, component) in world.query::<&VoxelTerrainComponent>() {
            if !component.enabled || component.generator_id.is_empty() {
                continue;
            }
            let id = VoxelEntryId {
                entity_bits: entity.bits(),
                kind: 1,
            };
            let entry = match terrain_entry(world, entity, component) {
                Ok(entry) => entry,
                Err(error) => {
                    errors.push(format!("terrain {}: {error}", entity.bits()));
                    continue;
                }
            };
            let descriptor = match descriptor(component, &entry) {
                Ok(descriptor) => descriptor,
                Err(error) => {
                    errors.push(format!("terrain {}: {error}", entity.bits()));
                    continue;
                }
            };
            live.insert(id);
            let store_ptr = Arc::as_ptr(&entry.store) as usize;
            let regenerate_existing = self
                .entries
                .get(&id)
                .is_some_and(|runtime| runtime.store_ptr == store_ptr);
            let replace = self.entries.get(&id).is_none_or(|runtime| {
                runtime.descriptor != descriptor
                    || runtime.source_revision != component.source_revision
                    || runtime.store_ptr != store_ptr
            });
            if replace {
                let writer = VoxelSourceWriter::new(
                    VoxelTerrainId(u128::from(entity.bits())),
                    VoxelSourceId(0),
                    entry.store.clone(),
                );
                match VoxelGenerationWorker::start(writer, self.registry.clone()) {
                    Ok(worker) => {
                        self.entries.insert(
                            id,
                            Runtime {
                                descriptor: descriptor.clone(),
                                source_revision: component.source_revision,
                                store_ptr,
                                regenerate_existing,
                                worker,
                                pending: None,
                                recent: HashSet::new(),
                                recent_order: VecDeque::new(),
                                error: None,
                            },
                        );
                    }
                    Err(error) => {
                        errors.push(format!("terrain {} worker: {error}", entity.bits()));
                        continue;
                    }
                }
            }
            let runtime = self.entries.get_mut(&id).expect("inserted runtime");
            pending |= runtime.accept_completed();
            if let Some(error) = &runtime.error {
                errors.push(format!("terrain {}: {error}", entity.bits()));
                continue;
            }
            if runtime.pending.is_some() {
                continue;
            }
            let state = match entry.store.try_read() {
                Ok(state) => state,
                Err(_) => {
                    pending = true;
                    continue;
                }
            };
            let keys: Vec<_> = visible_keys(&descriptor, camera)
                .into_iter()
                .filter(|key| {
                    !runtime.recent.contains(key)
                        && (runtime.regenerate_existing || !state.1.contains_key(&raw_key(*key)))
                })
                .collect();
            let revision = state.0;
            drop(state);
            if keys.is_empty() {
                continue;
            }
            let job = VoxelGenerationJob {
                terrain: VoxelTerrainId(u128::from(entity.bits())),
                source: VoxelSourceId(0),
                expected_revision: revision,
                descriptor,
                keys: keys.clone(),
            };
            match runtime.worker.try_submit(job) {
                Ok(ticket) => {
                    runtime.pending = Some((ticket, keys));
                    pending = true;
                }
                Err(helio_pass_voxel_mesh::VoxelGenerationAdmissionError::Full) => pending = true,
                Err(error) => {
                    let message = format!("generation admission: {error:?}");
                    runtime.error = Some(message.clone());
                    errors.push(format!("terrain {}: {message}", entity.bits()));
                }
            }
        }
        self.entries.retain(|id, _| live.contains(id));
        self.errors = errors;
        pending
    }
}

fn descriptor(
    component: &VoxelTerrainComponent,
    entry: &helio_pass_voxel_mesh::VoxelSceneEntry,
) -> Result<VoxelGeneratorDescriptor, String> {
    let parameters: Parameters = if component.generator_parameters.trim().is_empty() {
        Parameters::default()
    } else {
        serde_json::from_str(&component.generator_parameters)
            .map_err(|error| format!("invalid generator_parameters JSON: {error}"))?
    };
    if usize::from(parameters.material_slot) > component.material_ids.len() {
        return Err("generator material_slot exceeds the SceneDB material palette".into());
    }
    let descriptor = VoxelGeneratorDescriptor {
        id: component.generator_id.clone(),
        version: component.generator_version,
        seed: component.seed,
        shape_mode: component.shape_mode,
        domain: entry.domain,
        origin: entry.origin,
        voxel_size: entry.voxel_size,
        planet_radius: component.planet_radius * (entry.voxel_size / component.voxel_size),
        base_height: parameters.base_height,
        amplitude: parameters.amplitude,
        wavelength: parameters.wavelength,
        material_slot: parameters.material_slot,
    };
    descriptor.validate()?;
    Ok(descriptor)
}

fn raw_key(key: VoxelChunkKey) -> [u64; 4] {
    [key.x as u64, key.y as u64, key.z as u64, u64::from(key.lod)]
}

fn chunk_coord(world: f64, origin: f64, voxel_size: f64) -> i64 {
    ((world - origin) / (voxel_size * 8.0))
        .floor()
        .clamp(i64::MIN as f64, i64::MAX as f64) as i64
}

fn visible_keys(descriptor: &VoxelGeneratorDescriptor, camera: [f64; 3]) -> Vec<VoxelChunkKey> {
    let center = if descriptor.shape_mode == 0 {
        [
            chunk_coord(camera[0], descriptor.origin[0], descriptor.voxel_size),
            chunk_coord(
                descriptor.origin[1] + descriptor.base_height,
                descriptor.origin[1],
                descriptor.voxel_size,
            ),
            chunk_coord(camera[2], descriptor.origin[2], descriptor.voxel_size),
        ]
    } else {
        let delta = std::array::from_fn::<_, 3, _>(|axis| camera[axis] - descriptor.origin[axis]);
        let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
        let direction = if length.is_finite() && length > 0.0 {
            delta.map(|v| v / length)
        } else {
            [0.0, 1.0, 0.0]
        };
        std::array::from_fn(|axis| {
            chunk_coord(
                descriptor.origin[axis] + direction[axis] * descriptor.planet_radius,
                descriptor.origin[axis],
                descriptor.voxel_size,
            )
        })
    };
    let mut keys = Vec::with_capacity(125);
    for dz in -2..=2i64 {
        for dy in -2..=2i64 {
            for dx in -2..=2i64 {
                let Some(x) = center[0].checked_add(dx) else {
                    continue;
                };
                let Some(y) = center[1].checked_add(dy) else {
                    continue;
                };
                let Some(z) = center[2].checked_add(dz) else {
                    continue;
                };
                let key = VoxelChunkKey::new(x, y, z, 0);
                if descriptor.domain.validate_key(key).is_ok() {
                    keys.push(key);
                }
            }
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_terrain_generates_into_scenedb_without_frame_thread_publication() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, VoxelTerrainComponent::default());
        let store = world
            .get::<VoxelTerrainComponent>(entity)
            .unwrap()
            .payload_store();
        let mut manager = VoxelTerrainGenerationManager::default();
        assert!(manager.reconcile(&world, [0.0, 5.0, 0.0]));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            manager.reconcile(&world, [0.0, 5.0, 0.0]);
            if store.read().unwrap().0 > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let state = store.read().unwrap();
        assert!(state.0 > 0);
        assert!(!state.1.is_empty());
        assert!(state.1.contains_key(&[0, u64::MAX, 0, 0]));
        drop(state);
        assert!(manager.errors().is_empty(), "{:?}", manager.errors());
        {
            let mut terrain = world.get_mut::<VoxelTerrainComponent>(entity).unwrap();
            terrain.generator_parameters = r#"{"base_height":-16.0}"#.into();
            terrain.source_revision += 1;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            manager.reconcile(&world, [0.0, 5.0, 0.0]);
            if store.read().unwrap().0 >= 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let state = store.read().unwrap();
        assert!(
            state.0 >= 2,
            "changed descriptor must publish a new revision"
        );
        assert!(
            !state.1.contains_key(&[0, u64::MAX, 0, 0]),
            "regeneration must replace old solid chunks with known air"
        );
        drop(state);
        world.remove::<VoxelTerrainComponent>(entity);
        manager.reconcile(&world, [0.0; 3]);
        assert!(manager.entries.is_empty());
    }

    #[test]
    fn unsupported_shape_and_bad_parameters_report_errors() {
        let mut world = World::new();
        let entity = world.spawn();
        let mut component = VoxelTerrainComponent::default();
        component.shape_mode = 9;
        world.insert(entity, component);
        let mut manager = VoxelTerrainGenerationManager::default();
        assert!(!manager.reconcile(&world, [0.0; 3]));
        assert_eq!(manager.errors().len(), 1);
    }

    #[test]
    fn bounded_planet_mode_publishes_only_chunks_in_its_domain() {
        let mut world = World::new();
        let entity = world.spawn();
        let mut terrain = VoxelTerrainComponent::default();
        terrain.generator_id = helio_pass_voxel_mesh::VOXEL_PLANET_GENERATOR.into();
        terrain.shape_mode = 1;
        terrain.domain_mode = 0;
        terrain.planet_radius = 12.0;
        terrain.bounds_min_x = -24.0;
        terrain.bounds_min_y = -24.0;
        terrain.bounds_min_z = -24.0;
        terrain.bounds_max_x = 24.0;
        terrain.bounds_max_y = 24.0;
        terrain.bounds_max_z = 24.0;
        let store = terrain.payload_store();
        world.insert(entity, terrain);
        let mut manager = VoxelTerrainGenerationManager::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            manager.reconcile(&world, [0.0, 16.0, 20.0]);
            if store.read().unwrap().0 > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let state = store.read().unwrap();
        assert!(state.0 > 0, "bounded planet generation did not publish");
        assert!(
            !state.1.is_empty(),
            "bounded planet should include visible solid chunks"
        );
        assert!(state.1.keys().all(|key| {
            [0, 1, 2].into_iter().all(|axis| {
                let coordinate = key[axis] as i64;
                (-3..=2).contains(&coordinate)
            })
        }));
        assert!(manager.errors().is_empty(), "{:?}", manager.errors());
    }
}
