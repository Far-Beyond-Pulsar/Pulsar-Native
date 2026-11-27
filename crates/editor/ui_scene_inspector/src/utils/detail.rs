//! One entity's components, and one component's value: its typed fields and
//! its bytes.
//!
//! Everything here reads one `&World` borrow and returns owned data, so the
//! scene's lock is held only for the read.

use std::any::Any;
use std::sync::Arc;

use pulsar_reflection::{EngineClass, TypeStructure, RUNTIME_TYPE_REGISTRY};
use pulsar_scene_model::attachments;
use pulsar_scene_model::{ComponentMeta, ComponentOwner, Name, StableId, Transform};
use pulsar_scenedb::{ComponentId, Entity, World};

use super::fields::{error_value, Field, FieldSet, FieldSource};
use super::format;
use super::hex::{normalize_regions, Region};

/// One component of one entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ComponentRef {
    pub entity: Entity,
    pub id: ComponentId,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComponentItem {
    pub target: ComponentRef,
    pub name: Arc<str>,
    pub full: &'static str,
    pub class: Option<&'static str>,
    pub size: usize,
}

/// Components shown together: the entity's own, or those of one component
/// instance attached to it.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub entity: Entity,
    pub title: String,
    /// `Some` for an attached instance: whether it is enabled.
    pub enabled: Option<bool>,
    pub items: Vec<ComponentItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EntityDetail {
    pub entity: Entity,
    pub label: String,
    pub stable_id: Option<String>,
    pub archetype: u32,
    pub row: usize,
    /// For a component instance, the object it is attached to.
    pub owner: Option<(Entity, String)>,
    pub sections: Vec<Section>,
}

impl EntityDetail {
    pub fn items(&self) -> impl Iterator<Item = &ComponentItem> {
        self.sections.iter().flat_map(|section| &section.items)
    }

    /// The component to show when this entity is selected: `wanted` itself
    /// if listed, else a component of the same type (so stepping through
    /// entities compares one component), else the first that says the most.
    pub fn pick(&self, wanted: Option<ComponentRef>) -> Option<ComponentRef> {
        if let Some(wanted) = wanted {
            if self.items().any(|item| item.target == wanted) {
                return Some(wanted);
            }
            if let Some(item) = self.items().find(|item| item.target.id == wanted.id) {
                return Some(item.target);
            }
        }
        let transform = pulsar_scenedb::component_id::<Transform>();
        self.items()
            .find(|item| item.target.id == transform || item.class.is_some())
            .or_else(|| self.items().next())
            .map(|item| item.target)
    }
}

fn label_of(world: &World, entity: Entity) -> String {
    if let Some(meta) = world.get::<ComponentMeta>(entity) {
        return meta.class_name.clone();
    }
    match world.get::<Name>(entity) {
        Some(name) if !name.0.is_empty() => name.0.clone(),
        _ => world
            .get::<StableId>(entity)
            .map_or_else(|| format!("Entity {}", format::entity(entity)), |id| id.0.clone()),
    }
}

/// The components of one entity: its instance's class value first, then
/// `Transform` and `Name`, then by name.
fn items(world: &World, entity: Entity) -> Vec<ComponentItem> {
    let class = world
        .get::<ComponentMeta>(entity)
        .and_then(|meta| pulsar_world_registry::component_id_for_class(&meta.class_name));
    let classes = super::index::class_names();
    let mut items: Vec<ComponentItem> = world
        .component_ids(entity)
        .filter_map(|id| {
            let size = std::mem::size_of_val(world.get_dyn(entity, id)?);
            let full = pulsar_scenedb::component::type_name(id);
            Some(ComponentItem {
                target: ComponentRef { entity, id },
                name: format::short_type(full).into(),
                full,
                class: classes.get(&id).copied(),
                size,
            })
        })
        .collect();
    let transform = pulsar_scenedb::component_id::<Transform>();
    let name = pulsar_scenedb::component_id::<Name>();
    items.sort_by(|a, b| {
        let rank = |item: &ComponentItem| match item.target.id {
            id if Some(id) == class => 0,
            id if id == transform => 1,
            id if id == name => 2,
            _ => 3,
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.full.cmp(b.full))
    });
    items
}

