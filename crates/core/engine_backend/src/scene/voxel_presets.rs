//! Ready-made voxel worlds, added to a level in one step: an Earth-like
//! planet, the Moon, a desert planet and a flat block world.
//!
//! A preset is an example project's level (`assets/examples/<name>`): Helio's
//! voxel_flight worlds as Pulsar levels, which the `voxel_example_levels`
//! test checks against voxel_flight, so a new world and its example are one
//! description. [`add_voxel_world`] puts a preset's world into a level: the
//! terrain (layers, features, look), sky and clouds, sun, haze, ground cover
//! and wind, the post-process look, and the camera over the ground.
//!
//! A level holds one sky, one wind and one directional light
//! ([`level_rules`](super::level_rules)). The world's sky, haze, wind and
//! ground cover replace the level's; its sun and post-process look update the
//! level's directional light and unbound post-process volume in place; its
//! terrain replaces a voxel terrain of the level on that terrain's own
//! object. Objects left with nothing on them are removed.

use helio_component::{
    FoliageComponent, FoliageForm, GlobalFogComponent, LightComponent, PostProcessVolumeComponent,
    VoxelTerrainComponent, VoxelTerrainLayersComponent,
};
use pulsar_scene_model::{attachments, NewInstance};
use pulsar_scenedb::{Entity, World};

use super::{
    level_rules, EditorCamera, Name, ObjectType, RuntimeLevel, SceneWorldExt, SpawnObject,
    Transform, Visibility,
};

/// A ready-made voxel world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VoxelWorldPreset {
    /// Earth: woods, meadows, clouds and haze.
    Earth,
    /// The Moon: craters and boulders under a black sky.
    Moon,
    /// A desert planet: dunes, rocks and shrubs in dusty air.
    Desert,
    /// Flat grassland without edges under Earth's sky, to build on.
    Blocks,
}

impl VoxelWorldPreset {
    pub const ALL: [Self; 4] = [Self::Earth, Self::Moon, Self::Desert, Self::Blocks];

    /// The example project with this world (`assets/examples/<name>`).
    pub fn example(self) -> &'static str {
        match self {
            Self::Earth => "voxel_planet",
            Self::Moon => "voxel_moon",
            Self::Desert => "voxel_desert",
            Self::Blocks => "voxel_blocks",
        }
    }

    /// The example's level file.
    fn level(self) -> &'static str {
        match self {
            Self::Earth => {
                include_str!("../../../../../assets/examples/voxel_planet/scene/default.level")
            }
            Self::Moon => {
                include_str!("../../../../../assets/examples/voxel_moon/scene/default.level")
            }
            Self::Desert => {
                include_str!("../../../../../assets/examples/voxel_desert/scene/default.level")
            }
            Self::Blocks => {
                include_str!("../../../../../assets/examples/voxel_blocks/scene/default.level")
            }
        }
    }

    /// The world as a level of its own.
    pub fn template(self) -> Result<RuntimeLevel, String> {
        let file = serde_json::from_str(self.level())
            .map_err(|error| format!("{} level: {error}", self.example()))?;
        RuntimeLevel::from_scene_file_with_classes(file, &pulsar_class::ClassRegistry::default())
            .map_err(|error| format!("{} level: {error}", self.example()))
    }
}

/// What [`add_voxel_world`] did.
#[derive(Clone, Debug, PartialEq)]
pub struct VoxelWorldSetup {
    /// The terrain's object.
    pub terrain: String,
    /// Objects it added.
    pub added: Vec<String>,
    /// Existing objects it changed or removed.
    pub changed: Vec<String>,
    /// The world's camera over its ground.
    pub camera: Option<EditorCamera>,
}

/// What of a level a new world takes over.
struct Plan {
    /// A voxel terrain's object (the first by id): the new terrain goes on
    /// it.
    terrain_owner: Option<Entity>,
    /// The directional light, updated in place.
    sun: Option<Entity>,
    /// The first enabled unbound post-process volume, updated in place.
    look: Option<Entity>,
    /// Instances the world replaces: the sky, hazes, wind, ground cover and
    /// the terrain with its layers.
    replaced: Vec<Entity>,
}

