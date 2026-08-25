//! Identity types through the reflection layer (#642).
//!
//! Goal: `Entity`/`ActorRef`/`ComponentRef` ride `pulsar_reflection`'s
//! registries like any other value, so reflected methods can take and
//! return object references (C's dispatcher, D's VM args, F's pins all
//! consume this) and the Blueprint pin system sees them as real, colorable
//! types instead of opaque integers.
//!
//! ## What is a trait impl vs. a registration shim
//!
//! - `ActorRef`/`ComponentRef` are OUR types: full manual
//!   [`Reflectable`] impls below.
//! - `Entity` (and the gap-filling `u32`) are foreign, pinned-crate types:
//!   coherence forbids `impl ForeignTrait for ForeignType`, so they get
//!   **registration shims** -- hand-written
//!   `inventory::submit!(RuntimeTypeRegistration { .. })` entries keyed by
//!   their TypeId. The registry path (`serialize_json_for_any`,
//!   `deserialize_json_for_type`, struct-field marshalling) works purely
//!   off those fn-pointer entries; no trait impl is needed for any of it.
//!   UPSTREAM ASK: move the Entity entry into SceneDB (or reflection's
//!   prims) so it also gains the trait impl + a proper `{slot, generation}`
//!   structure if pins ever want to display generation separately; until
//!   then JSON carries the packed bits (`Entity::bits()`), matching
//!   SceneDB's own "Handle must pass as u64" README rule.
//!
//! ## Marshalling rules (which identity crosses which boundary in which form)
//!
//! | Boundary | Form | Conversion point |
//! |---|---|---|
//! | Rust ↔ Rust through this crate | `ActorRef`/`ComponentRef` values | none |
//! | Reflection args/returns | `Box<dyn Any>` holding the concrete ref type | caller boxes / callee downcasts |
//! | Raw FFI / dylib (PIE ABI v2) | `u64` = `Entity::bits()` ONLY | glue converts before validation |
//! | JSON at rest (graphs, saves) | [`SerializedComponentRef`] (stable_id) or entity-bits number for transient state | [`crate::resolution`] / these shims |
//!
//! A stale reference crossing ANY boundary stays a typed error at first
//! use ([`crate::contract`]) -- marshalling never validates, accessors do.

use pulsar_reflection::{
    Reflectable, ReflectError, ReflectResult, RuntimeTypeInfo, TypeDeserializer, TypeSerializer,
    TypeStructure, WrapperType,
};
#[cfg(test)]
use pulsar_reflection::RUNTIME_TYPE_REGISTRY;
use pulsar_scenedb::Entity;
use serde_json::{json, Value};

use crate::refs::{ActorRef, ComponentRef};

// ── static type descriptors ────────────────────────────────────────────────

/// `Entity`'s descriptor: primitive-shaped (the packed u64 IS the type's
/// entire JSON story; see the module doc for the upstream-ask caveat).
static ENTITY_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: std::any::TypeId::of::<Entity>(),
    type_name: "pulsar_scenedb::Entity",
    size: std::mem::size_of::<Entity>(),
    align: std::mem::align_of::<Entity>(),
    structure: TypeStructure::Primitive,
    color: Some("#56D364"),
};

/// `u32` is missing from upstream's prim set but appears in our identity
/// structs (`component_index`) -- registered here so struct marshalling
/// works; same upstream-ask note as Entity.
static U32_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: std::any::TypeId::of::<u32>(),
    type_name: "u32",
    size: std::mem::size_of::<u32>(),
    align: std::mem::align_of::<u32>(),
    structure: TypeStructure::Primitive,
    color: None,
};

/// `String`'s descriptor FOR FIELD METADATA ONLY. Deliberately NOT
/// registered here -- upstream's prim set owns String's registry entry;
/// duplicating it could overwrite theirs depending on link order. Struct
/// marshalling resolves field values BY TYPE ID, so this local descriptor
/// (same TypeId) routes through upstream's entry without ever touching the
/// registry during descriptor construction -- which would deadlock (see
/// COMPONENT_REF_TYPE_INFO below).
static STRING_FIELD_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: std::any::TypeId::of::<String>(),
    type_name: "alloc::string::String",
    size: std::mem::size_of::<String>(),
    align: std::mem::align_of::<String>(),
    structure: TypeStructure::String,
    color: None,
};

static ACTOR_REF_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: std::any::TypeId::of::<ActorRef>(),
    type_name: "pulsar_script_object_model::ActorRef",
    size: std::mem::size_of::<ActorRef>(),
    align: std::mem::align_of::<ActorRef>(),
    structure: TypeStructure::Wrapper {
        wrapper_kind: WrapperType::Custom("ActorRef"),
        inner: &ENTITY_TYPE_INFO,
    },
    color: Some("#F0883E"),
};

