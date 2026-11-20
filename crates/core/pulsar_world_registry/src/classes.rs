//! Looking up registered classes by name or component id, hydrating and removing their values, and class requirements.

use crate::{values, EngineClassMut, WorldComponentRegistration};
use pulsar_reflection::EngineClass;
use pulsar_scenedb::{ComponentId, Entity, World};
use serde_json::Value;

pub(crate) fn find(class_name: &str) -> Option<&'static WorldComponentRegistration> {
    crate::runtime::world_components()
        .iter()
        .copied()
        .into_iter()
        .find(|r| r.class_name == class_name)
}

/// Same lookup as [`find`], keyed by `ComponentId` instead of class name --
/// what a drained `ChangeTracker::component_removals` entry actually
/// carries (see `WorldComponentRegistration::component_type`'s doc). A
/// linear scan over the registered classes, same as `find` -- the registered-
/// class count is small (dozens, not thousands) and this only runs once
/// per removal event, not per frame per entity, so it isn't worth a
/// memoized `HashMap` until that stops being true.
pub(crate) fn find_by_component_id(
    component_type: ComponentId,
) -> Option<&'static WorldComponentRegistration> {
    crate::runtime::world_components()
        .iter()
        .copied()
        .into_iter()
        .find(|r| (r.component_type)() == component_type)
}

/// Resolve a registered class's `pulsar_scenedb::ComponentId` -- the erased
/// identity change cursors and [`crate::ComponentWatch`] are keyed by. The
/// watch-path counterpart to [`find_by_component_id`]: an editor caller that
/// only knows a class NAME (the properties panel's reflection metadata) can
/// watch an instance without ever naming the Rust type. Returns `None` for
/// classes not registered here -- those have no live `World` representation,
/// so there is nothing to watch.
pub fn component_id_for_class(class_name: &str) -> Option<ComponentId> {
    find(class_name).map(|r| (r.component_type)())
}

/// The registered component classes that have a method `name` (reflected
/// or world): what a host calls by name on any component offering it, the
/// way scripts do, without naming a class.
pub fn components_with_method(name: &str) -> Vec<ComponentId> {
    crate::runtime::world_components()
        .iter()
        .map(|r| (r.component_type)())
        .filter(|cid| pulsar_scenedb::find_component_method(*cid, name).is_some())
        .collect()
}

/// Hydrate `class_name`'s typed component from `data` onto `entity`: decode
/// the JSON once into an owned value and insert it through
/// [`crate::values::insert_world_component_value`] (SceneDB's erased insert, with
/// every normal write hook). Returns `Ok(false)` if `class_name` isn't
/// registered here (not a world component class). Returns `Err` only if
/// `class_name` *is* registered but `data` failed to decode -- nothing is
/// inserted then.
pub fn hydrate_world_component_for_class(
    class_name: &str,
    world: &mut World,
    entity: Entity,
    data: &Value,
) -> Result<bool, String> {
    let Some(registration) = find(class_name) else {
        return Ok(false);
    };
    let value = (registration.decode)(data)?;
    values::insert_world_component_value(class_name, world, entity, value)
        .map(|_| true)
        .map_err(|error| error.to_string())
}

/// Remove `class_name`'s typed component from `entity`, if that class is
/// registered here. Returns `false` if `class_name` isn't registered.
pub fn remove_world_component_for_class(
    class_name: &str,
    world: &mut World,
    entity: Entity,
) -> bool {
    match find(class_name) {
        Some(registration) => {
            (registration.remove)(world, entity);
            true
        }
        None => false,
    }
}

/// Borrow `class_name`'s typed value already in `World` as `&dyn EngineClass`
/// for reading -- the properties panel's read path.
/// `None` if `class_name` isn't registered here, or `entity` doesn't have
/// this component in `World` yet; callers should fall back to whatever
/// non-live source they have (JSON, a `Default` instance) in that case.
pub fn get_world_component_as_engine_class<'w>(
    class_name: &str,
    world: &'w World,
    entity: Entity,
) -> Option<&'w dyn EngineClass> {
    (find(class_name)?.get_as_engine_class)(world, entity)
}