pub fn read_detail(world: &World, entity: Entity) -> Option<EntityDetail> {
    if !world.is_alive(entity) {
        return None;
    }
    let mut ids: Vec<ComponentId> = world.component_ids(entity).collect();
    ids.sort_unstable();
    let (archetype, row) = world
        .archetypes
        .iter()
        .filter(|archetype| archetype.key.0 == ids)
        .find_map(|archetype| {
            let row = archetype.entities.iter().position(|&e| e == entity)?;
            Some((archetype.id.0, row))
        })?;

    let mut sections = vec![Section {
        entity,
        title: "Components".to_string(),
        enabled: None,
        items: items(world, entity),
    }];
    for instance in attachments::instances(world, entity) {
        if !world.is_alive(instance) {
            continue;
        }
        sections.push(Section {
            entity: instance,
            title: label_of(world, instance),
            enabled: Some(attachments::is_enabled(world, instance)),
            items: items(world, instance),
        });
    }

    let owner = world
        .get::<ComponentOwner>(entity)
        .map(ComponentOwner::entity)
        .filter(|&owner| world.is_alive(owner))
        .map(|owner| (owner, label_of(world, owner)));

    Some(EntityDetail {
        entity,
        label: label_of(world, entity),
        stable_id: world.get::<StableId>(entity).map(|id| id.0.clone()),
        archetype,
        row,
        owner,
        sections,
    })
}

#[derive(Clone, Debug)]
pub struct ComponentData {
    pub target: ComponentRef,
    pub full: &'static str,
    pub name: String,
    pub align: usize,
    /// The value as it lies in its column.
    pub bytes: Arc<[u8]>,
    /// `None` when nothing describes the type: only the bytes can be shown.
    pub fields: Option<FieldSet>,
    /// The named byte ranges of the value, from its type's layout.
    pub regions: Vec<Region>,
}

/// The bytes of a component value, as it lies in its column. Heap data (a
/// `String`'s text, a `Vec`'s items) is not part of them, only the pointer,
/// length and capacity that lead to it.
pub fn value_bytes(value: &dyn Any) -> Vec<u8> {
    let len = std::mem::size_of_val(value);
    let start = (value as *const dyn Any).cast::<u8>();
    // SAFETY: `start` and `len` are the address and size of the live value
    // `value` borrows, so the range is in bounds and nothing writes it while
    // the borrow is held. The bytes are only copied out and displayed, never
    // given a meaning the program acts on (the same dump SceneDB's own
    // telemetry takes). Padding bytes among them hold whatever the memory
    // held.
    unsafe { std::slice::from_raw_parts(start, len) }.to_vec()
}

pub fn read_component(world: &World, target: ComponentRef) -> Option<ComponentData> {
    let value = world.get_dyn(target.entity, target.id)?;
    let full = pulsar_scenedb::component::type_name(target.id);
    let bytes: Arc<[u8]> = value_bytes(value).into();
    let (fields, regions) = decode(world, target, value);
    Some(ComponentData {
        target,
        full,
        name: format::short_type(full),
        align: std::mem::align_of_val(value),
        regions: normalize_regions(regions, bytes.len()),
        bytes,
        fields,
    })
}

/// The typed view of a value: the scene model's own types, then a component
/// class's reflected properties, then the type's runtime reflection.
fn decode(world: &World, target: ComponentRef, value: &dyn Any) -> (Option<FieldSet>, Vec<Region>) {
    if let Some((set, regions)) = super::builtins::decode(value) {
        return (Some(set), regions);
    }
    let regions = reflected_regions(value);
    let class = super::index::class_names().get(&target.id).copied();
    if let Some(class) = class {
        if let Some(value) =
            pulsar_world_registry::get_world_component_as_engine_class(class, world, target.entity)
        {
            if let Some(set) = class_fields(value) {
                return (Some(set), regions);
            }
        }
    }
    (reflected_fields(value), regions)
}