/// `ComponentRef`'s descriptor. A plain static -- deliberately constructed
/// WITHOUT touching `RUNTIME_TYPE_REGISTRY`: the registry's own build calls
/// every registration's `type_info()` fn, so any re-entrant registry access
/// here would deadlock on its initialization lock.
static COMPONENT_REF_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: std::any::TypeId::of::<ComponentRef>(),
    type_name: "pulsar_script_object_model::ComponentRef",
    size: std::mem::size_of::<ComponentRef>(),
    align: std::mem::align_of::<ComponentRef>(),
    structure: TypeStructure::Struct {
        fields: &[
            pulsar_reflection::FieldInfo {
                name: "entity",
                type_info: &ENTITY_TYPE_INFO,
                offset: std::mem::offset_of!(ComponentRef, entity),
            },
            pulsar_reflection::FieldInfo {
                name: "class_name",
                type_info: &STRING_FIELD_TYPE_INFO,
                offset: std::mem::offset_of!(ComponentRef, class_name),
            },
            pulsar_reflection::FieldInfo {
                name: "component_index",
                type_info: &U32_TYPE_INFO,
                offset: std::mem::offset_of!(ComponentRef, component_index),
            },
        ],
    },
    color: Some("#58A6FF"),
};

// ── public descriptor access ───────────────────────────────────────────────

/// Static type info for `Entity` -- what method metadata and pin renderers
/// cite when naming this type without needing the trait impl.
pub fn entity_type_info() -> &'static RuntimeTypeInfo {
    &ENTITY_TYPE_INFO
}

/// Static type info for `ActorRef`.
pub fn actor_ref_type_info() -> &'static RuntimeTypeInfo {
    &ACTOR_REF_TYPE_INFO
}

/// Static type info for `ComponentRef`.
pub fn component_ref_type_info() -> &'static RuntimeTypeInfo {
    &COMPONENT_REF_TYPE_INFO
}

// ── Reflectable for our own handle types ───────────────────────────────────

impl Reflectable for ActorRef {
    fn type_info() -> &'static RuntimeTypeInfo {
        &ACTOR_REF_TYPE_INFO
    }

    fn serialize(&self, serializer: &mut dyn TypeSerializer) -> ReflectResult<()> {
        serializer.serialize_registered(&self.0)
    }

    fn deserialize(deserializer: &mut dyn TypeDeserializer) -> ReflectResult<Self> {
        let boxed = deserializer.deserialize_registered(&ACTOR_REF_TYPE_INFO)?;
        let found = format!("{:?}", (&*boxed).type_id());
        boxed
            .downcast::<ActorRef>()
            .map(|value| *value)
            .map_err(|_| ReflectError::TypeMismatch { expected: "ActorRef", found })
    }

    fn clone_any(&self) -> Box<dyn std::any::Any> {
        Box::new(*self)
    }
}

impl Reflectable for ComponentRef {
    fn type_info() -> &'static RuntimeTypeInfo {
        &COMPONENT_REF_TYPE_INFO
    }

    fn serialize(&self, serializer: &mut dyn TypeSerializer) -> ReflectResult<()> {
        serializer.serialize_struct(&[
            ("entity", &self.entity),
            ("class_name", &self.class_name),
            ("component_index", &self.component_index),
        ])
    }

    fn deserialize(deserializer: &mut dyn TypeDeserializer) -> ReflectResult<Self> {
        let mut fields =
            deserializer.deserialize_struct(Self::type_info().fields().ok_or_else(|| {
                ReflectError::DeserializationFailed("ComponentRef is not a struct".into())
            })?)?;
        let take = |fields: &mut std::collections::HashMap<
            &'static str,
            Box<dyn std::any::Any>,
        >,
                    name: &'static str| {
            fields.remove(name).ok_or_else(|| ReflectError::MissingField {
                struct_name: "ComponentRef",
                field_name: name,
            })
        };
        let entity = *take(&mut fields, "entity")?
            .downcast::<Entity>()
            .map_err(|_| ReflectError::TypeMismatch { expected: "Entity", found: "other".into() })?;
        let class_name = *take(&mut fields, "class_name")?
            .downcast::<String>()
            .map_err(|_| ReflectError::TypeMismatch { expected: "String", found: "other".into() })?;
        let component_index = *take(&mut fields, "component_index")?
            .downcast::<u32>()
            .map_err(|_| ReflectError::TypeMismatch { expected: "u32", found: "other".into() })?;
        Ok(Self { entity, class_name, component_index })
    }

    fn clone_any(&self) -> Box<dyn std::any::Any> {
        Box::new(self.clone())
    }
}

