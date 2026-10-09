//! Level-wide rules on authored components: what a level may hold only one
//! of.
//!
//! - **One sky** (#1057). The sky is the level's [`AtmosphereComponent`];
//!   the atmosphere pass draws the first enabled row, so a second one would
//!   only fight the first. Adding one to a level that has one (enabled or
//!   not) is refused ([`ONE_ATMOSPHERE`]).
//!
//! Editors check a prospective change with these functions before applying
//! it and report the returned message. A level file that already breaks a
//! rule is repaired on load by [`enforce_on_load`].

use std::any::Any;

use pulsar_scene_model::attachments;
use pulsar_scenedb::{Entity, World};

use helio_component::AtmosphereComponent;

/// The sky's class name.
pub const ATMOSPHERE_CLASS: &str = "AtmosphereComponent";

/// Why adding an atmosphere was refused.
pub const ONE_ATMOSPHERE: &str =
    "This level already has a sky (an AtmosphereComponent); a level has one. Edit it in World Settings.";

/// Every attached [`AtmosphereComponent`] instance, enabled or not, in
/// attach order (the world's entity order: a level's file order on load).
pub fn atmospheres(world: &World) -> Vec<Entity> {
    let mut found: Vec<Entity> = world
        .query::<&AtmosphereComponent>()
        .map(|(instance, _)| instance)
        .filter(|instance| attachments::owner_of(world, *instance).is_some())
        .collect();
    found.sort_by_key(|entity| entity.index());
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
    for component in components {
        if is_atmosphere(component.class_name, component.value) {
            atmospheres += 1;
            if atmospheres > 1 {
                return Err(ONE_ATMOSPHERE);
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
/// level: a level's only sky cannot be duplicated.
pub fn check_copy(world: &World, source: Entity) -> Result<(), &'static str> {
    if world.get::<AtmosphereComponent>(source).is_some() {
        return Err(ONE_ATMOSPHERE);
    }
    Ok(())
}

/// Whether `instance`, as it is now, breaks a rule against the rest of the
/// level: for checking an instance right after a path that decodes its
/// value itself (a record) attached it.
pub fn check_instance(world: &World, instance: Entity) -> Result<(), &'static str> {
    if world.get::<AtmosphereComponent>(instance).is_some()
        && atmospheres(world).iter().any(|other| *other != instance)
    {
        return Err(ONE_ATMOSPHERE);
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
/// instance (file order) and disabling the rest; each is logged as a
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
