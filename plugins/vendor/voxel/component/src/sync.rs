//! A voxel terrain's dependents follow it: edits belong to the ground they
//! were made on, and a world's sky and haze are its air. [`VoxelWorldSync`]
//! keeps both in step from the scene's change journals; the terrains'
//! scene subsystem ([`crate::runtime::VoxelWorlds`]) polls it.

use pulsar_scene_model::attachments;
use pulsar_scenedb::{Entity, World};

use crate::world::{WorldSource, entity_source};
use crate::{VoxelTerrainComponent, VoxelTerrainLayersComponent, VoxelWorldShape};
use helio_pass_voxel_planet::grid::Shape;

/// Edits belong to the terrain they were made on: a terrain whose form,
/// generator, seed, settings or features changed drops the edits made on
/// the old one (and saves without them). Returns how many edits were
/// dropped.
pub fn sync_edit_journals(world: &mut World) -> usize {
    let stale: Vec<(Entity, u64)> = attachments::enabled_components::<VoxelTerrainComponent>(world)
        .filter(|(_, _, component)| !component.generator.id.is_empty())
        .filter_map(|(instance, _, component)| {
            let fingerprint = entity_source(world, instance, component)
                .ok()?
                .fingerprint();
            (component.edits.terrain() != fingerprint).then_some((instance, fingerprint))
        })
        .collect();
    let mut dropped = 0;
    for (entity, fingerprint) in stale {
        if let Some(mut terrain) = world.get_mut::<VoxelTerrainComponent>(entity) {
            let count = terrain.edits.made_on(fingerprint);
            if count > 0 {
                tracing::info!(
                    "Voxel terrain {}: its ground changed; {count} edits made on the old ground were dropped",
                    entity.bits()
                );
                terrain.source_revision = terrain.source_revision.wrapping_add(1);
            }
            dropped += count;
        }
    }
    dropped
}

/// The form of a terrain's world: its shape, radius and plane size in
/// metres, with its owner's scale.
fn form(source: &WorldSource) -> (VoxelWorldShape, f64, f64) {
    let shape = match source.recipe.shape {
        Shape::Sphere => VoxelWorldShape::Sphere,
        Shape::Plane => VoxelWorldShape::Plane,
        Shape::InfinitePlane => VoxelWorldShape::InfinitePlane,
    };
    (shape, source.recipe.radius_m, source.recipe.plane_size_m)
}

/// The method a ground's dependents answer: what sits on a voxel terrain's
/// object and follows its ground (a sky, a haze, whatever kind brings one)
/// has `follow_ground(sphere, planet_radius_m, air, density, tint_r,
/// tint_g, tint_b, haze) -> bool`, called by name: voxel names no kind.
pub const FOLLOW_GROUND: &str = "follow_ground";