// ── registry shims for the foreign types ───────────────────────────────────

fn serialize_entity_json(value: &dyn std::any::Any) -> ReflectResult<Value> {
    let entity =
        value.downcast_ref::<Entity>().ok_or_else(|| ReflectError::TypeMismatch {
            expected: "pulsar_scenedb::Entity",
            found: format!("{:?}", value.type_id()),
        })?;
    Ok(json!(entity.bits()))
}

fn deserialize_entity_json(value: Value) -> ReflectResult<Entity> {
    let bits = value.as_u64().ok_or_else(|| ReflectError::TypeMismatch {
        expected: "packed entity u64",
        found: format!("{value}"),
    })?;
    Ok(Entity::from_bits(bits))
}

fn serialize_u32_json(value: &dyn std::any::Any) -> ReflectResult<Value> {
    let n = value.downcast_ref::<u32>().ok_or_else(|| ReflectError::TypeMismatch {
        expected: "u32",
        found: format!("{:?}", value.type_id()),
    })?;
    Ok(json!(n))
}

fn deserialize_u32_json(value: Value) -> ReflectResult<u32> {
    // Accept only values that fit exactly; a float/overflowed value is a
    // hard mismatch, not a truncation.
    if value.is_u64() && value.as_u64().map(|n| n <= u32::MAX as u64).unwrap_or(false) {
        Ok(value.as_u64().unwrap() as u32)
    } else {
        Err(ReflectError::TypeMismatch {
            expected: "u32",
            found: format!("{value}"),
        })
    }
}

inventory::submit! {
    pulsar_reflection::RuntimeTypeRegistration {
        type_info: entity_type_info,
        serialize_json: serialize_entity_json,
        deserialize_json: |value| {
            deserialize_entity_json(value).map(|e| Box::new(e) as Box<dyn std::any::Any>)
        },
    }
}

inventory::submit! {
    pulsar_reflection::RuntimeTypeRegistration {
        type_info: || &U32_TYPE_INFO,
        serialize_json: serialize_u32_json,
        deserialize_json: |value| {
            deserialize_u32_json(value).map(|n| Box::new(n) as Box<dyn std::any::Any>)
        },
    }
}

fn serialize_actor_ref_json(value: &dyn std::any::Any) -> ReflectResult<Value> {
    let r = value.downcast_ref::<ActorRef>().ok_or_else(|| ReflectError::TypeMismatch {
        expected: "ActorRef",
        found: format!("{:?}", value.type_id()),
    })?;
    Ok(json!(r.entity().bits()))
}

fn deserialize_actor_ref_json(value: Value) -> ReflectResult<ActorRef> {
    deserialize_entity_json(value).map(ActorRef::new)
}

fn serialize_component_ref_json(value: &dyn std::any::Any) -> ReflectResult<Value> {
    let r = value
        .downcast_ref::<ComponentRef>()
        .ok_or_else(|| ReflectError::TypeMismatch {
            expected: "ComponentRef",
            found: format!("{:?}", value.type_id()),
        })?;
    let mut serializer = pulsar_reflection::JsonSerializer::new();
    Reflectable::serialize(r, &mut serializer)?;
    Ok(serializer.into_json())
}

fn deserialize_component_ref_json(value: Value) -> ReflectResult<ComponentRef> {
    let mut deserializer = pulsar_reflection::JsonDeserializer::new(value);
    Reflectable::deserialize(&mut deserializer)
}

inventory::submit! {
    pulsar_reflection::RuntimeTypeRegistration {
        type_info: actor_ref_type_info,
        serialize_json: serialize_actor_ref_json,
        deserialize_json: |value| {
            deserialize_actor_ref_json(value).map(|r| Box::new(r) as Box<dyn std::any::Any>)
        },
    }
}

