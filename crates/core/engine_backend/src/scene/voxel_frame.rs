//! SceneDB voxel row projection for renderer-independent voxel sources.
//!
//! A voxel world (`VoxelTerrainComponent`) projects its description: form,
//! generator and edit journal.

use helio_component::{VoxelTerrainComponent, VoxelWorldShape};
use helio_voxel_data::VoxelEditJournal;
use pulsar_scene_model::attachments;
use pulsar_scenedb::{Entity, World};

use crate::scene::{Transform, Visibility};

/// A voxel world's terrain instance entity (its bits include the
/// generation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VoxelEntryId {
    pub entity_bits: u64,
}

/// CPU description of one live SceneDB voxel world, independent of a renderer.
#[derive(Clone)]
pub struct VoxelSceneEntry {
    pub appearance_parameters: String,
    /// Surface smoothing, 0 to 1 (`VoxelTerrainComponent::smoothness`).
    pub smoothness: f32,
    pub id: VoxelEntryId,
    /// Editor visibility is independent of whether this source owns the
    /// camera environment (for example a planet's atmosphere).
    pub visible: bool,
    pub source_revision: u64,
    pub editable: bool,
    /// World position of the world's sample origin. A terrain sits
    /// unrotated with a uniform scale, which `voxel_size` and the world
    /// form include.
    pub origin: [f64; 3],
    pub voxel_size: f64,
    pub generator: Option<VoxelGeneratorConfig>,
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
/// source scheduler, and the features grown on its ground (with its seed).
/// Executable generator code is registered outside SceneDB.
#[derive(Clone, Debug, PartialEq)]
pub struct VoxelGeneratorConfig {
    pub id: String,
    pub version: u32,
    pub seed: u64,
    pub parameters: String,
    pub features: helio_pass_voxel_planet::feature_recipe::FeatureRecipe,
}

/// Fingerprint of the ground a terrain entry's generator and features make
/// (0 without a generator): its edits belong to it
/// (`VoxelEditJournal::made_on`).
pub fn terrain_fingerprint(entry: &VoxelSceneEntry) -> u64 {
    let Some(generator) = &entry.generator else {
        return 0;
    };
    helio_component::voxel_world::ground_fingerprint(
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
        &generator.features,
    )
}

/// Edits belong to the terrain they were made on: a terrain whose form,
/// generator, seed, settings or features changed drops the edits made on the old one
/// (and saves without them). Returns how many edits were dropped.
pub fn sync_edit_journals(world: &mut World) -> usize {
    let stale: Vec<(Entity, u64)> = attachments::enabled_components::<VoxelTerrainComponent>(world)
        .filter_map(|(instance, _, component)| {
            let fingerprint = terrain_fingerprint(&terrain_entry(world, instance, component).ok()?);
            (fingerprint != 0 && component.edits.terrain() != fingerprint)
                .then_some((instance, fingerprint))
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

/// A voxel world's sky and haze are its air: an atmosphere or global fog
/// on the entity of a voxel terrain sits on that terrain's ground, so
/// resizing the planet or turning it into a plane takes them along. A
/// sphere's atmosphere is centred on it at its radius; a plane's is flat
/// ground at the origin, at the Earth's radius unless it was already flat
/// (its curvature is the author's). A sphere's fog measures heights over
/// it, a plane's along world Y. A world whose `air` is a preset (not
/// `Custom`) derives the rest of both from it too
/// ([`AtmosphereComponent::for_air`], [`GlobalFogComponent::for_air`]):
/// an airless moon has neither; the sky's clouds stay as authored. Returns
/// how many atmospheres and fogs changed.
///
/// [`AtmosphereComponent::for_air`]: helio_component::AtmosphereComponent::for_air
/// [`GlobalFogComponent::for_air`]: helio_component::GlobalFogComponent::for_air
pub fn sync_air_to_terrains(world: &mut World) -> usize {
    sync_skies(world) + sync_hazes(world)
}

/// The skies of [`sync_air_to_terrains`].
fn sync_skies(world: &mut World) -> usize {
    use helio_component::{AtmosphereComponent, AtmospherePlacement};
    let world_ref: &World = world;
    let changed: Vec<(Entity, AtmosphereComponent)> =
        attachments::enabled_components::<VoxelTerrainComponent>(world_ref)
            .filter_map(|(instance, owner, component)| {
                Some((
                    owner,
                    component,
                    terrain_entry(world_ref, instance, component).ok()?.world,
                ))
            })
            .flat_map(|(owner, terrain, form)| {
                attachments::enabled_components_of::<AtmosphereComponent>(world_ref, owner)
                    .into_iter()
                    .filter_map(move |(sky, atmosphere)| {
                        let (placement, radius_km) = match form.shape {
                            VoxelWorldShape::Sphere => (
                                AtmospherePlacement::PlanetAtOwner,
                                (form.planet_radius / 1000.0) as f32,
                            ),
                            _ if atmosphere.placement == AtmospherePlacement::GroundAtOrigin => (
                                AtmospherePlacement::GroundAtOrigin,
                                atmosphere.planet_radius_km,
                            ),
                            _ => (
                                AtmospherePlacement::GroundAtOrigin,
                                AtmosphereComponent::default().planet_radius_km,
                            ),
                        };
                        let placed = AtmosphereComponent {
                            placement,
                            planet_radius_km: radius_km,
                            ..atmosphere.clone()
                        };
                        let wanted = placed
                            .for_air(
                                terrain.air,
                                terrain.air_density,
                                terrain.air_tint,
                                terrain.air_haze,
                            )
                            .unwrap_or(placed);
                        (!same_sky(atmosphere, &wanted)).then_some((sky, wanted))
                    })
            })
            .collect();
    for (sky, wanted) in &changed {
        if let Some(mut atmosphere) = world.get_mut::<helio_component::AtmosphereComponent>(*sky) {
            *atmosphere = wanted.clone();
        }
    }
    changed.len()
}

/// The hazes of [`sync_air_to_terrains`].
fn sync_hazes(world: &mut World) -> usize {
    use helio_component::GlobalFogComponent;
    let world_ref: &World = world;
    let changed: Vec<(Entity, GlobalFogComponent)> =
        attachments::enabled_components::<VoxelTerrainComponent>(world_ref)
            .filter_map(|(instance, owner, component)| {
                Some((
                    owner,
                    component,
                    terrain_entry(world_ref, instance, component).ok()?.world,
                ))
            })
            .flat_map(|(owner, terrain, form)| {
                attachments::enabled_components_of::<GlobalFogComponent>(world_ref, owner)
                    .into_iter()
                    .filter_map(move |(fog, medium)| {
                        let mut placed = medium.clone();
                        placed.medium.planet_center = [0.0; 3];
                        placed.medium.planet_radius = match form.shape {
                            VoxelWorldShape::Sphere => form.planet_radius as f32,
                            _ => 0.0,
                        };
                        let wanted = placed
                            .for_air(terrain.air, terrain.air_density, terrain.air_haze)
                            .unwrap_or(placed);
                        let same = medium.enabled == wanted.enabled
                            && medium.medium.to_medium() == wanted.medium.to_medium();
                        (!same).then_some((fog, wanted))
                    })
            })
            .collect();
    for (fog, wanted) in &changed {
        if let Some(mut medium) = world.get_mut::<GlobalFogComponent>(*fog) {
            *medium = wanted.clone();
        }
    }
    changed.len()
}

/// Whether two skies are the same air (every property).
fn same_sky(
    a: &helio_component::AtmosphereComponent,
    b: &helio_component::AtmosphereComponent,
) -> bool {
    a.enabled == b.enabled && a.placement == b.placement && a.to_row() == b.to_row()
}

/// Keeps voxel worlds' dependents in step with them when a terrain, its
/// layer settings, an atmosphere or a fog changed (change cursors, so
/// frames that change none cost nothing): edits made on ground a terrain no
/// longer has are dropped ([`sync_edit_journals`]) and skies and hazes
/// follow their terrain ([`sync_air_to_terrains`]).
pub struct VoxelWorldSync {
    cursors: [pulsar_scenedb::ChangeCursor; 4],
    /// The world revision at the last poll; a smaller one means the world
    /// was replaced, and the cursors with it.
    revision: u64,
    synced: bool,
    scratch: Vec<pulsar_scenedb::ComponentChange>,
}

impl VoxelWorldSync {
    pub fn new(world: &World) -> Self {
        Self {
            cursors: [
                world.open_change_cursor::<VoxelTerrainComponent>(),
                world.open_change_cursor::<helio_component::VoxelTerrainLayersComponent>(),
                world.open_change_cursor::<helio_component::AtmosphereComponent>(),
                world.open_change_cursor::<helio_component::GlobalFogComponent>(),
            ],
            revision: world.revision(),
            synced: false,
            scratch: Vec::new(),
        }
    }

    /// Drops edits made on ground a terrain no longer has and moves skies
    /// and hazes onto their terrain, when anything they depend on changed since the
    /// last poll. Returns how many edits were dropped.
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
        let dropped = sync_edit_journals(world);
        sync_air_to_terrains(world);
        // Its own writes are not changes to answer next poll (an overflow
        // leaves the cursor at the newest entry all the same).
        for cursor in &mut self.cursors {
            self.scratch.clear();
            let _ = world.read_changes(cursor, &mut self.scratch);
        }
        dropped
    }
}

pub fn project_voxel_entries(world: &World) -> (Vec<VoxelSceneEntry>, Vec<String>) {
    profiling::profile_scope!("voxel_project_entries");
    let mut entries = Vec::new();
    let mut errors = Vec::new();
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

pub(super) fn terrain_entry(
    world: &World,
    entity: Entity,
    component: &VoxelTerrainComponent,
) -> Result<VoxelSceneEntry, &'static str> {
    let (origin, scale) = origin_scale(world, entity)?;
    if !component.voxel_size.is_finite() || component.voxel_size <= 0.0 {
        return Err("voxel_size must be finite and positive");
    }
    let voxel_size = component.voxel_size * scale;
    // Where a slow projection's time goes (`VOXEL_ENTRIES slow`).
    let started = std::time::Instant::now();
    let generator = (!component.generator.id.is_empty()).then(|| {
        let parameters = helio_component::voxel_world::generator_settings(world, entity, component);
        let settings_ms = started.elapsed().as_secs_f64() * 1e3;
        let features = helio_component::feature_recipe(&component.features);
        let features_ms = started.elapsed().as_secs_f64() * 1e3 - settings_ms;
        (
            VoxelGeneratorConfig {
                id: component.generator.id.clone(),
                version: component.generator.version,
                seed: component.seed,
                parameters,
                features,
            },
            settings_ms,
            features_ms,
        )
    });
    let (generator, settings_ms, features_ms) = match generator {
        Some((generator, settings_ms, features_ms)) => (Some(generator), settings_ms, features_ms),
        None => (None, 0.0, 0.0),
    };
    Ok(VoxelSceneEntry {
        id: VoxelEntryId {
            entity_bits: entity.bits(),
        },
        visible: true,
        source_revision: component.source_revision,
        editable: component.editable,
        origin,
        voxel_size,
        generator,
        world: VoxelWorldForm {
            shape: component.shape,
            planet_radius: component.planet_radius * scale,
            plane_size: component.plane_size * scale,
        },
        appearance_parameters: component.appearance_parameters.clone(),
        smoothness: component.smoothness,
        edits: component.edits.clone(),
    })
    .map(|mut entry| {
        // Edits made on other ground are not this terrain's (the scene step
        // drops them from the journal: `sync_edit_journals`).
        let fingerprinted = std::time::Instant::now();
        let fingerprint = terrain_fingerprint(&entry);
        if entry.edits.terrain() != 0 && entry.edits.terrain() != fingerprint {
            entry.edits = VoxelEditJournal::default();
            entry.edits.made_on(fingerprint);
        }
        let ms = started.elapsed().as_secs_f64() * 1e3;
        if ms >= 5.0 {
            tracing::warn!(
                "VOXEL_ENTRIES slow: terrain {} in {ms:.1} ms (settings {settings_ms:.1}, features {features_ms:.1}, fingerprint {:.1})",
                entity.bits(),
                fingerprinted.elapsed().as_secs_f64() * 1e3
            );
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
    fn a_world_projects_its_description() {
        let mut world = World::new();
        let entity = world.spawn();
        let mut component = VoxelTerrainComponent::planet(1_000.0);
        component.voxel_size = 0.5;
        component.generator.id = "test.world".into();
        component.generator.version = 7;
        component.seed = 42;
        component.generator_parameters = "{\"biome\":1}".into();
        world.insert(entity, component);

        let entry = terrain_entry(&world, entity, world.get(entity).unwrap()).unwrap();
        assert_eq!(
            entry.generator,
            Some(VoxelGeneratorConfig {
                id: "test.world".into(),
                version: 7,
                seed: 42,
                parameters: "{\"biome\":1}".into(),
                features: Default::default(),
            })
        );
        assert_eq!(entry.voxel_size, 0.5);
        assert_eq!(
            (entry.world.shape, entry.world.planet_radius),
            (VoxelWorldShape::Sphere, 1_000.0)
        );
    }

    #[test]
    fn projects_worlds_and_reports_bad_rows() {
        let mut world = World::new();
        let terrain = spawn_test_instance(&mut world, "VoxelTerrainComponent");
        world.insert(terrain, VoxelTerrainComponent::default());
        let (entries, errors) = project_voxel_entries(&world);
        assert!(errors.is_empty());
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].id,
            VoxelEntryId {
                entity_bits: terrain.bits()
            }
        );
        world
            .get_mut::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .voxel_size = f64::NAN;
        let (entries, errors) = project_voxel_entries(&world);
        assert!(entries.is_empty());
        assert!(errors[0].contains("voxel_size"));
    }

    /// Edits belong to the ground they were made on: changing the seed or
    /// the layer stack drops them; the form's unused size does not.
    #[test]
    fn edits_are_dropped_when_the_ground_changes() {
        use helio_component::{VoxelLayerKind, VoxelTerrainLayer, VoxelTerrainLayersComponent};
        let mut world = World::new();
        let object = world.spawn();
        let entity = pulsar_world_registry::attach_value(
            &mut world,
            object,
            VoxelTerrainComponent::planet(1_000.0),
        )
        .unwrap();
        let layers = pulsar_world_registry::attach_value(
            &mut world,
            object,
            VoxelTerrainLayersComponent::default(),
        )
        .unwrap();
        let edit = helio_voxel_data::VoxelBrushEdit {
            center: [0.0, 1_000.0, 0.0],
            radius: 1.0,
            shape: helio_voxel_data::VoxelBrushShape::Sphere,
            op: helio_voxel_data::VoxelBrushOp::Remove,
            material: 0,
            height: Default::default(),
        };
        let edits = |world: &World| {
            world
                .get::<VoxelTerrainComponent>(entity)
                .unwrap()
                .edits
                .len()
        };
        let projected = |world: &World| project_voxel_entries(world).0[0].edits.len();
        world
            .get_mut::<VoxelTerrainComponent>(entity)
            .unwrap()
            .edits
            .push(edit);
        // A new journal adopts the terrain it is on.
        assert_eq!(sync_edit_journals(&mut world), 0);
        assert_eq!((edits(&world), projected(&world)), (1, 1));
        world
            .get_mut::<VoxelTerrainComponent>(entity)
            .unwrap()
            .plane_size = 77.0;
        assert_eq!(
            sync_edit_journals(&mut world),
            0,
            "a planet does not use the plane size"
        );
        // Another seed: other ground. The projection already shows none.
        world.get_mut::<VoxelTerrainComponent>(entity).unwrap().seed += 1;
        assert_eq!(projected(&world), 0);
        assert_eq!(sync_edit_journals(&mut world), 1);
        assert_eq!(edits(&world), 0);
        // So does another layer stack.
        world
            .get_mut::<VoxelTerrainComponent>(entity)
            .unwrap()
            .edits
            .push(edit);
        assert_eq!(sync_edit_journals(&mut world), 0);
        world
            .get_mut::<VoxelTerrainLayersComponent>(layers)
            .unwrap()
            .stack
            .layers
            .push(VoxelTerrainLayer::new(VoxelLayerKind::Craters));
        assert_eq!(sync_edit_journals(&mut world), 1);
        assert_eq!((edits(&world), projected(&world)), (0, 0));
    }

    #[test]
    fn a_terrains_sky_follows_its_size_and_shape() {
        use helio_component::{AtmosphereComponent, AtmospherePlacement};
        let mut world = World::new();
        let object = world.spawn();
        let terrain = pulsar_world_registry::attach_value(
            &mut world,
            object,
            VoxelTerrainComponent::planet(250_000.0),
        )
        .unwrap();
        let sky =
            pulsar_world_registry::attach_value(&mut world, object, AtmosphereComponent::default())
                .unwrap();
        let mut sync = VoxelWorldSync::new(&world);
        let air = |world: &World| {
            let a = world.get::<AtmosphereComponent>(sky).unwrap();
            (a.placement, a.planet_radius_km)
        };
        sync.poll(&mut world);
        assert_eq!(air(&world), (AtmospherePlacement::PlanetAtOwner, 250.0));
        world
            .get_mut::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .planet_radius = 40_000.0;
        sync.poll(&mut world);
        assert_eq!(air(&world), (AtmospherePlacement::PlanetAtOwner, 40.0));
        // A plane's sky is flat ground: the Earth's curvature, not the old planet's.
        world
            .get_mut::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .shape = VoxelWorldShape::Plane;
        sync.poll(&mut world);
        let earth = AtmosphereComponent::default().planet_radius_km;
        assert_eq!(air(&world), (AtmospherePlacement::GroundAtOrigin, earth));
        // A sky edited by hand on a planet goes back onto it.
        world
            .get_mut::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .shape = VoxelWorldShape::Sphere;
        sync.poll(&mut world);
        world
            .get_mut::<AtmosphereComponent>(sky)
            .unwrap()
            .planet_radius_km = 9.0;
        sync.poll(&mut world);
        assert_eq!(air(&world), (AtmospherePlacement::PlanetAtOwner, 40.0));
        assert_eq!(sync_air_to_terrains(&mut world), 0);
    }

    #[test]
    fn a_worlds_air_preset_makes_its_sky() {
        use helio_component::{AtmosphereComponent, VoxelAir};
        let mut world = World::new();
        let object = world.spawn();
        let terrain = pulsar_world_registry::attach_value(
            &mut world,
            object,
            VoxelTerrainComponent::planet(1_737_400.0),
        )
        .unwrap();
        let mut authored = AtmosphereComponent::default();
        authored.mie_anisotropy = 0.1;
        let sky = pulsar_world_registry::attach_value(&mut world, object, authored).unwrap();
        let mut sync = VoxelWorldSync::new(&world);
        let sky_of = |world: &World| world.get::<AtmosphereComponent>(sky).unwrap().clone();
        sync.poll(&mut world);
        assert_eq!(
            sky_of(&world).mie_anisotropy,
            0.1,
            "custom air keeps the authored sky"
        );
        world.get_mut::<VoxelTerrainComponent>(terrain).unwrap().air = VoxelAir::None;
        sync.poll(&mut world);
        assert!(!sky_of(&world).enabled, "a moon has no sky");
        assert_eq!(sky_of(&world).planet_radius_km, 1_737.4);
        world.get_mut::<VoxelTerrainComponent>(terrain).unwrap().air = VoxelAir::Thin;
        sync.poll(&mut world);
        let thin = sky_of(&world);
        assert!(
            thin.enabled
                && thin.rayleigh_scattering[2]
                    < AtmosphereComponent::default().rayleigh_scattering[2]
        );
        // Hand edits to a derived sky go back to the air's.
        world
            .get_mut::<AtmosphereComponent>(sky)
            .unwrap()
            .mie_anisotropy = 0.2;
        sync.poll(&mut world);
        assert_eq!(sky_of(&world).mie_anisotropy, thin.mie_anisotropy);
        world
            .get_mut::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .air_density = 2.0;
        sync.poll(&mut world);
        assert!(
            (sky_of(&world).rayleigh_scattering[0] - thin.rayleigh_scattering[0] * 2.0).abs()
                < 1e-9
        );
        assert_eq!(sync_air_to_terrains(&mut world), 0);
    }

    #[test]
    fn a_worlds_air_preset_makes_its_haze() {
        use helio_component::{GlobalFogComponent, VoxelAir};
        let mut world = World::new();
        let object = world.spawn();
        let mut planet = VoxelTerrainComponent::planet(6_371_000.0);
        planet.air = VoxelAir::Earthlike;
        let terrain = pulsar_world_registry::attach_value(&mut world, object, planet).unwrap();
        let fog =
            pulsar_world_registry::attach_value(&mut world, object, GlobalFogComponent::default())
                .unwrap();
        let mut sync = VoxelWorldSync::new(&world);
        let haze = |world: &World| world.get::<GlobalFogComponent>(fog).unwrap().clone();
        sync.poll(&mut world);
        let earth = helio_pass_volumetric_fog::GlobalFogComponent {
            mode: 1,
            extinction: 0.0006,
            height_falloff: 0.004,
            anisotropy: 0.6,
            ..Default::default()
        }
        .around_planet([0.0; 3], 6_371_000.0);
        assert_eq!(
            haze(&world).medium.to_medium(),
            earth,
            "Earth's air, over the planet"
        );
        world
            .get_mut::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .air_haze = 2.0;
        sync.poll(&mut world);
        assert!((haze(&world).medium.extinction - 0.0012).abs() < 1e-9);
        world.get_mut::<VoxelTerrainComponent>(terrain).unwrap().air = VoxelAir::None;
        sync.poll(&mut world);
        assert!(!haze(&world).enabled, "a moon has no haze");
        // Custom air keeps an authored medium, placed on the ground.
        world.get_mut::<VoxelTerrainComponent>(terrain).unwrap().air = VoxelAir::Custom;
        world
            .get_mut::<GlobalFogComponent>(fog)
            .unwrap()
            .medium
            .albedo = [0.9, 0.8, 0.7];
        sync.poll(&mut world);
        assert_eq!(haze(&world).medium.albedo, [0.9, 0.8, 0.7]);
        assert_eq!(haze(&world).medium.planet_radius, 6_371_000.0);
        world
            .get_mut::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .shape = VoxelWorldShape::Plane;
        sync.poll(&mut world);
        assert_eq!(
            haze(&world).medium.planet_radius,
            0.0,
            "a plane's heights are along Y"
        );
        assert_eq!(sync_air_to_terrains(&mut world), 0);
    }

    /// Features are part of the ground: the entry carries their recipe,
    /// and edits made before a change to them are dropped.
    #[test]
    fn features_are_part_of_the_ground() {
        use helio_pass_voxel_planet::feature_recipe::FeatureRecipe;
        let mut world = World::new();
        let object = world.spawn();
        let terrain = pulsar_world_registry::attach_value(
            &mut world,
            object,
            VoxelTerrainComponent::planet(1_000.0),
        )
        .unwrap();
        let entry =
            |world: &World| terrain_entry(world, terrain, world.get(terrain).unwrap()).unwrap();
        let bare = terrain_fingerprint(&entry(&world));
        world
            .get_mut::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .features = helio_component::feature_spawns(&FeatureRecipe::moon());
        let moon = entry(&world);
        assert_eq!(
            moon.generator.as_ref().unwrap().features,
            FeatureRecipe::moon()
        );
        assert_ne!(terrain_fingerprint(&moon), bare);
        let mut sync = VoxelWorldSync::new(&world);
        world
            .get_mut::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .edits
            .push(helio_voxel_data::VoxelBrushEdit {
                center: [0.0, 1_000.0, 0.0],
                radius: 1.0,
                shape: helio_voxel_data::VoxelBrushShape::Sphere,
                op: helio_voxel_data::VoxelBrushOp::Remove,
                material: 0,
                height: Default::default(),
            });
        assert_eq!(sync.poll(&mut world), 0, "the journal adopts its ground");
        world
            .get_mut::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .features[0]
            .density = 0.9;
        assert_eq!(sync.poll(&mut world), 1, "other features are other ground");
    }

    #[test]
    fn journals_are_checked_only_when_a_terrain_or_its_layers_change() {
        use helio_component::{VoxelLayerKind, VoxelTerrainLayer, VoxelTerrainLayersComponent};
        let mut world = World::new();
        let object = world.spawn();
        let terrain = pulsar_world_registry::attach_value(
            &mut world,
            object,
            VoxelTerrainComponent::planet(1_000.0),
        )
        .unwrap();
        let layers = pulsar_world_registry::attach_value(
            &mut world,
            object,
            VoxelTerrainLayersComponent::default(),
        )
        .unwrap();
        let mut sync = VoxelWorldSync::new(&world);
        let edit = helio_voxel_data::VoxelBrushEdit {
            center: [0.0, 1_000.0, 0.0],
            radius: 1.0,
            shape: helio_voxel_data::VoxelBrushShape::Sphere,
            op: helio_voxel_data::VoxelBrushOp::Remove,
            material: 0,
            height: Default::default(),
        };
        world
            .get_mut::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .edits
            .push(edit);
        assert_eq!(sync.poll(&mut world), 0, "the journal adopts its ground");
        assert_eq!(sync.poll(&mut world), 0);
        // Another stack is other ground: its edits go.
        world
            .get_mut::<VoxelTerrainLayersComponent>(layers)
            .unwrap()
            .stack
            .layers
            .push(VoxelTerrainLayer::new(VoxelLayerKind::Craters));
        assert_eq!(sync.poll(&mut world), 1);
        assert_eq!(
            world
                .get::<VoxelTerrainComponent>(terrain)
                .unwrap()
                .edits
                .len(),
            0
        );
        // Dropping them wrote the terrain; the re-check finds nothing more.
        assert_eq!(sync.poll(&mut world), 0);
        world
            .get_mut::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .edits
            .push(edit);
        assert_eq!(sync.poll(&mut world), 0, "edits on the current ground stay");
    }

    #[test]
    fn default_terrain_has_a_registered_generator_and_valid_parameters() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, VoxelTerrainComponent::default());

        let entry = terrain_entry(&world, entity, world.get(entity).unwrap()).unwrap();
        let generator = entry.generator.unwrap();
        assert_eq!(generator.id, helio_voxel_data::VOXEL_TERRAIN_GENERATOR);
        assert_eq!(
            generator.version,
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
                entry.generator.unwrap().id,
                helio_voxel_data::VOXEL_TERRAIN_GENERATOR
            );
        }
    }
}