/// A voxel world's sky and haze are its air: what sits on a voxel
/// terrain's object and answers [`FOLLOW_GROUND`] follows that terrain's
/// ground, so resizing the planet or turning it into a plane takes it
/// along, and a world whose `air` is a preset (not `Custom`) has it derive
/// the rest from that air (the sky's and the fog's own rules: an airless
/// moon has neither; the sky's clouds stay as authored). Returns how many
/// dependents changed.
pub fn sync_air_to_terrains(world: &mut World) -> usize {
    let kinds = pulsar_world_registry::components_with_method(FOLLOW_GROUND);
    if kinds.is_empty() {
        return 0;
    }
    let world_ref: &World = world;
    let calls: Vec<(Entity, pulsar_scenedb::ComponentId, Ground)> = terrain_forms(world_ref)
        .flat_map(|(owner, terrain, (shape, planet_radius, _))| {
            let ground = Ground {
                sphere: shape == VoxelWorldShape::Sphere,
                planet_radius_m: planet_radius,
                air: format!("{:?}", terrain.air),
                density: f64::from(terrain.air_density),
                tint: terrain.air_tint.map(f64::from),
                haze: f64::from(terrain.air_haze),
            };
            let kinds = &kinds;
            attachments::instances(world_ref, owner)
                .into_iter()
                .filter(|instance| attachments::is_enabled(world_ref, *instance))
                .flat_map(move |instance| {
                    let ground = ground.clone();
                    kinds
                        .iter()
                        .filter(move |cid| world_ref.has_component(instance, **cid))
                        .map(move |cid| (instance, *cid, ground.clone()))
                })
        })
        .collect();
    let mut changed = 0;
    for (instance, cid, ground) in calls {
        let mut args: Vec<Box<dyn std::any::Any>> = vec![
            Box::new(ground.sphere),
            Box::new(ground.planet_radius_m),
            Box::new(ground.air),
            Box::new(ground.density),
            Box::new(ground.tint[0]),
            Box::new(ground.tint[1]),
            Box::new(ground.tint[2]),
            Box::new(ground.haze),
        ];
        match world.call_component_method(instance, cid, FOLLOW_GROUND, &mut args) {
            Ok(Some(answer)) => {
                if answer.downcast::<bool>().is_ok_and(|moved| *moved) {
                    changed += 1;
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!("{FOLLOW_GROUND} on {}: {error:?}", instance.bits()),
        }
    }
    changed
}

/// A terrain's ground and air, as [`FOLLOW_GROUND`] takes them.
#[derive(Clone)]
struct Ground {
    sphere: bool,
    planet_radius_m: f64,
    air: String,
    density: f64,
    tint: [f64; 3],
    haze: f64,
}

/// The enabled terrains with their owner and form.
fn terrain_forms(
    world: &World,
) -> impl Iterator<Item = (Entity, &VoxelTerrainComponent, (VoxelWorldShape, f64, f64))> {
    attachments::enabled_components::<VoxelTerrainComponent>(world).filter_map(
        move |(instance, owner, component)| {
            let source = entity_source(world, instance, component).ok()?;
            Some((owner, component, form(&source)))
        },
    )
}

/// Keeps voxel worlds' dependents in step with them when a terrain, its
/// layer settings, an atmosphere or a fog changed (change cursors, so
/// frames that change none cost nothing): edits made on ground a terrain no
/// longer has are dropped ([`sync_edit_journals`]) and skies and hazes
/// follow their terrain ([`sync_air_to_terrains`]).
pub struct VoxelWorldSync {
    cursors: Vec<pulsar_scenedb::ChangeCursor>,
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
                world.open_change_cursor::<VoxelTerrainLayersComponent>(),
            ]
            .into_iter()
            .chain(
                pulsar_world_registry::components_with_method(FOLLOW_GROUND)
                    .into_iter()
                    .map(|cid| world.open_change_cursor_id(cid)),
            )
            .collect(),
            revision: world.revision(),
            synced: false,
            scratch: Vec::new(),
        }
    }

    /// Drops edits made on ground a terrain no longer has and moves skies
    /// and hazes onto their terrain, when anything they depend on changed
    /// since the last poll. Returns how many edits were dropped.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Edits belong to the ground they were made on: changing the seed or
    /// the layer stack drops them; the form's unused size does not.
    #[test]
    fn edits_are_dropped_when_the_ground_changes() {
        use crate::{VoxelLayerKind, VoxelTerrainLayer, VoxelTerrainLayersComponent};
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
        let projected = |world: &World| {
            let c = world.get::<VoxelTerrainComponent>(entity).unwrap();
            crate::runtime::entity_target(world, entity, c)
                .unwrap()
                .edits
                .len()
        };
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
        use sky_component::{AtmosphereComponent, AtmospherePlacement};
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
        use crate::VoxelAir;
        use sky_component::AtmosphereComponent;
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
        use crate::VoxelAir;
        use fog_component::GlobalFogComponent;
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
        let earth = helio_pass_volumetric_fog::GlobalFogRow {
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
        let source =
            |world: &World| entity_source(world, terrain, world.get(terrain).unwrap()).unwrap();
        let bare = source(&world).fingerprint();
        world
            .get_mut::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .features = crate::feature_spawns(&FeatureRecipe::moon());
        let moon = source(&world);
        assert_eq!(moon.features, FeatureRecipe::moon());
        assert_ne!(moon.fingerprint(), bare);
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
        use crate::{VoxelLayerKind, VoxelTerrainLayer, VoxelTerrainLayersComponent};
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
}