inventory::submit! {
    pulsar_reflection::RuntimeTypeRegistration {
        type_info: component_ref_type_info,
        serialize_json: serialize_component_ref_json,
        deserialize_json: |value| {
            deserialize_component_ref_json(value).map(|r| Box::new(r) as Box<dyn std::any::Any>)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_reflection::{JsonDeserializer, JsonSerializer};

    /// #642: Entity is registered end-to-end through the registry shim --
    /// lookup by TypeId AND by name both find it, and JSON carries the
    /// packed bits.
    #[test]
    fn entity_is_registered_with_json_round_trip() {
        let info = RUNTIME_TYPE_REGISTRY.get_by_id(std::any::TypeId::of::<Entity>());
        assert!(info.is_some(), "Entity must be registered");
        assert_eq!(info.unwrap().base_name(), "Entity");

        let entity = Entity::from_bits((7u64 << 32) | 3);
        let json = RUNTIME_TYPE_REGISTRY
            .serialize_json_for_any(&entity)
            .expect("serializes via shim");
        assert_eq!(json, serde_json::json!((7u64 << 32) | 3));

        let back = RUNTIME_TYPE_REGISTRY
            .deserialize_json_for_type(entity_type_info(), json)
            .expect("deserializes via shim");
        assert_eq!(back.downcast_ref::<Entity>(), Some(&entity));
    }

    /// #642: u32 (the component_index field type) marshals through the
    /// registry -- and rejects out-of-range/typed garbage instead of
    /// truncating.
    #[test]
    fn u32_shim_round_trips_and_rejects_garbage() {
        let json = RUNTIME_TYPE_REGISTRY.serialize_json_for_any(&42u32).unwrap();
        assert_eq!(json, serde_json::json!(42));
        let back = RUNTIME_TYPE_REGISTRY.deserialize_json_for_type(&U32_TYPE_INFO, json).unwrap();
        assert_eq!(back.downcast_ref::<u32>(), Some(&42));

        assert!(RUNTIME_TYPE_REGISTRY
            .deserialize_json_for_type(&U32_TYPE_INFO, serde_json::json!(u32::MAX as u64 + 1))
            .is_err());
        assert!(RUNTIME_TYPE_REGISTRY
            .deserialize_json_for_type(&U32_TYPE_INFO, serde_json::json!("nope"))
            .is_err());
    }

    /// #642: ActorRef implements Reflectable end-to-end -- trait serialize
    /// through JsonSerializer, trait deserialize back, registry path too.
    #[test]
    fn actor_ref_reflectable_round_trip() {
        let r = ActorRef::new(Entity::from_bits((9u64 << 32) | 4));

        let mut serializer = JsonSerializer::new();
        Reflectable::serialize(&r, &mut serializer).unwrap();
        let json = serializer.into_json();
        assert_eq!(json, serde_json::json!((9u64 << 32) | 4));

        let mut deserializer = JsonDeserializer::new(json);
        let back = ActorRef::deserialize(&mut deserializer).unwrap();
        assert_eq!(back, r);

        // Registry path (what C's marshalling will use).
        let any_json = RUNTIME_TYPE_REGISTRY.serialize_json_for_any(&r).unwrap();
        let back_any =
            RUNTIME_TYPE_REGISTRY.deserialize_json_for_type(actor_ref_type_info(), any_json).unwrap();
        assert_eq!(back_any.downcast_ref::<ActorRef>(), Some(&r));
    }

    /// #642: ComponentRef serializes as a real struct ({entity, class_name,
    /// component_index}) through BOTH the trait path and the registry path.
    #[test]
    fn component_ref_reflectable_struct_round_trip() {
        let r = ComponentRef {
            entity: Entity::from_bits((2u64 << 32) | 11),
            class_name: "TestGizmo".into(),
            component_index: 3,
        };

        let mut serializer = JsonSerializer::new();
        Reflectable::serialize(&r, &mut serializer).unwrap();
        let json = serializer.into_json();
        assert_eq!(
            json,
            serde_json::json!({
                "entity": (2u64 << 32) | 11,
                "class_name": "TestGizmo",
                "component_index": 3,
            })
        );

        let mut deserializer = JsonDeserializer::new(json);
        assert_eq!(ComponentRef::deserialize(&mut deserializer).unwrap(), r);

        let any_json = RUNTIME_TYPE_REGISTRY.serialize_json_for_any(&r).unwrap();
        let back_any = RUNTIME_TYPE_REGISTRY
            .deserialize_json_for_type(component_ref_type_info(), any_json)
            .unwrap();
        assert_eq!(back_any.downcast_ref::<ComponentRef>(), Some(&r));
    }

    /// #642: the descriptors are stable statics with distinct declared pin
    /// colors (F's graph pins color/filter by these).
    #[test]
    fn descriptors_expose_distinct_pin_colors() {
        let colors = [
            entity_type_info().color,
            actor_ref_type_info().color,
            component_ref_type_info().color,
        ];
        assert!(colors.iter().all(|c| c.is_some()));
        assert_eq!(
            colors.iter().collect::<std::collections::HashSet<_>>().len(),
            3,
            "identity types need visually distinct pins"
        );
    }
}
