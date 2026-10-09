//! Level-wide rules on authored components: what a level may hold only one
//! of.
//!
//! - **One sky** (#1057). The sky is the level's [`AtmosphereComponent`];
//!   the atmosphere pass draws the first enabled row, so a second one would
//!   only fight the first. Adding one to a level that has one (enabled or
//!   not) is refused ([`ONE_ATMOSPHERE`]).
//! - **One directional light.** The atmosphere's sun, the voxel sun and the
//!   renderer's sun direction all take the first directional light, so a
//!   level casts at most one: an enabled `LightComponent` instance whose
//!   light is enabled and directional ([`directional_lights`]). Adding one,
//!   turning a light directional, or enabling one while another casts is
//!   refused ([`ONE_DIRECTIONAL_LIGHT`]). A disabled directional light is
//!   allowed; it casts nothing until it is the only one.
//!
//! Editors check a prospective change with these functions before applying
//! it and report the returned message. A level file that already breaks a
//! rule is repaired on load by [`enforce_on_load`].

use std::any::Any;

use pulsar_scene_model::attachments;
use pulsar_scene_model::SceneWorldExt;
use pulsar_scenedb::{Entity, World};

use helio_component::{AtmosphereComponent, LightComponent, LightType};

/// The sky's class name.
pub const ATMOSPHERE_CLASS: &str = "AtmosphereComponent";

/// Why adding an atmosphere was refused.
pub const ONE_ATMOSPHERE: &str =
    "This level already has a sky (an AtmosphereComponent); a level has one. Edit it in World Settings.";

/// Why a second directional light was refused.
pub const ONE_DIRECTIONAL_LIGHT: &str =
    "This level already has a directional light; a level has one. Disable it or change its type first.";

/// Whether an edit of a `class_name` value (a property, the whole value)
/// can break a rule, so an editor checks it ([`check_instance`]) after the
/// write: a light can be turned into a second directional one.
pub fn edits_are_checked(class_name: &str) -> bool {
    class_name == "LightComponent"
}

/// Whether `light`, on an enabled instance, is a directional light that
/// casts.
pub fn is_directional(light: &LightComponent) -> bool {
    light.general.enabled && light.general.light_type == LightType::Directional
}

/// Every casting directional light: the enabled `LightComponent` instances
/// whose light is enabled and directional. Never more than
/// one in a level the editor built or the loader repaired. In level order.
pub fn directional_lights(world: &World) -> Vec<Entity> {
    let mut found: Vec<Entity> = world
        .query::<&LightComponent>()
        .filter(|(instance, light)| {
            is_directional(light) && attachments::is_enabled(world, *instance)
        })
        .map(|(instance, _)| instance)
        .collect();
    sort_in_level_order(world, &mut found);
    found
}

/// Sort component instances in level order: their objects depth first in
/// hierarchy order (the order a level file lists them), then each object's
/// component list. "The first" of several is the first in this order.
fn sort_in_level_order(world: &World, instances: &mut [Entity]) {
    if instances.len() < 2 {
        return;
    }
    let mut rank = std::collections::HashMap::new();
    let mut stack: Vec<Entity> = world.children_of(None).into_iter().rev().collect();
    while let Some(object) = stack.pop() {
        for instance in attachments::instances(world, object) {
            let next = rank.len();
            rank.entry(instance).or_insert(next);
        }
        stack.extend(world.children_of(Some(object)).into_iter().rev());
    }
    instances.sort_by_key(|instance| {
        (
            rank.get(instance).copied().unwrap_or(usize::MAX),
            instance.index(),
        )
    });
}

/// Every attached [`AtmosphereComponent`] instance, enabled or not, in
/// level order (a level file's order).
pub fn atmospheres(world: &World) -> Vec<Entity> {
    let mut found: Vec<Entity> = world
        .query::<&AtmosphereComponent>()
        .map(|(instance, _)| instance)
        .filter(|instance| attachments::owner_of(world, *instance).is_some())
        .collect();
    sort_in_level_order(world, &mut found);
    found
}

/// The level's sky: its first [`AtmosphereComponent`] instance and that
/// instance's owner object.
pub fn level_atmosphere(world: &World) -> Option<(Entity, Entity)> {
    let instance = *atmospheres(world).first()?;
    Some((instance, attachments::owner_of(world, instance)?))
}

/// One component a caller is about to add: its class, its value (`None`
/// for the class default) and whether it will be enabled.
#[derive(Clone, Copy)]
pub struct NewComponent<'a> {
    pub class_name: &'a str,
    pub value: Option<&'a dyn Any>,
    pub enabled: bool,
}

