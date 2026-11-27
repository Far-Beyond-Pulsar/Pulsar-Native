//! The scene model's own components, read field by field.
//!
//! They are plain types without reflection registrations, and they are the
//! ones on nearly every entity, so the inspector names their fields here.
//! Anything else is read through reflection ([`super::detail`]).

use std::any::Any;
use std::mem::{offset_of, size_of};

use pulsar_scene_model::attachments::ClassSlot;
use pulsar_scene_model::{
    ComponentAttachments, ComponentMeta, ComponentOwner, ContentSpace, LevelInstanceMember,
    LevelInstanceRoot, Name, ObjectType, Parent, RigidFrame, Selected, SiblingIndex, SpaceSlot,
    StableId, Transform, UnresolvedComponent, Visibility,
};
use pulsar_scene_model::components::RenderProps;
use pulsar_scenedb::Entity;
use serde_json::{json, Value};

use super::fields::{entity_value, Field, FieldSet, FieldSource};
use super::hex::Region;

fn field(name: &str, type_label: &str, value: Value) -> Field {
    Field::new(name, type_label, value)
}

fn region(name: &str, type_label: &str, offset: usize, len: usize) -> Region {
    Region {
        name: name.to_string(),
        type_label: type_label.to_string(),
        offset,
        len,
    }
}

fn set(fields: Vec<Field>) -> FieldSet {
    FieldSet {
        source: FieldSource::SceneModel,
        fields,
    }
}

/// The fields and byte layout of `value`, if it is a scene model component.
pub fn decode(value: &dyn Any) -> Option<(FieldSet, Vec<Region>)> {
    // A whole-value region for the single-field types.
    let whole = |type_label: &str, len: usize| vec![region("value", type_label, 0, len)];

    if let Some(StableId(id)) = value.downcast_ref() {
        return Some((
            set(vec![field("id", "String", json!(id))]),
            whole("String", size_of::<String>()),
        ));
    }
    if let Some(Name(name)) = value.downcast_ref() {
        return Some((
            set(vec![field("name", "String", json!(name))]),
            whole("String", size_of::<String>()),
        ));
    }
    if let Some(transform) = value.downcast_ref::<Transform>() {
        return Some((
            set(vec![
                field("position", "[f64; 3]", json!(transform.position)),
                field("rotation", "[f32; 3] (degrees, YXZ)", json!(transform.rotation)),
                field("scale", "[f32; 3]", json!(transform.scale)),
            ]),
            vec![
                region("position", "[f64; 3]", offset_of!(Transform, position), 24),
                region("rotation", "[f32; 3]", offset_of!(Transform, rotation), 12),
                region("scale", "[f32; 3]", offset_of!(Transform, scale), 12),
            ],
        ));
    }
    if let Some(visibility) = value.downcast_ref::<Visibility>() {
        return Some((
            set(vec![
                field("visible", "bool", json!(visibility.visible)),
                field("locked", "bool", json!(visibility.locked)),
            ]),
            vec![
                region("visible", "bool", offset_of!(Visibility, visible), 1),
                region("locked", "bool", offset_of!(Visibility, locked), 1),
            ],
        ));
    }
    if let Some(Parent(parent)) = value.downcast_ref() {
        return Some((
            set(vec![field("parent", "Entity", entity_value(*parent))]),
            whole("Entity", size_of::<Entity>()),
        ));
    }
    if let Some(SiblingIndex(order)) = value.downcast_ref() {
        return Some((
            set(vec![field("order", "u64", json!(order))]),
            whole("u64", 8),
        ));
    }
    if let Some(selected) = value.downcast_ref::<Selected>() {
        return Some((
            set(vec![field("order", "u64", json!(selected.order))]),
            whole("u64", 8),
        ));
    }
    if let Some(object_type) = value.downcast_ref::<ObjectType>() {
        return Some((
            set(vec![field(
                "kind",
                "ObjectType",
                json!(format!("{object_type:?}")),
            )
            .as_variant()]),
            whole("ObjectType", size_of::<ObjectType>()),
        ));
    }
    if let Some(props) = value.downcast_ref::<RenderProps>() {
        let map: serde_json::Map<String, Value> = {
            let mut keys: Vec<&String> = props.props.keys().collect();
            keys.sort();
            keys.into_iter()
                .map(|key| (key.clone(), props.props[key].clone()))
                .collect()
        };
        return Some((
            set(vec![field("props", "HashMap<String, Value>", Value::Object(map))]),
            Vec::new(),
        ));
    }
    if let Some(owner) = value.downcast_ref::<ComponentOwner>() {
        return Some((
            set(vec![
                field("owner", "Entity", entity_value(owner.entity())),
                field("enabled", "bool", json!(owner.is_enabled())),
            ]),
            vec![
                region("owner_index", "u32", offset_of!(ComponentOwner, owner_index), 4),
                region(
                    "owner_generation",
                    "u32",
                    offset_of!(ComponentOwner, owner_generation),
                    4,
                ),
                region("enabled", "u32", offset_of!(ComponentOwner, enabled), 4),
            ],
        ));
    }
    if let Some(meta) = value.downcast_ref::<ComponentMeta>() {
        let slot = meta.class_slot.as_ref().map_or(Value::Null, slot_value);
        return Some((
            set(vec![
                field("class_name", "String", json!(meta.class_name)),
                field("id", "ComponentInstanceId", json!(meta.id.to_string())),
                field(
                    "parent",
                    "Option<ComponentInstanceId>",
                    json!(meta.parent.map(|parent| parent.to_string())),
                ),
                field("class_slot", "Option<ClassSlot>", slot),
            ]),
            vec![
                region("id", "u128", offset_of!(ComponentMeta, id), 16),
                region(
                    "class_name",
                    "String",
                    offset_of!(ComponentMeta, class_name),
                    size_of::<String>(),
                ),
            ],
        ));
    }
    if let Some(ComponentAttachments(instances)) = value.downcast_ref() {
        let list: Vec<Value> = instances.iter().map(|&entity| entity_value(entity)).collect();
        return Some((
            set(vec![field("instances", "Vec<Entity>", Value::Array(list))]),
            whole("Vec<Entity>", size_of::<Vec<Entity>>()),
        ));
    }
    if let Some(unresolved) = value.downcast_ref::<UnresolvedComponent>() {
        return Some((
            FieldSet {
                source: FieldSource::Unresolved,
                fields: vec![
                    field("reason", "String", json!(unresolved.reason)),
                    field("data", "stored payload", unresolved.data.clone()),
                ],
            },
            Vec::new(),
        ));
    }
    if let Some(member) = value.downcast_ref::<LevelInstanceMember>() {
        return Some((
            set(vec![field("root", "Entity", entity_value(member.root))]),
            whole("Entity", size_of::<Entity>()),
        ));
    }
    if let Some(ContentSpace(space)) = value.downcast_ref() {
        return Some((
            set(vec![field("space", "u32", json!(space))]),
            whole("u32", 4),
        ));
    }
    if let Some(slot) = value.downcast_ref::<SpaceSlot>() {
        return Some((
            set(vec![field("slot", "u32", json!(slot.slot))]),
            whole("u32", 4),
        ));
    }
    if value.is::<LevelInstanceRoot>() || value.is::<RigidFrame>() {
        return Some((set(Vec::new()), Vec::new()));
    }
    None
}

fn slot_value(slot: &ClassSlot) -> Value {
    json!({
        "slot_id": slot.slot_id,
        "local_transform": slot.local_transform.map(|transform| json!({
            "position": transform.position,
            "rotation": transform.rotation,
            "scale": transform.scale,
        })),
    })
}