/// A component class's `#[property]` fields, read through their getters.
pub fn class_fields(value: &dyn EngineClass) -> Option<FieldSet> {
    let properties = value.get_properties();
    if properties.is_empty() {
        return None;
    }
    let fields = properties
        .iter()
        .map(|property| {
            let current = (property.getter)(value);
            let json = RUNTIME_TYPE_REGISTRY
                .serialize_json_for_any(current.as_ref())
                .unwrap_or_else(|error| error_value(error.to_string()));
            let mut field = Field::typed(property.display_name.clone(), property.type_info, json);
            field.category = property.category.map(str::to_string);
            field
        })
        .collect();
    Some(FieldSet {
        source: FieldSource::Class,
        fields,
    })
}

/// The fields of a type registered with runtime reflection.
fn reflected_fields(value: &dyn Any) -> Option<FieldSet> {
    let info = RUNTIME_TYPE_REGISTRY.get_by_id(value.type_id())?;
    let json = match RUNTIME_TYPE_REGISTRY.serialize_json_for_any(value) {
        Ok(json) => json,
        // One member without a serializer fails the whole struct. Its layout
        // is still known: name every field and say why none can be read.
        Err(error) => {
            let error = error.to_string();
            let fields = match info.structure {
                TypeStructure::Struct { fields } => fields
                    .iter()
                    .map(|field| {
                        Field::typed(field.name, field.type_info, error_value(error.clone()))
                    })
                    .collect(),
                _ => vec![Field::typed("value", info, error_value(error))],
            };
            return Some(FieldSet {
                source: FieldSource::Reflected,
                fields,
            });
        }
    };
    let fields = match (&info.structure, json) {
        (TypeStructure::Struct { fields }, serde_json::Value::Object(mut map)) => {
            // Declaration order, then any member serialization added.
            let mut out: Vec<Field> = fields
                .iter()
                .filter_map(|field| {
                    let value = map.remove(field.name)?;
                    Some(Field::typed(field.name, field.type_info, value))
                })
                .collect();
            out.extend(
                map.into_iter()
                    .map(|(name, value)| Field::new(name, "", value)),
            );
            out
        }
        (_, json) => vec![Field::typed("value", info, json)],
    };
    Some(FieldSet {
        source: FieldSource::Reflected,
        fields,
    })
}