/// Whether `components` may all be added to the level, in order. `Err`
/// carries the message to show; nothing should be added then.
pub fn check_new_components(
    world: &World,
    components: &[NewComponent<'_>],
) -> Result<(), &'static str> {
    let mut atmospheres = atmospheres(world).len();
    let mut suns = directional_lights(world).len();
    for component in components {
        if is_atmosphere(component.class_name, component.value) {
            atmospheres += 1;
            if atmospheres > 1 {
                return Err(ONE_ATMOSPHERE);
            }
        }
        // A light added as the class default is a point light.
        let sun = component
            .value
            .and_then(|value| value.downcast_ref::<LightComponent>())
            .is_some_and(is_directional);
        if sun && component.enabled {
            suns += 1;
            if suns > 1 {
                return Err(ONE_DIRECTIONAL_LIGHT);
            }
        }
    }
    Ok(())
}

/// [`check_new_components`] for one component.
pub fn check_new_component(
    world: &World,
    class_name: &str,
    value: Option<&dyn Any>,
    enabled: bool,
) -> Result<(), &'static str> {
    check_new_components(
        world,
        &[NewComponent {
            class_name,
            value,
            enabled,
        }],
    )
}

/// Whether a copy of `source` (an attached instance) may be added to the
/// level: a level's only sky, or its casting directional light, cannot be
/// duplicated (the copy keeps the source's enabled state).
pub fn check_copy(world: &World, source: Entity) -> Result<(), &'static str> {
    if world.get::<AtmosphereComponent>(source).is_some() {
        return Err(ONE_ATMOSPHERE);
    }
    if directional_lights(world).contains(&source) {
        return Err(ONE_DIRECTIONAL_LIGHT);
    }
    Ok(())
}

/// Whether enabling `instance` would break a rule: it holds a directional
/// light and another one already casts.
pub fn check_enable(world: &World, instance: Entity) -> Result<(), &'static str> {
    let sun = world
        .get::<LightComponent>(instance)
        .is_some_and(is_directional);
    if sun
        && directional_lights(world)
            .iter()
            .any(|other| *other != instance)
    {
        return Err(ONE_DIRECTIONAL_LIGHT);
    }
    Ok(())
}

/// Whether `instance`, as it is now, breaks a rule against the rest of the
/// level: for checking an instance right after a path that decodes its
/// value itself (a record) attached it, or after an edit of its value (a
/// property, the whole value), which the caller reverts when refused.
pub fn check_instance(world: &World, instance: Entity) -> Result<(), &'static str> {
    if world.get::<AtmosphereComponent>(instance).is_some()
        && atmospheres(world).iter().any(|other| *other != instance)
    {
        return Err(ONE_ATMOSPHERE);
    }
    let suns = directional_lights(world);
    if suns.contains(&instance) && suns.len() > 1 {
        return Err(ONE_DIRECTIONAL_LIGHT);
    }
    Ok(())
}

fn is_atmosphere(class_name: &str, value: Option<&dyn Any>) -> bool {
    match value {
        Some(value) => value.is::<AtmosphereComponent>(),
        None => class_name == ATMOSPHERE_CLASS,
    }
}

/// What [`enforce_on_load`] changed: an instance it disabled, why, and the
/// object it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Demotion {
    pub instance: Entity,
    pub owner: Entity,
    pub rule: &'static str,
}

/// Repair a freshly loaded level that breaks a rule, keeping the first
/// instance (file order) and disabling the rest (extra skies, extra casting
/// directional lights); each is logged as a
/// warning naming its object. Disabling keeps the authored value: the
/// instance stays attached and is saved as it was, only switched off.
pub fn enforce_on_load(world: &mut World) -> Vec<Demotion> {
    let mut demoted = Vec::new();
    let extra_atmospheres: Vec<Entity> = atmospheres(world)
        .into_iter()
        .filter(|instance| attachments::is_enabled(world, *instance))
        .skip(1)
        .collect();
    for instance in extra_atmospheres {
        demote(world, instance, ONE_ATMOSPHERE, &mut demoted);
    }
    // Every light keeps its type and values; only the instances past the
    // first casting directional light are switched off, so exactly one
    // directional light renders.
    for instance in directional_lights(world).into_iter().skip(1) {
        demote(world, instance, ONE_DIRECTIONAL_LIGHT, &mut demoted);
    }
    demoted
}

fn demote(world: &mut World, instance: Entity, rule: &'static str, demoted: &mut Vec<Demotion>) {
    let Some(owner) = attachments::owner_of(world, instance) else {
        return;
    };
    attachments::set_enabled(world, instance, false);
    let object = world
        .get::<pulsar_scene_model::StableId>(owner)
        .map(|id| id.0.clone())
        .unwrap_or_default();
    let class = attachments::meta(world, instance)
        .map(|meta| meta.class_name.clone())
        .unwrap_or_default();
    tracing::warn!(object = %object, class = %class, "Disabled on load: {rule}");
    demoted.push(Demotion {
        instance,
        owner,
        rule,
    });
}