/// Borrow `class_name`'s typed value already in `World` as
/// `&mut dyn EngineClass` for direct in-place editing -- the properties
/// panel's write path. Apply a property's setter closure straight to this
/// reference: it mutates the one real `World`-resident component, so there
/// is nothing to write back afterward. `None` under the same conditions as
/// [`get_world_component_as_engine_class`].
pub fn get_world_component_as_engine_class_mut<'w>(
    class_name: &str,
    world: &'w mut World,
    entity: Entity,
) -> Option<EngineClassMut<'w>> {
    (find(class_name)?.get_as_engine_class_mut)(world, entity)
}

/// The registered class `class_name`'s registration: its required classes
/// and presentation.
pub fn world_component(class_name: &str) -> Option<&'static WorldComponentRegistration> {
    find(class_name)
}

/// Whether a class's `requires` entry is attached with it
/// ([`required_class`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequiredClass {
    /// Attach it.
    Attach,
    /// Its plugin is absent and the requirer's plugin requires its contract
    /// optionally: the class is placed without it.
    Absent,
    /// Refused, with the reason: its plugin is not one the requirer's plugin
    /// requires (a `class` contract covering it), or it is required and not
    /// registered.
    Refused(String),
}

/// Whether `requirer` may attach class `required` when placed: only a class
/// of a plugin whose `class` contract covering it the requirer's plugin
/// requires (the engine's classes need none), so a plugin never places
/// another plugin's component it did not declare it uses.
pub fn required_class(requirer: &WorldComponentRegistration, required: &str) -> RequiredClass {
    use pulsar_plugins::{ContractKind, Owner};
    let requirer_plugin = match pulsar_plugins::owner_of(requirer.origin) {
        Owner::Plugin(plugin) => Some(plugin),
        _ => None,
    };
    // The installed plugin whose class contract covers `required`, and the
    // requirer's requirement on it.
    let provider = pulsar_plugins::installed().iter().find_map(|plugin| {
        plugin
            .provider_of(ContractKind::Class, required)
            .map(|c| (plugin, c))
    });
    let requirement = provider.and_then(|(_, contract)| {
        requirer_plugin.and_then(|plugin| plugin.requires.iter().find(|r| r.id == contract.id))
    });
    let registration = find(required);
    if let (Some(registration), Some(plugin)) = (registration, requirer_plugin) {
        let same_plugin = matches!(
            pulsar_plugins::owner_of(registration.origin),
            Owner::Plugin(owner) if owner.id == plugin.id
        );
        let engine = matches!(
            pulsar_plugins::owner_of(registration.origin),
            Owner::Engine(_)
        );
        if same_plugin || engine || requirement.is_some() {
            return RequiredClass::Attach;
        }
    } else if registration.is_some() {
        // An engine class requiring: no plugin to declare it.
        return RequiredClass::Attach;
    }
    match (registration, provider, requirement) {
        (None, Some(_), Some(requirement)) if requirement.optional => RequiredClass::Absent,
        (None, _, _) => RequiredClass::Refused(format!(
            "class {} requires class {required}, which is not registered (its plugin is disabled              or not installed)",
            requirer.class_name
        )),
        (Some(_), Some((plugin, contract)), None) => RequiredClass::Refused(format!(
            "class {} requires class {required} of plugin {}, but its plugin does not require              contract {}; {required} is not attached with it",
            requirer.class_name, plugin.id, contract.id
        )),
        (Some(_), _, _) => RequiredClass::Refused(format!(
            "class {} requires class {required}, which no installed plugin provides under a              class contract; it is not attached with it",
            requirer.class_name
        )),
    }
}

/// Every registered class's refused requirements ([`required_class`]):
/// what the loader reports when a project opens.
pub fn class_problems() -> Vec<String> {
    crate::runtime::world_components()
        .iter()
        .flat_map(|registration| {
            registration.requires.iter().filter_map(move |required| {
                match required_class(registration, required) {
                    RequiredClass::Refused(problem) => Some(problem),
                    _ => None,
                }
            })
        })
        .collect()
}

/// Every currently-registered `World`-backed class name (the add-component
/// menu, inventory checks).
pub fn registered_world_component_classes() -> impl Iterator<Item = &'static str> {
    crate::runtime::world_components()
        .iter()
        .copied()
        .map(|r| r.class_name)
}
