//! Level-wide rules on authored components: what a level may hold only one
//! of.
//!
//! - **One sky** (#1057). The sky is the level's [`AtmosphereComponent`];
//!   the atmosphere pass draws the first enabled row, so a second one would
//!   only fight the first. Adding one to a level that has one (enabled or
//!   not) is refused ([`ONE_ATMOSPHERE`]).
//! - **One of each plugin singleton.** A plugin's component a level holds
//!   at most one of (the foliage plugin's global wind, #1123) registers
//!   itself ([`LevelSingletonRegistration`]); adding a second is refused
//!   with the registration's message, as for the sky.
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

/// A plugin's component a level holds at most one of (the foliage plugin's
/// global wind): the rules treat it as they treat the sky. Submitted by the
/// plugin's component crate.
pub struct LevelSingletonRegistration {
    /// The component's class name.
    pub class_name: &'static str,
    /// Why adding a second one is refused.
    pub refusal: &'static str,
    /// Every attached instance, enabled or not, in level order
    /// ([`attached_in_level_order`]).
    pub instances: fn(&World) -> Vec<Entity>,
    /// Whether a component value is one.
    pub is: fn(&dyn Any) -> bool,
}

inventory::collect!(LevelSingletonRegistration);

fn singletons() -> impl Iterator<Item = &'static LevelSingletonRegistration> {
    inventory::iter::<LevelSingletonRegistration>.into_iter()
}

/// The registered singleton `class_name` names, or `value` is.
fn singleton_of(
    class_name: &str,
    value: Option<&dyn Any>,
) -> Option<&'static LevelSingletonRegistration> {
    singletons().find(|singleton| match value {
        Some(value) => (singleton.is)(value),
        None => class_name == singleton.class_name,
    })
}

/// The registered singleton `instance` holds.
fn singleton_held(world: &World, instance: Entity) -> Option<&'static LevelSingletonRegistration> {
    singletons().find(|singleton| (singleton.instances)(world).contains(&instance))
}

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
pub fn sort_in_level_order(world: &World, instances: &mut [Entity]) {
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

/// Of `instances` (instance entities), the attached ones in level order (a
/// level file's order).
pub fn attached_in_level_order(world: &World, instances: impl Iterator<Item = Entity>) -> Vec<Entity> {
    let mut found: Vec<Entity> = instances
        .filter(|instance| attachments::owner_of(world, *instance).is_some())
        .collect();
    sort_in_level_order(world, &mut found);
    found
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
    let mut held: Vec<(&'static str, usize)> = singletons()
        .map(|singleton| (singleton.class_name, (singleton.instances)(world).len()))
        .collect();
    let mut suns = directional_lights(world).len();
    for component in components {
        if is_atmosphere(component.class_name, component.value) {
            atmospheres += 1;
            if atmospheres > 1 {
                return Err(ONE_ATMOSPHERE);
            }
        }
        if let Some(singleton) = singleton_of(component.class_name, component.value) {
            let count = held
                .iter_mut()
                .find(|(class, _)| *class == singleton.class_name)
                .map(|(_, count)| count)
                .expect("every registered singleton is counted");
            *count += 1;
            if *count > 1 {
                return Err(singleton.refusal);
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
/// level: a level's only sky or plugin singleton, or its casting directional light,
/// cannot be duplicated (the copy keeps the source's enabled state).
pub fn check_copy(world: &World, source: Entity) -> Result<(), &'static str> {
    if world.get::<AtmosphereComponent>(source).is_some() {
        return Err(ONE_ATMOSPHERE);
    }
    if let Some(singleton) = singleton_held(world, source) {
        return Err(singleton.refusal);
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
    if let Some(singleton) = singleton_held(world, instance) {
        if (singleton.instances)(world)
            .iter()
            .any(|other| *other != instance)
        {
            return Err(singleton.refusal);
        }
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
/// instance (file order) and disabling the rest (extra skies, extra plugin
/// singletons, extra casting directional lights); each is logged as a
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
    for singleton in singletons() {
        let extra: Vec<Entity> = (singleton.instances)(world)
            .into_iter()
            .filter(|instance| attachments::is_enabled(world, *instance))
            .skip(1)
            .collect();
        for instance in extra {
            demote(world, instance, singleton.refusal, &mut demoted);
        }
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
