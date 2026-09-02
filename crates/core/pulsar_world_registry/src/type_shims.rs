//! Registry shims for wrapper types upstream computes lazily but never
//! inventory-registers (`Vec<T>`, `Option<T>` at pinned rev `745ee78`).
//!
//! Without these, the registry-level JSON legs
//! (`serialize_json_for_any`/`deserialize_json_for_type`) refuse every vec-
//! or option-typed property, so reflected values of those shapes could not
//! marshal through the #644 conversion layer at all. The shims reuse the
//! upstream trait impls (`<T as Reflectable>::serialize/deserialize` via
//! [`pulsar_reflection::JsonSerializer`]/[`pulsar_reflection::JsonDeserializer`])
//! -- no new serialization logic, just the missing registry entries.
//!
//! UPSTREAM ASK (same protocol as the object-model crate's Entity/u32
//! shims): register common wrapper instantiations in pulsar_reflection's
//! prims (or add a generic lazy fallback keyed off `Reflectable::type_info`
//! presence) so this file can shrink to nothing. Instantiations here are
//! the corpus scripting properties actually use; add more as components
//! demand them.

use pulsar_reflection::{
    JsonDeserializer, JsonSerializer, ReflectResult, Reflectable, RuntimeTypeRegistration,
};
use serde_json::Value;
use std::any::Any;

/// Serialize any `T: Reflectable` through its own trait impl.
fn serialize_via_trait<T: Reflectable + 'static>(value: &dyn Any) -> ReflectResult<Value> {
    let typed =
        value
            .downcast_ref::<T>()
            .ok_or_else(|| pulsar_reflection::ReflectError::TypeMismatch {
                expected: std::any::type_name::<T>(),
                found: format!("{:?}", value.type_id()),
            })?;
    let mut serializer = JsonSerializer::new();
    typed.serialize(&mut serializer)?;
    Ok(serializer.into_json())
}

/// Deserialize any `T: Reflectable` through its own trait impl.
fn deserialize_via_trait<T: Reflectable + 'static>(value: Value) -> ReflectResult<Box<dyn Any>> {
    let mut deserializer = JsonDeserializer::new(value);
    let parsed = T::deserialize(&mut deserializer)?;
    Ok(Box::new(parsed) as Box<dyn Any>)
}

macro_rules! register_wrapper_shim {
    ($ser:ident, $de:ident, $wrapper:ty) => {
        fn $ser(value: &dyn Any) -> ReflectResult<Value> {
            serialize_via_trait::<$wrapper>(value)
        }
        fn $de(value: Value) -> ReflectResult<Box<dyn Any>> {
            deserialize_via_trait::<$wrapper>(value)
        }
        inventory::submit! {
            RuntimeTypeRegistration {
                type_info: <$wrapper as Reflectable>::type_info,
                serialize_json: $ser,
                deserialize_json: $de,
            }
        }
    };
}

register_wrapper_shim!(ser_vec_f32, de_vec_f32, Vec<f32>);
register_wrapper_shim!(ser_vec_f64, de_vec_f64, Vec<f64>);
register_wrapper_shim!(ser_vec_i32, de_vec_i32, Vec<i32>);
register_wrapper_shim!(ser_vec_u64, de_vec_u64, Vec<u64>);
register_wrapper_shim!(ser_vec_string, de_vec_string, Vec<String>);
register_wrapper_shim!(ser_opt_bool, de_opt_bool, Option<bool>);
register_wrapper_shim!(ser_opt_i32, de_opt_i32, Option<i32>);
register_wrapper_shim!(ser_opt_f32, de_opt_f32, Option<f32>);
register_wrapper_shim!(ser_opt_string, de_opt_string, Option<String>);

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_reflection::{Reflectable, RUNTIME_TYPE_REGISTRY};

    /// The shimmed instantiations are real registry entries now: lookup by
    /// TypeId succeeds and the JSON round trip preserves values (#644's
    /// "strings, Vec<T>" compound requirement).
    #[test]
    fn shimmed_wrappers_round_trip_through_the_json_leg() {
        let vec_info = <Vec<f32> as Reflectable>::type_info();
        assert!(
            RUNTIME_TYPE_REGISTRY.has_type_id(vec_info.type_id),
            "shim must register the wrapper instantiation"
        );

        let value = vec![1.0f32, -2.5, 4.25];
        let json = RUNTIME_TYPE_REGISTRY
            .serialize_json_for_any(&value)
            .unwrap();
        assert_eq!(json, serde_json::json!([1.0, -2.5, 4.25]));
        let back = RUNTIME_TYPE_REGISTRY
            .deserialize_json_for_type(vec_info, json)
            .unwrap();
        assert_eq!(back.downcast_ref::<Vec<f32>>(), Some(&value));

        let strings = vec!["a".to_string(), "b".to_string()];
        let json = RUNTIME_TYPE_REGISTRY
            .serialize_json_for_any(&strings)
            .unwrap();
        assert_eq!(json, serde_json::json!(["a", "b"]));
        let back = RUNTIME_TYPE_REGISTRY
            .deserialize_json_for_type(<Vec<String> as Reflectable>::type_info(), json)
            .unwrap();
        assert_eq!(back.downcast_ref::<Vec<String>>(), Some(&strings));

        let maybe = Some(7i32);
        let json = RUNTIME_TYPE_REGISTRY
            .serialize_json_for_any(&maybe)
            .unwrap();
        let back = RUNTIME_TYPE_REGISTRY
            .deserialize_json_for_type(<Option<i32> as Reflectable>::type_info(), json)
            .unwrap();
        assert_eq!(back.downcast_ref::<Option<i32>>(), Some(&Some(7)));
    }

    /// A wrong concrete type behind the erased reference is refused by the
    /// shim's downcast, not misinterpreted.
    #[test]
    fn shim_serialization_refuses_type_mismatched_boxes() {
        assert!(serialize_via_trait::<Vec<f32>>(&"not a vec".to_string()).is_err());
    }
}