/// The byte ranges a reflected struct's fields occupy.
fn reflected_regions(value: &dyn Any) -> Vec<Region> {
    let Some(info) = RUNTIME_TYPE_REGISTRY.get_by_id(value.type_id()) else {
        return Vec::new();
    };
    let TypeStructure::Struct { fields } = info.structure else {
        return Vec::new();
    };
    fields
        .iter()
        .map(|field| Region {
            name: field.name.to_string(),
            type_label: format::short_type(field.type_info.type_name),
            offset: field.offset,
            len: field.type_info.size,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_scene_model::attachments::NewInstance;
    use pulsar_scene_model::UnresolvedComponent;

    struct Opaque(#[allow(dead_code)] u32);

    fn object(world: &mut World) -> Entity {
        let entity = world.spawn();
        world.insert(entity, StableId("crate".into()));
        world.insert(entity, Name("Crate".into()));
        world.insert(
            entity,
            Transform {
                position: [1.0, 2.0, 3.0],
                rotation: [0.0; 3],
                scale: [1.0; 3],
            },
        );
        entity
    }

    #[test]
    fn an_object_lists_its_components_and_its_attached_instances() {
        let mut world = World::new();
        let owner = object(&mut world);
        let instance =
            attachments::spawn_instance(&mut world, owner, NewInstance::new("Mystery")).unwrap();
        world.insert(
            instance,
            UnresolvedComponent {
                data: serde_json::json!({ "speed": 4 }),
                reason: "unknown class".into(),
            },
        );

        let detail = read_detail(&world, owner).unwrap();
        assert_eq!(detail.label, "Crate");
        assert_eq!(&*detail.sections[0].items[0].name, "Transform");
        assert_eq!(detail.sections[1].title, "Mystery");
        assert_eq!(detail.sections[1].enabled, Some(true));
        let archetype = world
            .archetypes
            .iter()
            .find(|a| a.id.0 == detail.archetype)
            .unwrap();
        assert_eq!(archetype.entities[detail.row], owner);

        let of_instance = read_detail(&world, instance).unwrap();
        assert_eq!(of_instance.owner, Some((owner, "Crate".to_string())));

        world.despawn(owner);
        assert!(read_detail(&world, owner).is_none());
    }

    #[test]
    fn selection_keeps_to_a_component_type_across_entities() {
        let mut world = World::new();
        let (a, b) = (object(&mut world), object(&mut world));
        let name = pulsar_scenedb::component_id::<Name>();
        let detail = read_detail(&world, b).unwrap();
        assert_eq!(
            detail.pick(Some(ComponentRef { entity: a, id: name })),
            Some(ComponentRef { entity: b, id: name })
        );
        assert_eq!(
            detail.pick(None).unwrap().id,
            pulsar_scenedb::component_id::<Transform>()
        );
    }

    #[test]
    fn a_component_reads_as_fields_and_bytes_with_their_layout() {
        let mut world = World::new();
        let entity = object(&mut world);
        let target = ComponentRef {
            entity,
            id: pulsar_scenedb::component_id::<Transform>(),
        };
        let data = read_component(&world, target).unwrap();
        assert_eq!(data.bytes.len(), std::mem::size_of::<Transform>());
        let fields = data.fields.unwrap();
        assert_eq!(fields.source, FieldSource::SceneModel);
        assert_eq!(fields.value_at("0/1"), Some(&serde_json::json!(2.0)));
        // The layout names the bytes the values are in.
        let position = data.regions.iter().find(|r| r.name == "position").unwrap();
        let y = &data.bytes[position.offset + 8..position.offset + 16];
        assert_eq!(f64::from_le_bytes(y.try_into().unwrap()), 2.0);
    }

    #[derive(Clone, pulsar_reflection::Reflectable)]
    struct Tuning {
        stiffness: f32,
        offsets: [f32; 3],
    }

    /// A field type with no serializer.
    #[derive(Clone, pulsar_reflection::Reflectable)]
    struct Partly {
        stiffness: f32,
        order: Vec<u32>,
    }

    #[test]
    fn a_struct_that_cannot_serialize_still_names_its_fields() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            Partly {
                stiffness: 1.0,
                order: Default::default(),
            },
        );
        let target = ComponentRef {
            entity,
            id: pulsar_scenedb::component_id::<Partly>(),
        };
        let fields = read_component(&world, target).unwrap().fields.unwrap();
        let labels: Vec<&str> = fields.fields.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, ["stiffness", "order"]);
        assert!(fields.fields[1].value.get("$error").is_some());
    }

    #[test]
    fn a_reflected_type_reads_as_its_declared_fields_and_layout() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            Tuning {
                stiffness: 2.5,
                offsets: [1.0, 2.0, 3.0],
            },
        );
        let target = ComponentRef {
            entity,
            id: pulsar_scenedb::component_id::<Tuning>(),
        };
        let data = read_component(&world, target).unwrap();
        let fields = data.fields.expect("a Reflectable type has fields");
        assert_eq!(fields.source, FieldSource::Reflected);
        let labels: Vec<&str> = fields.fields.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, ["stiffness", "offsets"], "declaration order");
        assert_eq!(fields.value_at("0"), Some(&serde_json::json!(2.5)));
        assert_eq!(fields.value_at("1/1"), Some(&serde_json::json!(2.0)));
        let stiffness = data.regions.iter().find(|r| r.name == "stiffness").unwrap();
        let bytes = &data.bytes[stiffness.offset..stiffness.offset + stiffness.len];
        assert_eq!(f32::from_le_bytes(bytes.try_into().unwrap()), 2.5);
    }

    #[test]
    fn an_undescribed_type_still_shows_its_bytes() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Opaque(0x0403_0201));
        let target = ComponentRef {
            entity,
            id: pulsar_scenedb::component_id::<Opaque>(),
        };
        let data = read_component(&world, target).unwrap();
        assert!(data.fields.is_none());
        assert_eq!(&*data.bytes, [1, 2, 3, 4]);
    }
}