impl Plan {
    fn of(world: &World) -> Self {
        let terrain_owner = world
            .query::<&VoxelTerrainComponent>()
            .filter_map(|(instance, _)| attachments::owner_of(world, instance))
            .min_by_key(|owner| world.stable_id_of(*owner).map(str::to_string));
        let mut replaced = level_rules::atmospheres(world);
        replaced.extend(level_rules::winds(world));
        replaced.extend(attached::<GlobalFogComponent>(world, |_| true));
        // Ground cover belongs to the voxel world it grows on.
        replaced.extend(attached::<FoliageComponent>(world, |foliage| {
            foliage.general.kind == FoliageForm::Block
        }));
        if let Some(owner) = terrain_owner {
            replaced.extend(
                attachments::instances(world, owner)
                    .into_iter()
                    .filter(|instance| {
                        world.get::<VoxelTerrainComponent>(*instance).is_some()
                            || world
                                .get::<VoxelTerrainLayersComponent>(*instance)
                                .is_some()
                    }),
            );
        }
        Self {
            terrain_owner,
            sun: level_rules::directional_lights(world).first().copied(),
            look: attached::<PostProcessVolumeComponent>(world, |volume| {
                volume.enabled && volume.unbound
            })
            .into_iter()
            .find(|instance| attachments::is_enabled(world, *instance)),
            replaced,
        }
    }

    /// The existing objects the world changes.
    fn objects(&self, world: &World) -> Vec<Entity> {
        let mut objects: Vec<Entity> = self
            .replaced
            .iter()
            .chain(self.sun.iter())
            .chain(self.look.iter())
            .filter_map(|instance| attachments::owner_of(world, *instance))
            .chain(self.terrain_owner)
            .collect();
        objects.sort_by_key(|object| object.bits());
        objects.dedup();
        objects
    }
}

/// The attached `T` instances `keep` selects.
fn attached<T: pulsar_scenedb::Component>(world: &World, keep: impl Fn(&T) -> bool) -> Vec<Entity> {
    world
        .query::<&T>()
        .filter(|(instance, value)| {
            keep(value) && attachments::owner_of(world, *instance).is_some()
        })
        .map(|(instance, _)| instance)
        .collect()
}

/// The ids of the existing objects [`add_voxel_world`] would change or
/// remove in `world` (an editor's undo scope).
pub fn objects_changed_by(world: &World) -> Vec<String> {
    let plan = Plan::of(world);
    ids(world, &plan.objects(world))
}

fn ids(world: &World, objects: &[Entity]) -> Vec<String> {
    objects
        .iter()
        .filter_map(|object| world.stable_id_of(*object).map(str::to_string))
        .collect()
}

/// Add `preset`'s world to the level in `world` (see the module doc).
pub fn add_voxel_world(
    world: &mut World,
    preset: VoxelWorldPreset,
) -> Result<VoxelWorldSetup, String> {
    let template = preset.template()?;
    let scene = template.scene();
    let scene = scene.read();
    let source = &scene.world;

    let plan = Plan::of(world);
    let touched = plan.objects(world);
    let changed = ids(world, &touched);
    for instance in &plan.replaced {
        attachments::detach(world, *instance);
    }

    let mut added = Vec::new();
    let mut terrain = None;
    for object in source.children_of(None) {
        let instances = attachments::instances(source, object);
        let has_terrain = instances
            .iter()
            .any(|instance| source.get::<VoxelTerrainComponent>(*instance).is_some());
        let light = instances.iter().find(|instance| {
            source
                .get::<LightComponent>(**instance)
                .is_some_and(level_rules::is_directional)
        });
        let look = instances.iter().find(|instance| {
            source
                .get::<PostProcessVolumeComponent>(**instance)
                .is_some_and(|volume| volume.unbound)
        });
        match (light, plan.sun, look, plan.look) {
            (Some(light), Some(sun), _, _) => {
                set_value(source, *light, world, sun)?;
                // The sun shines along its object's rotation.
                let rotation = source.get::<Transform>(object).map(|t| t.rotation);
                if let (Some(owner), Some(rotation)) = (attachments::owner_of(world, sun), rotation)
                {
                    if let Some(mut transform) = world.get_mut::<Transform>(owner) {
                        transform.rotation = rotation;
                    }
                }
                continue;
            }
            (_, _, Some(look), Some(existing)) => {
                set_value(source, *look, world, existing)?;
                continue;
            }
            _ => {}
        }
        let owner = match plan.terrain_owner.filter(|_| has_terrain) {
            Some(owner) => owner,
            None => {
                let owner = copy_object(source, object, world)?;
                added.extend(world.stable_id_of(owner).map(str::to_string));
                owner
            }
        };
        if has_terrain {
            terrain = Some(owner);
        }
        for instance in instances {
            copy_instance(source, instance, world, owner)?;
        }
    }

    // Objects the replaced instances left bare.
    for object in touched {
        let bare = world.is_alive(object)
            && attachments::instances(world, object).is_empty()
            && world.children_of(Some(object)).is_empty()
            && world
                .get::<ObjectType>(object)
                .is_none_or(|kind| *kind == ObjectType::Empty);
        if bare {
            world.despawn_tree(object);
        }
    }

    let terrain =
        terrain.ok_or_else(|| format!("{} level has no voxel terrain", preset.example()))?;
    // The world sits on its terrain's object: so does its camera.
    let origin = world
        .get::<Transform>(terrain)
        .map_or([0.0; 3], |t| t.position);
    let camera = template.editor_camera().map(|camera| EditorCamera {
        position: [0, 1, 2].map(|axis| camera.position[axis] + origin[axis]),
        ..camera
    });
    Ok(VoxelWorldSetup {
        terrain: world
            .stable_id_of(terrain)
            .map(str::to_string)
            .unwrap_or_default(),
        added,
        changed,
        camera,
    })
}

