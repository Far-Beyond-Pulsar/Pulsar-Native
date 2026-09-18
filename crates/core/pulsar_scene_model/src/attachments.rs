//! Component-instance attachments stored on scene entities.
//!
//! Live registered components are typed World components and are authoritative.
//! [`ComponentAttachments`] is the per-entity record of *which* components an
//! object has: their order, enabled state and any dormant / unregistered payloads
//! that have no live typed value.

use pulsar_scenedb::{Entity, World};

use crate::instance::ComponentInstance;

/// Order, enabled state, and dormant/unregistered component payloads.
/// Live registered payloads are read from their typed World components.
#[derive(Clone, Default)]
pub struct ComponentAttachments(pub Vec<ComponentInstance>);

/// The component instances attached to `entity`, in order.
pub fn get_components(world: &World, entity: Entity) -> Vec<ComponentInstance> {
    world
        .get::<ComponentAttachments>(entity)
        .map(|attachments| attachments.0.clone())
        .unwrap_or_default()
}

/// Edit `entity`'s attachment list in place. `edit` returns whether it changed
/// anything; nothing is written back when it returns `false`.
pub fn edit_components(
    world: &mut World,
    entity: Entity,
    edit: impl FnOnce(&mut Vec<ComponentInstance>) -> bool,
) -> bool {
    if !world.is_alive(entity) {
        return false;
    }
    let mut attachments = world
        .get::<ComponentAttachments>(entity)
        .cloned()
        .unwrap_or_default();
    if !edit(&mut attachments.0) {
        return false;
    }
    world.insert(entity, attachments);
    true
}

pub fn add_component_instance(world: &mut World, entity: Entity, component: ComponentInstance) {
    edit_components(world, entity, |items| {
        items.push(component);
        true
    });
}

pub fn replace_components(world: &mut World, entity: Entity, components: Vec<ComponentInstance>) {
    edit_components(world, entity, |items| {
        *items = components;
        true
    });
}

pub fn remove_component(world: &mut World, entity: Entity, index: usize) -> bool {
    edit_components(world, entity, |items| {
        if index >= items.len() {
            return false;
        }
        items.remove(index);
        true
    })
}

pub fn update_component(
    world: &mut World,
    entity: Entity,
    index: usize,
    data: serde_json::Value,
) -> bool {
    edit_components(world, entity, |items| {
        let Some(component) = items.get_mut(index) else {
            return false;
        };
        component.data = data;
        true
    })
}

pub fn clear_components(world: &mut World, entity: Entity) {
    replace_components(world, entity, Vec::new());
}
