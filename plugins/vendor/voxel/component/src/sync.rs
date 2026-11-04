//! A voxel terrain's dependents follow it: edits belong to the ground they
//! were made on, and a world's sky and haze are its air. [`VoxelWorldSync`]
//! keeps both in step from the scene's change journals; the terrains'
//! scene subsystem ([`crate::runtime::VoxelWorlds`]) polls it.

use pulsar_scene_model::attachments;
use pulsar_scenedb::{Entity, World};

use crate::world::{WorldSource, entity_source};
use crate::{VoxelAir, VoxelTerrainComponent, VoxelTerrainLayersComponent, VoxelWorldShape};
use fog_component::GlobalFogComponent;
use helio_pass_voxel_planet::grid::Shape;
use sky_component::{AtmosphereComponent, AtmospherePlacement};

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

/// A voxel world's sky and haze are its air: an atmosphere or global fog
/// on the entity of a voxel terrain sits on that terrain's ground, so
/// resizing the planet or turning it into a plane takes them along. A
/// sphere's atmosphere is centred on it at its radius; a plane's is flat
/// ground at the origin, at the Earth's radius unless it was already flat
/// (its curvature is the author's). A sphere's fog measures heights over
/// it, a plane's along world Y. A world whose `air` is a preset (not
/// `Custom`) derives the rest of both from it too
/// ([`sky_for_air`], [`haze_for_air`]):
/// an airless moon has neither; the sky's clouds stay as authored. Returns
/// how many atmospheres and fogs changed.
pub fn sync_air_to_terrains(world: &mut World) -> usize {
    sync_skies(world) + sync_hazes(world)
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

/// The skies of [`sync_air_to_terrains`].
fn sync_skies(world: &mut World) -> usize {
    let world_ref: &World = world;
    let changed: Vec<(Entity, AtmosphereComponent)> = terrain_forms(world_ref)
        .flat_map(|(owner, terrain, (shape, planet_radius, _))| {
            attachments::enabled_components_of::<AtmosphereComponent>(world_ref, owner)
                .into_iter()
                .filter_map(move |(sky, atmosphere)| {
                    let (placement, radius_km) = match shape {
                        VoxelWorldShape::Sphere => (
                            AtmospherePlacement::PlanetAtOwner,
                            (planet_radius / 1000.0) as f32,
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
                    let wanted = sky_for_air(
                        &placed,
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
        if let Some(mut atmosphere) = world.get_mut::<AtmosphereComponent>(*sky) {
            *atmosphere = wanted.clone();
        }
    }
    changed.len()
}

/// The hazes of [`sync_air_to_terrains`].
fn sync_hazes(world: &mut World) -> usize {
    let world_ref: &World = world;
    let changed: Vec<(Entity, GlobalFogComponent)> = terrain_forms(world_ref)
        .flat_map(|(owner, terrain, (shape, planet_radius, _))| {
            attachments::enabled_components_of::<GlobalFogComponent>(world_ref, owner)
                .into_iter()
                .filter_map(move |(fog, medium)| {
                    let mut placed = medium.clone();
                    placed.medium.planet_center = [0.0; 3];
                    placed.medium.planet_radius = match shape {
                        VoxelWorldShape::Sphere => planet_radius as f32,
                        _ => 0.0,
                    };
                    let wanted =
                        haze_for_air(&placed, terrain.air, terrain.air_density, terrain.air_haze)
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
fn same_sky(a: &AtmosphereComponent, b: &AtmosphereComponent) -> bool {
    a.enabled == b.enabled && a.placement == b.placement && a.to_row() == b.to_row()
}

/// The sky a voxel world's `air` settings describe, on `sky`'s planet
/// (placement, radius, ground and sun kept): `None` for `Custom` (the
/// authored sky stays); `VoxelAir::None` is a disabled sky. `density`
/// scales every coefficient, `tint` the molecular scattering per channel
/// (the sky's colour), `haze` the aerosols. The clouds stay as authored.
pub fn sky_for_air(
    sky: &AtmosphereComponent,
    air: VoxelAir,
    density: f32,
    tint: [f32; 3],
    haze: f32,
) -> Option<AtmosphereComponent> {
    use helio_pass_sky::AtmosphereRow;
    let preset = match air {
        VoxelAir::Custom => return None,
        VoxelAir::None => {
            return Some(AtmosphereComponent {
                enabled: false,
                ..sky.clone()
            });
        }
        VoxelAir::Earthlike => AtmosphereComponent::from_row(&AtmosphereRow::earth()),
        VoxelAir::Thin => AtmosphereComponent::from_row(&AtmosphereRow::mars()),
        VoxelAir::Dense => {
            // A deep haze over a tall atmosphere (Titan-like).
            let earth = AtmosphereComponent::from_row(&AtmosphereRow::earth());
            AtmosphereComponent {
                thickness_km: 200.0,
                rayleigh_scattering: earth.rayleigh_scattering.map(|v| v * 1.5),
                rayleigh_scale_height_km: 20.0,
                mie_scattering: earth.mie_scattering.map(|v| v * 12.0),
                mie_absorption: [0.012, 0.018, 0.035],
                mie_scale_height_km: 6.0,
                mie_anisotropy: 0.6,
                ozone_absorption: [0.0; 3],
                ..earth
            }
        }
    };
    let density = density.max(0.0);
    let haze = haze.max(0.0);
    let times = |v: [f32; 3], k: [f32; 3]| [v[0] * k[0], v[1] * k[1], v[2] * k[2]];
    Some(AtmosphereComponent {
        enabled: density > 0.0,
        placement: sky.placement,
        planet_radius_km: sky.planet_radius_km,
        sun_angular_radius_deg: sky.sun_angular_radius_deg,
        ground_albedo: sky.ground_albedo,
        rayleigh_scattering: times(
            preset.rayleigh_scattering,
            tint.map(|t| t.max(0.0) * density),
        ),
        mie_scattering: preset.mie_scattering.map(|v| v * density * haze),
        mie_absorption: preset.mie_absorption.map(|v| v * density * haze),
        ozone_absorption: preset.ozone_absorption.map(|v| v * density),
        // The clouds stay as authored.
        clouds_enabled: sky.clouds_enabled,
        cloud_bottom_km: sky.cloud_bottom_km,
        cloud_top_km: sky.cloud_top_km,
        cloud_coverage: sky.cloud_coverage,
        cloud_type: sky.cloud_type,
        cloud_extinction_per_km: sky.cloud_extinction_per_km,
        cloud_weather_km: sky.cloud_weather_km,
        cloud_shape_km: sky.cloud_shape_km,
        cloud_detail_km: sky.cloud_detail_km,
        cloud_erosion: sky.cloud_erosion,
        cloud_wind: sky.cloud_wind,
        cloud_albedo: sky.cloud_albedo,
        cloud_voxels: sky.cloud_voxels,
        cloud_voxel_m: sky.cloud_voxel_m,
        cloud_voxel_fill: sky.cloud_voxel_fill,
        cloud_pole: sky.cloud_pole,
        cloud_circulation: sky.cloud_circulation,
        cloud_circulation_cells: sky.cloud_circulation_cells,
        cloud_synoptic_km: sky.cloud_synoptic_km,
        cloud_storminess: sky.cloud_storminess,
        cloud_mesoscale_km: sky.cloud_mesoscale_km,
        cloud_cirrus: sky.cloud_cirrus,
        cloud_cirrus_km: sky.cloud_cirrus_km,
        cloud_evolution: sky.cloud_evolution,
        cloud_seed: sky.cloud_seed,
        ..preset
    })
}

/// The ground haze a voxel world's `air` settings describe, over `fog`'s
/// planet (`planet_center` and `planet_radius` kept): `None` for `Custom`
/// (the authored medium stays); `VoxelAir::None` is no haze. A preset's
/// haze thins with altitude over the ground, scattering forward, of the
/// air's aerosols' colour; `density` and `haze` scale its extinction as
/// they scale the sky's aerosols ([`sky_for_air`]).
pub fn haze_for_air(
    fog: &GlobalFogComponent,
    air: VoxelAir,
    density: f32,
    haze: f32,
) -> Option<GlobalFogComponent> {
    // Extinction (1/m), its falloff with altitude (1/m), anisotropy and
    // the share it scatters, per channel.
    let (extinction, falloff, anisotropy, albedo) = match air {
        VoxelAir::Custom => return None,
        VoxelAir::None => {
            return Some(GlobalFogComponent {
                enabled: false,
                ..fog.clone()
            });
        }
        // A clear day's boundary layer: kilometres of visibility,
        // thinning over a few hundred metres.
        VoxelAir::Earthlike => (6.0e-4, 4.0e-3, 0.6, [1.0; 3]),
        // Fine butterscotch dust lofted high in thin air.
        VoxelAir::Thin => (4.0e-4, 1.2e-3, 0.65, [0.95, 0.78, 0.6]),
        // A deep orange smog hugging the ground.
        VoxelAir::Dense => (4.0e-3, 8.0e-4, 0.5, [0.9, 0.65, 0.35]),
    };
    let extinction = extinction * density.max(0.0) * haze.max(0.0);
    Some(GlobalFogComponent {
        enabled: extinction > 0.0,
        medium: fog_component::MediumProps {
            mode: fog_component::FogMode::HeightBased,
            extinction,
            albedo,
            emission: [0.0; 3],
            anisotropy,
            height_falloff: falloff,
            height: 0.0,
            ..fog.medium.clone()
        },
    })
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
                world.open_change_cursor::<VoxelTerrainLayersComponent>(),
                world.open_change_cursor::<AtmosphereComponent>(),
                world.open_change_cursor::<GlobalFogComponent>(),
            ],
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

    /// A world's air derives its sky on the sky's planet; the clouds stay
    /// as authored.
    #[test]
    fn a_worlds_air_derives_its_sky() {
        use helio_pass_sky::AtmosphereRow;
        use sky_component::{AtmosphereComponent, AtmospherePlacement};
        let sky = AtmosphereComponent {
            placement: AtmospherePlacement::PlanetAtOwner,
            planet_radius_km: 1737.0,
            clouds_enabled: true,
            cloud_coverage: 0.7,
            ..Default::default()
        };
        assert!(
            sky_for_air(&sky, VoxelAir::Custom, 1.0, [1.0; 3], 1.0).is_none(),
            "custom keeps the authored sky"
        );
        assert!(
            !sky_for_air(&sky, VoxelAir::None, 1.0, [1.0; 3], 1.0)
                .unwrap()
                .enabled,
            "an airless world has no sky"
        );
        let earth = sky_for_air(&sky, VoxelAir::Earthlike, 1.0, [1.0; 3], 1.0).unwrap();
        assert!(earth.enabled);
        assert_eq!(
            (earth.placement, earth.planet_radius_km),
            (AtmospherePlacement::PlanetAtOwner, 1737.0),
            "the planet stays"
        );
        assert_eq!(
            earth.rayleigh_scattering,
            AtmosphereRow::earth().rayleigh_scattering
        );
        let thin = sky_for_air(&sky, VoxelAir::Earthlike, 0.5, [1.0, 0.5, 1.0], 2.0).unwrap();
        assert!((thin.rayleigh_scattering[1] - earth.rayleigh_scattering[1] * 0.25).abs() < 1e-9);
        assert!(
            (thin.mie_scattering[0] - earth.mie_scattering[0]).abs() < 1e-9,
            "half the air, twice the haze"
        );
        assert!(
            !sky_for_air(&sky, VoxelAir::Earthlike, 0.0, [1.0; 3], 1.0)
                .unwrap()
                .enabled,
            "no air at all"
        );
        let air = sky_for_air(&sky, VoxelAir::Thin, 1.0, [1.0; 3], 1.0).unwrap();
        assert!(
            air.clouds_enabled && air.cloud_coverage == 0.7,
            "a world's air keeps its clouds"
        );
    }

    /// Earth's air makes the haze Helio's voxel worlds fly through, over
    /// the planet the fog was placed on; the air's amounts scale it.
    #[test]
    fn a_worlds_air_derives_its_ground_haze() {
        use fog_component::{GlobalFogComponent, MediumProps};
        use helio_pass_volumetric_fog::GlobalFogRow as GlobalMedium;
        let placed = GlobalFogComponent {
            medium: MediumProps {
                planet_radius: 6_371_000.0,
                ..MediumProps::default()
            },
            ..GlobalFogComponent::default()
        };
        assert!(
            haze_for_air(&placed, VoxelAir::Custom, 1.0, 1.0).is_none(),
            "custom keeps the authored medium"
        );
        assert!(
            !haze_for_air(&placed, VoxelAir::None, 1.0, 1.0)
                .unwrap()
                .enabled,
            "no air, no haze"
        );
        let earth = haze_for_air(&placed, VoxelAir::Earthlike, 1.0, 1.0).unwrap();
        let flight = GlobalMedium {
            mode: 1,
            extinction: 0.0006,
            height_falloff: 0.004,
            anisotropy: 0.6,
            ..GlobalMedium::default()
        }
        .around_planet([0.0; 3], 6_371_000.0);
        assert!(earth.enabled);
        assert_eq!(earth.medium.to_medium(), flight);
        let hazy = haze_for_air(&placed, VoxelAir::Earthlike, 0.5, 4.0).unwrap();
        assert!(
            (hazy.medium.extinction - 0.0012).abs() < 1e-9,
            "half the air, four times the haze"
        );
        assert!(
            !haze_for_air(&placed, VoxelAir::Earthlike, 1.0, 0.0)
                .unwrap()
                .enabled,
            "no haze at all"
        );
    }
}
