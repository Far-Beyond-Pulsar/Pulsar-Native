//! Level-wide rules on authored components: what a level may hold only one
//! of.
//!
//! - **One of each plugin singleton.** What a level holds at most one of
//!   registers itself ([`LevelSingletonRegistration`]): the sky plugin's
//!   atmosphere (#1057: the atmosphere pass draws the first enabled row, so
//!   a second would only fight the first), the foliage plugin's global wind
//!   (#1123). Adding a second, or a change that makes one
//!   (a property edit, enabling an instance, where the registration says
//!   those count), is refused with the registration's message.
//! - Directional lights are not level singletons: the renderer supports
//!   several enabled directional lights (the sky draws each as an emitter).
//!
//! Editors check a prospective change with these functions before applying
//! it and report the returned message. A level file that already breaks a
//! rule is repaired on load by [`enforce_on_load`].

use std::any::Any;

use pulsar_scene_model::attachments;
use pulsar_scene_model::SceneWorldExt;
use pulsar_scenedb::{Entity, World};

/// Something a level holds at most one of (the sky plugin's atmosphere, the
/// foliage plugin's global wind). Submitted by the plugin's component crate.
pub struct LevelSingletonRegistration {
    /// The component's class name.
    pub class_name: &'static str,
    /// Why adding a second one is refused.
    pub refusal: &'static str,
    /// Every instance that counts, in level order
    /// ([`attached_in_level_order`]): every attached one, or with
    /// `enabled_only` the enabled ones.
    pub instances: fn(&World) -> Vec<Entity>,
    /// Whether a component value is one.
    pub is: fn(&dyn Any) -> bool,
    /// Whether the value `instance` holds now is one.
    pub held: fn(&World, Entity) -> bool,
    /// Whether the class's default value is one (an atmosphere is; a light
    /// is a point light).
    pub default_is: bool,
    /// Only enabled instances count: enabling one is checked, a disabled
    /// one is allowed.
    pub enabled_only: bool,
    /// An edit of a value (a property, the whole value) can make it one, so
    /// editors check it ([`check_instance`]) after the write.
    pub edits_checked: bool,
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
        None => singleton.default_is && class_name == singleton.class_name,
    })
}

/// The registered singleton `instance` holds.
fn singleton_held(world: &World, instance: Entity) -> Option<&'static LevelSingletonRegistration> {
    singletons().find(|singleton| (singleton.instances)(world).contains(&instance))
}

/// Whether an edit of a `class_name` value (a property, the whole value)
/// can break a rule, so an editor checks it ([`check_instance`]) after the
/// write: a registration says its edits count (a light can be turned into
/// a second directional one).
pub fn edits_are_checked(class_name: &str) -> bool {
    singletons().any(|singleton| singleton.edits_checked && singleton.class_name == class_name)
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

/// Of `instances` (instance entities), the attached ones in level order (a
/// level file's order).
pub fn attached_in_level_order(
    world: &World,
    instances: impl Iterator<Item = Entity>,
) -> Vec<Entity> {
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
    let mut held: Vec<(&'static str, usize)> = singletons()
        .map(|singleton| (singleton.class_name, (singleton.instances)(world).len()))
        .collect();
    for component in components {
        if let Some(singleton) = singleton_of(component.class_name, component.value)
            .filter(|singleton| component.enabled || !singleton.enabled_only)
        {
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
/// level: a level's only plugin singleton (its sky) cannot be duplicated
/// (the copy keeps the source's enabled state).
pub fn check_copy(world: &World, source: Entity) -> Result<(), &'static str> {
    if let Some(singleton) = singleton_held(world, source) {
        return Err(singleton.refusal);
    }
    Ok(())
}

/// Whether enabling `instance` would break a rule: it holds a singleton
/// only enabled instances count of (a directional light) and another one
/// already counts.
pub fn check_enable(world: &World, instance: Entity) -> Result<(), &'static str> {
    for singleton in singletons().filter(|singleton| singleton.enabled_only) {
        if (singleton.held)(world, instance)
            && (singleton.instances)(world)
                .iter()
                .any(|other| *other != instance)
        {
            return Err(singleton.refusal);
        }
    }
    Ok(())
}

/// Whether `instance`, as it is now, breaks a rule against the rest of the
/// level: for checking an instance right after a path that decodes its
/// value itself (a record) attached it, or after an edit of its value (a
/// property, the whole value), which the caller reverts when refused.
pub fn check_instance(world: &World, instance: Entity) -> Result<(), &'static str> {
    if let Some(singleton) = singleton_held(world, instance) {
        if (singleton.instances)(world)
            .iter()
            .any(|other| *other != instance)
        {
            return Err(singleton.refusal);
        }
    }
    Ok(())
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
/// instance (file order) and disabling the rest (extra plugin singletons:
/// skies, global winds); each is logged as a
/// warning naming its object. Disabling keeps the authored value: the
/// instance stays attached and is saved as it was, only switched off.
pub fn enforce_on_load(world: &mut World) -> Vec<Demotion> {
    let mut demoted = Vec::new();
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