/// A copy of `source`'s top-level `object` (name, transform, type,
/// visibility; no components) in `world`.
fn copy_object(source: &World, object: Entity, world: &mut World) -> Result<Entity, String> {
    let spec = SpawnObject {
        stable_id: None,
        name: source
            .get::<Name>(object)
            .map(|name| name.0.clone())
            .unwrap_or_default(),
        parent: None,
        transform: source.get::<Transform>(object).copied().unwrap_or_default(),
        visibility: source
            .get::<Visibility>(object)
            .cloned()
            .unwrap_or_default(),
        object_type: source
            .get::<ObjectType>(object)
            .cloned()
            .unwrap_or(ObjectType::Empty),
    };
    world.spawn_object(spec).map_err(|error| error.to_string())
}

/// Attach a copy of `source`'s `instance` to `owner` in `world`.
fn copy_instance(
    source: &World,
    instance: Entity,
    world: &mut World,
    owner: Entity,
) -> Result<(), String> {
    use pulsar_world_registry::{ComponentPayload, InstanceValue};
    let snapshot = pulsar_world_registry::snapshot_instance(source, instance)
        .ok_or("a preset component cannot be copied")?;
    let class = snapshot.meta.class_name;
    let InstanceValue::Value(value) = snapshot.value else {
        return Err(format!("{class} is not a class of this build"));
    };
    let spec = NewInstance {
        enabled: snapshot.enabled,
        ..NewInstance::new(class)
    };
    pulsar_world_registry::attach_component(world, owner, spec, ComponentPayload::Value(value))
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Give `world`'s `instance` the value of `source`'s `from` (same class).
fn set_value(
    source: &World,
    from: Entity,
    world: &mut World,
    instance: Entity,
) -> Result<(), String> {
    let snapshot = pulsar_world_registry::snapshot_instance(source, from)
        .ok_or("a preset component cannot be copied")?;
    pulsar_world_registry::set_instance_value(world, instance, snapshot.value)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use helio_component::AtmosphereComponent;

    fn level_of(world: &World) -> Vec<(String, Vec<String>)> {
        let mut objects: Vec<_> = world
            .children_of(None)
            .into_iter()
            .map(|object| {
                let mut classes: Vec<String> = attachments::instances(world, object)
                    .into_iter()
                    .filter_map(|instance| {
                        attachments::meta(world, instance).map(|m| m.class_name.clone())
                    })
                    .collect();
                classes.sort();
                (
                    world
                        .get::<Name>(object)
                        .map(|n| n.0.clone())
                        .unwrap_or_default(),
                    classes,
                )
            })
            .collect();
        objects.sort();
        objects
    }

    /// Every preset in an empty level is its example level, camera included.
    #[test]
    fn a_preset_in_an_empty_level_is_its_example() {
        for preset in VoxelWorldPreset::ALL {
            let mut world = World::new();
            let setup = add_voxel_world(&mut world, preset).unwrap();
            let template = preset.template().unwrap();
            let example = template.scene();
            assert_eq!(
                level_of(&world),
                level_of(&example.read().world),
                "{preset:?}"
            );
            assert_eq!(setup.camera, template.editor_camera(), "{preset:?}");
            assert!(setup.changed.is_empty());
            assert_eq!(setup.added.len(), world.children_of(None).len());
            let terrain = world.entity_for(&setup.terrain).unwrap();
            assert_eq!(
                attachments::enabled_components_of::<VoxelTerrainComponent>(&world, terrain).len(),
                1
            );
            assert!(level_rules::atmospheres(&world).len() <= 1);
            assert!(level_rules::winds(&world).len() <= 1);
            assert_eq!(level_rules::directional_lights(&world).len(), 1);
        }
    }

    /// A level's sky, sun, wind, haze, look and terrain are taken over, not
    /// doubled: the level rules hold and a second setup changes the first
    /// world rather than adding another.
    #[test]
    fn a_second_world_takes_over_the_first() {
        let mut world = World::new();
        let first = add_voxel_world(&mut world, VoxelWorldPreset::Earth).unwrap();
        let objects = world.children_of(None).len();
        let moon = add_voxel_world(&mut world, VoxelWorldPreset::Moon).unwrap();
        assert_eq!(moon.terrain, first.terrain, "the terrain's object is kept");
        assert!(moon.added.is_empty(), "{moon:?}");
        // The meadow went with Earth.
        assert_eq!(world.children_of(None).len(), objects - 1);
        assert_eq!(world.query::<&FoliageComponent>().count(), 0);
        assert_eq!(level_rules::atmospheres(&world).len(), 1);
        assert_eq!(level_rules::winds(&world).len(), 0);
        assert_eq!(level_rules::directional_lights(&world).len(), 1);
        assert_eq!(world.query::<&VoxelTerrainComponent>().count(), 1);
        assert_eq!(world.query::<&VoxelTerrainLayersComponent>().count(), 1);
        let terrain = world.query::<&VoxelTerrainComponent>().next().unwrap().1;
        assert_eq!(terrain.planet_radius, 1_737_400.0);
        let mut changed = moon.changed.clone();
        changed.sort();
        let mut before = first.added.clone();
        before.sort();
        assert_eq!(changed, before, "every Earth object is taken over");
        // An object the world does not know stays as it is.
        let mut sky_only = World::new();
        let lamp = sky_only.spawn_object(SpawnObject::new("Lamp")).unwrap();
        let mut point = LightComponent::default();
        point.general.light_type = helio_component::LightType::Point;
        pulsar_world_registry::attach_value(&mut sky_only, lamp, point).unwrap();
        let sky = sky_only.spawn_object(SpawnObject::new("Sky")).unwrap();
        pulsar_world_registry::attach_value(&mut sky_only, sky, AtmosphereComponent::default())
            .unwrap();
        assert_eq!(objects_changed_by(&sky_only), ids(&sky_only, &[sky]));
        add_voxel_world(&mut sky_only, VoxelWorldPreset::Earth).unwrap();
        assert!(!sky_only.is_alive(sky), "the bare sky object is removed");
        assert!(sky_only.is_alive(lamp));
        assert_eq!(level_rules::atmospheres(&sky_only).len(), 1);
    }

    /// A world replacing a terrain on a moved object sits, with its camera,
    /// where that object is.
    #[test]
    fn a_reused_terrain_object_keeps_its_place() {
        let mut world = World::new();
        let owner = world
            .spawn_object(SpawnObject::new("Ground").with_transform(Transform {
                position: [100.0, 0.0, 0.0],
                ..Transform::default()
            }))
            .unwrap();
        pulsar_world_registry::attach_value(&mut world, owner, VoxelTerrainComponent::plane(512.0))
            .unwrap();
        let setup = add_voxel_world(&mut world, VoxelWorldPreset::Blocks).unwrap();
        assert_eq!(world.entity_for(&setup.terrain), Some(owner));
        assert_eq!(setup.changed, ids(&world, &[owner]));
        let example = VoxelWorldPreset::Blocks
            .template()
            .unwrap()
            .editor_camera()
            .unwrap();
        assert_eq!(
            setup.camera.unwrap().position[0],
            example.position[0] + 100.0
        );
    }
}
