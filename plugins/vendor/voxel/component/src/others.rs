//! The other kinds a voxel world composes (a preset's sky, sun, wind, haze,
//! ground cover and look; the sun its terrain is lit by) are reached by
//! class name and property through the world registry, the path scripts
//! take: voxel links none of their crates, so it requires none of their
//! plugins, and a project without one simply has no such instance. What a
//! kind does for a ground (a sky or a haze following it) is that kind's own
//! method (`sync::FOLLOW_GROUND`).

use pulsar_scene_model::attachments;
use pulsar_scenedb::{Entity, World};

/// The sky (`pulsar.sky`): one per level.
pub const ATMOSPHERE: &str = "AtmosphereComponent";
/// The global wind (`pulsar.foliage`): one per level.
pub const WIND: &str = "WindComponent";
/// A medium filling the world (`pulsar.fog`).
pub const GLOBAL_FOG: &str = "GlobalFogComponent";
/// A foliage layer (`pulsar.foliage`): ground cover is its `Block` kind.
pub const FOLIAGE: &str = "FoliageComponent";
/// A light (`pulsar.lights`): the sun is its enabled `Directional` one.
pub const LIGHT: &str = "LightComponent";
/// A post-process volume (`pulsar.post`): the look is its enabled unbound one.
pub const POST_VOLUME: &str = "PostProcessVolumeComponent";

/// The attached instances of `class`, in level order.
pub fn instances_of(world: &World, class: &str) -> Vec<Entity> {
    engine_backend::scene::level_rules::attached_in_level_order(
        world,
        world
            .query::<&attachments::ComponentMeta>()
            .filter(|(_, meta)| meta.class_name == class)
            .map(|(instance, _)| instance),
    )
}

/// `instance`'s property `name` (of `class`) as JSON, if it has one.
pub fn property(
    world: &World,
    instance: Entity,
    class: &str,
    name: &str,
) -> Option<serde_json::Value> {
    pulsar_world_registry::get_component_property(world, instance, class, 0, name).ok()
}

/// Whether `instance`'s property `name` is `value`.
pub fn property_is(
    world: &World,
    instance: Entity,
    class: &str,
    name: &str,
    value: serde_json::Value,
) -> bool {
    property(world, instance, class, name).is_some_and(|found| found == value)
}

/// `instance`'s enum property `name` (of `class`) as its variant's name: the
/// registry marshals an enum as its variant index, which the property's
/// reflected type names.
pub fn variant(world: &World, instance: Entity, class: &str, name: &str) -> Option<&'static str> {
    let index = property(world, instance, class, name)?.as_u64()?;
    let meta = pulsar_world_registry::property_descriptor(class, name).ok()?;
    match &meta.type_info.structure {
        pulsar_reflection::TypeStructure::Enum { variants } => {
            variants.get(index as usize).copied()
        }
        _ => None,
    }
}

/// Whether `instance` is a light that is the sun: enabled and directional.
pub fn is_sun(world: &World, instance: Entity) -> bool {
    property_is(world, instance, LIGHT, "enabled", serde_json::json!(true))
        && variant(world, instance, LIGHT, "light_type") == Some("Directional")
}

/// The level's suns ([`is_sun`], attached and enabled), in level order.
pub fn suns(world: &World) -> Vec<Entity> {
    instances_of(world, LIGHT)
        .into_iter()
        .filter(|instance| attachments::is_enabled(world, *instance) && is_sun(world, *instance))
        .collect()
}
