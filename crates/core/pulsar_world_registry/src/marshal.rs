//! Unified value marshalling (#644): ONE conversion layer between the three
//! argument representations scripting backends deal in --
//! [`serde_json::Value`] (editor/metadata path), `Box<dyn Any>` (reflection
//! caller closures), and raw arena bytes (the VM `DispatchFn` ABI, encoded
//! per [`crate::vm_abi`]'s versioned spec).
//!
//! Every conversion is driven by the value's registered
//! [`RuntimeTypeInfo`] -- never by per-call-site ad-hoc matches -- so any
//! type reflection knows about marshals through every leg. The ONLY
//! hardcoded type lists are the fast paths: the closed Direct set (numeric
//! primitives + `bool` + packed `Entity`) that skips both JSON and the
//! registry entirely -- exactly what the issue's "no hardcoded primitive
//! lists BEYOND fast paths" clause allows. Everything else routes by
//! classification ([`crate::vm_abi::classify`]).
//!
//! All failures are [`ScriptRefError::Marshalling`] with a context naming
//! the leg and type; nothing here panics or truncates.
//!
//! ## Performance note (#D4)
//!
//! Hot-path property sets compose
//! `set_component_property_boxed` + [`any_to_bytes`] / [`bytes_to_any`]:
//! those legs never touch JSON for Direct/String/Vector kinds. JSON is the
//! editor/metadata representation only.

use std::any::{Any, TypeId};

use pulsar_reflection::{RuntimeTypeInfo, RUNTIME_TYPE_REGISTRY};
use pulsar_scenedb::Entity;
use serde_json::Value;

use crate::errors::ScriptRefError;
use crate::vm_abi::VmValueKind;

/// The closed Direct fast-path set: types whose entire representation fits
/// in 8 native bytes and whose every bit pattern is a valid value (`Entity`
/// rides its packed `bits()` u64, matching the FFI rule in the object
/// model's reflect module). Everything else is NOT direct.
#[allow(clippy::nonminimal_bool)] // explicit TypeId chain reads clearest here
pub(crate) fn is_direct_type(type_id: TypeId) -> bool {
    type_id == TypeId::of::<bool>()
        || type_id == TypeId::of::<u8>()
        || type_id == TypeId::of::<u16>()
        || type_id == TypeId::of::<u32>()
        || type_id == TypeId::of::<u64>()
        || type_id == TypeId::of::<i8>()
        || type_id == TypeId::of::<i16>()
        || type_id == TypeId::of::<i32>()
        || type_id == TypeId::of::<i64>()
        || type_id == TypeId::of::<f32>()
        || type_id == TypeId::of::<f64>()
        || type_id == TypeId::of::<Entity>()
}

// ── JSON ⇄ Any ─────────────────────────────────────────────────────────────

/// Serialize any registered reflected value to JSON via the runtime type
/// registry. `context` names the call site (e.g. `"LightComponent.color"`)
/// for the error message.
pub fn any_to_json(context: &str, value: &dyn Any) -> Result<Value, ScriptRefError> {
    RUNTIME_TYPE_REGISTRY.serialize_json_for_any(value).map_err(|e| {
        ScriptRefError::Marshalling { context: context.to_string(), message: e.to_string() }
    })
}

/// Deserialize JSON into a typed value against `type_info`'s registration.
///
/// Exactness invariant: the returned box holds EXACTLY the registered
/// concrete type (`type_info.type_id`) or this is an `Err` -- callers can
/// hand the result straight to setter closures/downcasts without a second
/// check.
pub fn json_to_any(
    context: &str,
    type_info: &'static RuntimeTypeInfo,
    value: Value,
) -> Result<Box<dyn Any>, ScriptRefError> {
    RUNTIME_TYPE_REGISTRY
        .deserialize_json_for_type(type_info, value)
        .map_err(|e| {
            ScriptRefError::Marshalling { context: context.to_string(), message: e.to_string() }
        })
}

// ── Any ⇄ arena bytes (encoding v1, see vm_abi) ────────────────────────────

fn marshal_error(
    leg: &str,
    type_info: &RuntimeTypeInfo,
    message: impl std::fmt::Display,
) -> ScriptRefError {
    ScriptRefError::Marshalling {
        context: format!("{leg} {}", type_info.type_name),
        message: message.to_string(),
    }
}

/// Encode `value` (whose concrete type `type_info` describes) as arena
/// bytes per encoding v1, appending to `out`. Zero-allocation for Direct
/// kinds; length-prefixed staging for String/Vector/JsonEncoded.
pub fn any_to_bytes(
    type_info: &'static RuntimeTypeInfo,
    value: &dyn Any,
    out: &mut Vec<u8>,
) -> Result<(), ScriptRefError> {
    let kind =
        crate::vm_abi::classify(type_info).map_err(|m| marshal_error("vm encode", type_info, m))?;
    match kind {
        VmValueKind::Direct => encode_direct(type_info, value, out),
        VmValueKind::Utf8String => {
            let s = downcast_ref::<String>(type_info, value)?;
            out.extend_from_slice(&(s.len() as u64).to_ne_bytes());
            out.extend_from_slice(s.as_bytes());
            Ok(())
        }
        VmValueKind::Vector => encode_vector(type_info, value, out),
        VmValueKind::JsonEncoded => {
            let json = RUNTIME_TYPE_REGISTRY
                .serialize_json_for_any(value)
                .map_err(|e| marshal_error("vm encode", type_info, e))?;
            let payload =
                serde_json::to_vec(&json).map_err(|e| marshal_error("vm encode", type_info, e))?;
            out.extend_from_slice(&(payload.len() as u64).to_ne_bytes());
            out.extend_from_slice(&payload);
            Ok(())
        }
    }
}

/// Decode one value of `type_info` from the START of `bytes` (the inverse
/// of [`any_to_bytes`]). Variable-length kinds are `[u64 length][payload]`;
/// callers can read the header to advance past them. Truncated or oversized
/// payloads are typed errors, never panics.
pub fn bytes_to_any(
    type_info: &'static RuntimeTypeInfo,
    bytes: &[u8],
) -> Result<Box<dyn Any>, ScriptRefError> {
    let kind =
        crate::vm_abi::classify(type_info).map_err(|m| marshal_error("vm decode", type_info, m))?;
    match kind {
        VmValueKind::Direct => decode_direct(type_info, bytes),
        VmValueKind::Utf8String => {
            let payload = take_payload(type_info, bytes)?;
            let s = std::str::from_utf8(payload)
                .map_err(|e| marshal_error("vm decode", type_info, e))?
                .to_string();
            Ok(Box::new(s))
        }
        VmValueKind::Vector => decode_vector(type_info, bytes),
        VmValueKind::JsonEncoded => {
            let payload = take_payload(type_info, bytes)?;
            let json: Value =
                serde_json::from_slice(payload).map_err(|e| marshal_error("vm decode", type_info, e))?;
            RUNTIME_TYPE_REGISTRY
                .deserialize_json_for_type(type_info, json)
                .map_err(|e| marshal_error("vm decode", type_info, e))
        }
    }
}

// ── Direct kind internals ──────────────────────────────────────────────────

fn need_bytes<'a>(
    type_info: &RuntimeTypeInfo,
    bytes: &'a [u8],
    n: usize,
) -> Result<&'a [u8], ScriptRefError> {
    bytes.get(..n).ok_or_else(|| {
        marshal_error("vm decode", type_info, format!("need {n} byte(s), got {}", bytes.len()))
    })
}

macro_rules! direct_arms {
    ($($t:ty),* $(,)?) => {
        fn encode_direct(
            type_info: &'static RuntimeTypeInfo,
            value: &dyn Any,
            out: &mut Vec<u8>,
        ) -> Result<(), ScriptRefError> {
            // Descriptor agreement first: a value whose concrete type
            // differs from its descriptor is refused, never reinterpreted.
            if value.type_id() != type_info.type_id {
                return Err(marshal_error(
                    "vm encode",
                    type_info,
                    "value's concrete type does not match its descriptor",
                ));
            }
            $(
                if let Some(v) = value.downcast_ref::<$t>() {
                    out.extend_from_slice(&v.to_ne_bytes());
                    return Ok(());
                }
            )*
            if let Some(b) = value.downcast_ref::<bool>() {
                out.push(*b as u8);
                return Ok(());
            }
            if let Some(e) = value.downcast_ref::<Entity>() {
                out.extend_from_slice(&e.bits().to_ne_bytes());
                return Ok(());
            }
            Err(marshal_error(
                "vm encode",
                type_info,
                "value's concrete type does not match its descriptor",
            ))
        }

        fn decode_direct(
            type_info: &'static RuntimeTypeInfo,
            bytes: &[u8],
        ) -> Result<Box<dyn Any>, ScriptRefError> {
            $(
                if type_info.type_id == TypeId::of::<$t>() {
                    let raw = need_bytes(type_info, bytes, std::mem::size_of::<$t>())?;
                    let mut arr = [0u8; std::mem::size_of::<$t>()];
                    arr.copy_from_slice(raw);
                    return Ok(Box::new(<$t>::from_ne_bytes(arr)));
                }
            )*
            if type_info.type_id == TypeId::of::<bool>() {
                // Decode through u8 -- an arbitrary byte must never be
                // reinterpreted AS a bool (UB outside 0/1); normalize.
                let raw = need_bytes(type_info, bytes, 1)?;
                return Ok(Box::new(raw[0] != 0));
            }
            if type_info.type_id == TypeId::of::<Entity>() {
                let raw = need_bytes(type_info, bytes, 8)?;
                let mut arr = [0u8; 8];
                arr.copy_from_slice(raw);
                return Ok(Box::new(Entity::from_bits(u64::from_ne_bytes(arr))));
            }
            Err(marshal_error(
                "vm decode",
                type_info,
                "descriptor type has no direct decoding",
            ))
        }
    };
}

direct_arms! { u8, u16, u32, u64, i8, i16, i32, i64, f32, f64 }

// ── compound kind internals ────────────────────────────────────────────────

fn downcast_ref<'a, T: 'static>(
    type_info: &RuntimeTypeInfo,
    value: &'a dyn Any,
) -> Result<&'a T, ScriptRefError> {
    value.downcast_ref::<T>().ok_or_else(|| {
        marshal_error("vm marshal", type_info, "value's concrete type does not match its descriptor")
    })
}

fn take_payload<'a>(
    type_info: &RuntimeTypeInfo,
    bytes: &'a [u8],
) -> Result<&'a [u8], ScriptRefError> {
    let header =
        bytes.first_chunk::<8>().ok_or_else(|| marshal_error("vm decode", type_info, "missing length header"))?;
    let len = u64::from_ne_bytes(*header) as usize;
    bytes.get(8..8 + len).ok_or_else(|| {
        marshal_error(
            "vm decode",
            type_info,
            format!(
                "length header says {len} byte(s), only {} available",
                bytes.len().saturating_sub(8)
            ),
        )
    })
}

/// Vectors encode element-wise over their DIRECT element type; the element
/// comes from the descriptor's wrapper inner info so no vector
/// instantiation needs naming here beyond the closed direct set.
fn encode_vector(
    type_info: &'static RuntimeTypeInfo,
    value: &dyn Any,
    out: &mut Vec<u8>,
) -> Result<(), ScriptRefError> {
    element_info(type_info)?;

    macro_rules! vec_arm {
        ($t:ty) => {
            if let Some(v) = value.downcast_ref::<Vec<$t>>() {
                out.reserve(v.len() * std::mem::size_of::<$t>());
                out.extend_from_slice(&(v.len() as u64).to_ne_bytes());
                for item in v {
                    out.extend_from_slice(&item.to_ne_bytes());
                }
                return Ok(());
            }
        };
    }
    vec_arm!(u8);
    vec_arm!(u16);
    vec_arm!(u32);
    vec_arm!(u64);
    vec_arm!(i8);
    vec_arm!(i16);
    vec_arm!(i32);
    vec_arm!(i64);
    vec_arm!(f32);
    vec_arm!(f64);

    macro_rules! bool_vec_arm {
        () => {
            if let Some(v) = value.downcast_ref::<Vec<bool>>() {
                out.reserve(v.len());
                out.extend_from_slice(&(v.len() as u64).to_ne_bytes());
                for item in v {
                    out.push(*item as u8);
                }
                return Ok(());
            }
        };
    }
    bool_vec_arm!();

    Err(marshal_error(
        "vm encode",
        type_info,
        "vector element is not in the direct fast-path set",
    ))
}

fn decode_vector(
    type_info: &'static RuntimeTypeInfo,
    bytes: &[u8],
) -> Result<Box<dyn Any>, ScriptRefError> {
    element_info(type_info)?;
    let header = bytes
        .first_chunk::<8>()
        .ok_or_else(|| marshal_error("vm decode", type_info, "missing vector count header"))?;
    let count = u64::from_ne_bytes(*header) as usize;

    macro_rules! vec_decode_arm {
        ($t:ty) => {
            if type_info.inner_type().is_some_and(|inner| inner.type_id == TypeId::of::<$t>()) {
                const ESIZE: usize = std::mem::size_of::<$t>();
                let need = count.checked_mul(ESIZE).ok_or_else(|| {
                    marshal_error("vm decode", type_info, "vector size overflows")
                })?;
                let payload = bytes.get(8..8 + need).ok_or_else(|| {
                    marshal_error(
                        "vm decode",
                        type_info,
                        format!(
                            "need {need} element byte(s), got {}",
                            bytes.len().saturating_sub(8)
                        ),
                    )
                })?;
                let mut items = Vec::with_capacity(count);
                for chunk in payload.chunks_exact(ESIZE) {
                    let mut arr = [0u8; ESIZE];
                    arr.copy_from_slice(chunk);
                    items.push(<$t>::from_ne_bytes(arr));
                }
                return Ok(Box::new(items));
            }
        };
    }
    vec_decode_arm!(u8);
    vec_decode_arm!(u16);
    vec_decode_arm!(u32);
    vec_decode_arm!(u64);
    vec_decode_arm!(i8);
    vec_decode_arm!(i16);
    vec_decode_arm!(i32);
    vec_decode_arm!(i64);
    vec_decode_arm!(f32);
    vec_decode_arm!(f64);

    if type_info.inner_type().is_some_and(|inner| inner.type_id == TypeId::of::<bool>()) {
        let payload = bytes.get(8..8 + count).ok_or_else(|| {
            marshal_error(
                "vm decode",
                type_info,
                format!("need {count} element byte(s), got {}", bytes.len().saturating_sub(8)),
            )
        })?;
        return Ok(Box::new(payload.iter().map(|b| *b != 0).collect::<Vec<bool>>()));
    }

    Err(marshal_error(
        "vm decode",
        type_info,
        "vector element is not in the direct fast-path set",
    ))
}

fn element_info(type_info: &RuntimeTypeInfo) -> Result<&'static RuntimeTypeInfo, ScriptRefError> {
    type_info.inner_type().ok_or_else(|| {
        marshal_error("vm marshal", type_info, "vector descriptor without an inner element type")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_reflection::{
        FieldInfo, ReflectResult, Reflectable, TypeDeserializer, TypeSerializer, TypeStructure,
    };

    fn info_of<T: 'static>() -> &'static RuntimeTypeInfo {
        RUNTIME_TYPE_REGISTRY
            .get::<T>()
            .unwrap_or_else(|| panic!("{} should be registered", std::any::type_name::<T>()))
    }

    /// Nested reflected struct for the JsonEncoded-fallback corpus entry,
    /// registered by hand below.
    ///
    /// UPSTREAM NOTE: `#[derive(Reflectable)]` at pinned rev `745ee78`
    /// generates `*value.downcast_ref::<FieldTy>()` per field -- a MOVE out
    /// of a shared reference, which fails to compile for any struct with a
    /// non-`Copy` field (`String`, `Vec`). Hand-written impl is the local
    /// shim; upstream should `.clone()` there instead.
    #[derive(Debug, Clone, PartialEq)]
    struct NestedSample {
        alpha: f32,
        label: String,
    }

    static NESTED_ALPHA_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
        type_id: std::any::TypeId::of::<f32>(),
        type_name: "f32",
        size: 4,
        align: 4,
        structure: TypeStructure::Primitive,
        color: None,
    };

    static NESTED_LABEL_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
        type_id: std::any::TypeId::of::<String>(),
        type_name: "String",
        size: std::mem::size_of::<String>(),
        align: std::mem::align_of::<String>(),
        structure: TypeStructure::String,
        color: None,
    };

    static NESTED_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
        type_id: std::any::TypeId::of::<NestedSample>(),
        type_name: "marshal_tests::NestedSample",
        size: std::mem::size_of::<NestedSample>(),
        align: std::mem::align_of::<NestedSample>(),
        structure: TypeStructure::Struct {
            fields: &[
                FieldInfo {
                    name: "alpha",
                    type_info: &NESTED_ALPHA_INFO,
                    offset: std::mem::offset_of!(NestedSample, alpha),
                },
                FieldInfo {
                    name: "label",
                    type_info: &NESTED_LABEL_INFO,
                    offset: std::mem::offset_of!(NestedSample, label),
                },
            ],
        },
        color: None,
    };

    impl Reflectable for NestedSample {
        fn type_info() -> &'static RuntimeTypeInfo {
            &NESTED_INFO
        }

        fn serialize(&self, serializer: &mut dyn TypeSerializer) -> ReflectResult<()> {
            serializer.serialize_struct(&[
                ("alpha", &self.alpha as &dyn std::any::Any),
                ("label", &self.label as &dyn std::any::Any),
            ])
        }

        fn deserialize(
            deserializer: &mut dyn TypeDeserializer,
        ) -> ReflectResult<Self> {
            let mut fields =
                deserializer.deserialize_struct(NESTED_INFO.fields().expect("struct info"))?;
            let alpha = *fields.remove("alpha").ok_or_else(|| {
                pulsar_reflection::ReflectError::MissingField {
                    struct_name: "NestedSample",
                    field_name: "alpha",
                }
            })?
            .downcast::<f32>()
            .map_err(|_| pulsar_reflection::ReflectError::TypeMismatch {
                expected: "f32",
                found: "other".into(),
            })?;
            let label = *fields.remove("label").ok_or_else(|| {
                pulsar_reflection::ReflectError::MissingField {
                    struct_name: "NestedSample",
                    field_name: "label",
                }
            })?
            .downcast::<String>()
            .map_err(|_| pulsar_reflection::ReflectError::TypeMismatch {
                expected: "String",
                found: "other".into(),
            })?;
            Ok(Self { alpha, label })
        }

        fn clone_any(&self) -> Box<dyn std::any::Any> {
            Box::new(self.clone())
        }
    }

    fn nested_serialize_json(value: &dyn std::any::Any) -> pulsar_reflection::ReflectResult<Value> {
        let typed = value.downcast_ref::<NestedSample>().ok_or_else(|| {
            pulsar_reflection::ReflectError::TypeMismatch {
                expected: "NestedSample",
                found: format!("{:?}", value.type_id()),
            }
        })?;
        let mut serializer = pulsar_reflection::JsonSerializer::new();
        typed.serialize(&mut serializer)?;
        Ok(serializer.into_json())
    }

    fn nested_deserialize_json(
        value: Value,
    ) -> pulsar_reflection::ReflectResult<Box<dyn std::any::Any>> {
        let mut deserializer = pulsar_reflection::JsonDeserializer::new(value);
        Ok(Box::new(<NestedSample as Reflectable>::deserialize(
            &mut deserializer,
        )?) as Box<dyn std::any::Any>)
    }

    pulsar_reflection::inventory::submit! {
        pulsar_reflection::RuntimeTypeRegistration {
            type_info: <NestedSample as Reflectable>::type_info,
            serialize_json: nested_serialize_json,
            deserialize_json: nested_deserialize_json,
        }
    }

    /// Entity's registry shim lives in the object-model crate, not this
    /// binary; classification and the byte legs need only the TypeId, so
    /// tests build its descriptor locally.
    static ENTITY_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
        type_id: std::any::TypeId::of::<Entity>(),
        type_name: "pulsar_scenedb::Entity",
        size: std::mem::size_of::<Entity>(),
        align: std::mem::align_of::<Entity>(),
        structure: TypeStructure::Primitive,
        color: None,
    };

    /// #644 acceptance: every REGISTERED corpus value survives
    /// ANY→JSON→ANY, ANY→BYTES→ANY, and the composed JSON→ANY→BYTES→ANY
    /// trip with its value preserved.
    #[test]
    fn corpus_round_trips_through_every_pair_of_representations() {
        let nested = NestedSample { alpha: 0.5, label: "nested".into() };
        let corpus: Vec<(&'static RuntimeTypeInfo, Box<dyn Any>)> = vec![
            (info_of::<f32>(), Box::new(1.5f32)),
            (info_of::<f64>(), Box::new(-2.25f64)),
            (info_of::<i32>(), Box::new(-7i32)),
            (info_of::<u64>(), Box::new(77u64)),
            (info_of::<i64>(), Box::new(-77i64)),
            (info_of::<bool>(), Box::new(true)),
            (info_of::<String>(), Box::new("héllo wörld".to_string())),
            (<Vec<f32> as Reflectable>::type_info(), Box::new(vec![1.0f32, -2.5, 4.25])),
            (<Vec<i32> as Reflectable>::type_info(), Box::new(vec![3i32, -4, 5])),
            (
                <NestedSample as Reflectable>::type_info(),
                Box::new(nested.clone()),
            ),
        ];

        for (type_info, value) in &corpus {
            // ANY -> JSON -> ANY
            let json = any_to_json(type_info.type_name, &**value).expect("any→json");
            let from_json = json_to_any(type_info.type_name, type_info, json).expect("json→any");
            assert_same_value(type_info, &from_json, &**value);

            // ANY -> BYTES -> ANY
            let mut bytes = Vec::new();
            any_to_bytes(type_info, &**value, &mut bytes).expect("any→bytes");
            let from_bytes = bytes_to_any(type_info, &bytes).expect("bytes→any");
            assert_same_value(type_info, &from_bytes, &**value);

            // Composed: JSON -> ANY -> BYTES -> ANY
            let reboxed = json_to_any(
                type_info.type_name,
                type_info,
                any_to_json(type_info.type_name, &**value).unwrap(),
            )
            .expect("compose");
            let mut staged = Vec::new();
            any_to_bytes(type_info, &*reboxed, &mut staged).expect("compose");
            let final_value = bytes_to_any(type_info, &staged).expect("compose");
            assert_same_value(type_info, &final_value, &**value);
        }
    }

    /// Packed `Entity` rides the Direct byte legs (its JSON leg is owned by
    /// the object-model crate's shim and tested there). `u32` joins it via
    /// a local descriptor: upstream has neither trait impl nor registry
    /// entry for it, yet it appears as a Direct vec element type.
    #[test]
    fn direct_kinds_without_local_registrations_round_trip_bytes() {
        let entity = Entity::from_bits((6u64 << 32) | 9);
        let mut bytes = Vec::new();
        any_to_bytes(&ENTITY_INFO, &entity, &mut bytes).unwrap();
        assert_eq!(bytes.len(), 8, "direct encoding is exactly the packed bits");
        let back = bytes_to_any(&ENTITY_INFO, &bytes).unwrap();
        assert_eq!(back.downcast_ref::<Entity>().map(|e| e.bits()), Some(entity.bits()));

        assert_eq!(
            crate::vm_abi::classify(&ENTITY_INFO).unwrap(),
            crate::vm_abi::VmValueKind::Direct
        );

        static U32_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
            type_id: std::any::TypeId::of::<u32>(),
            type_name: "u32",
            size: 4,
            align: 4,
            structure: TypeStructure::Primitive,
            color: None,
        };
        let mut out = Vec::new();
        any_to_bytes(&U32_INFO, &9u32, &mut out).unwrap();
        assert_eq!(out.len(), 4);
        assert_eq!(bytes_to_any(&U32_INFO, &out).unwrap().downcast_ref::<u32>(), Some(&9));

        // Descriptor disagreement is refused for every direct kind.
        assert!(any_to_bytes(&U32_INFO, &9i64, &mut Vec::new()).is_err());
    }

    /// The nested struct classifies to the JSON fallback kind -- proving a
    /// registered non-primitive compound marshals without special casing.
    #[test]
    fn nested_registered_struct_rides_the_json_fallback_kind() {
        let info = <NestedSample as Reflectable>::type_info();
        assert_eq!(
            crate::vm_abi::classify(info).unwrap(),
            crate::vm_abi::VmValueKind::JsonEncoded
        );
    }

    /// Truncated/garbled byte payloads are typed errors, never panics --
    /// the decoder trusts nothing about the wire.
    #[test]
    fn truncated_byte_payloads_are_typed_errors() {
        let string_info = info_of::<String>();
        assert!(bytes_to_any(string_info, &[]).is_err());
        assert!(bytes_to_any(string_info, &[0, 0]).is_err());

        let mut full = Vec::new();
        any_to_bytes(string_info, &"abc".to_string(), &mut full).unwrap();
        assert!(full.len() > 4);
        assert!(
            bytes_to_any(string_info, &full[..full.len() - 1]).is_err(),
            "short payload refused"
        );
    }

    /// A concrete value whose type disagrees with its descriptor is
    /// refused -- marshalling never guesses.
    #[test]
    fn type_mismatched_values_are_refused() {
        let err = any_to_bytes(info_of::<f32>(), &7i32, &mut Vec::new()).unwrap_err();
        assert!(matches!(err, ScriptRefError::Marshalling { .. }));
    }

    /// Compare two erased boxes known to be of `type_info`'s type.
    fn assert_same_value(type_info: &RuntimeTypeInfo, got: &Box<dyn Any>, want: &(dyn Any)) {
        macro_rules! cmp {
            ($t:ty) => {
                if type_info.type_id == TypeId::of::<$t>() {
                    assert_eq!(
                        got.downcast_ref::<$t>(),
                        want.downcast_ref::<$t>(),
                        "{} round-trip changed the value",
                        type_info.type_name
                    );
                    return;
                }
            };
        }
        cmp!(f32); cmp!(f64); cmp!(i8); cmp!(i16); cmp!(i32); cmp!(i64);
        cmp!(u8); cmp!(u16); cmp!(u32); cmp!(u64); cmp!(bool);

        if type_info.type_id == TypeId::of::<String>() {
            assert_eq!(got.downcast_ref::<String>(), want.downcast_ref::<String>());
            return;
        }

        macro_rules! cmp_vec {
            ($t:ty) => {
                if type_info.inner_type().is_some_and(|i| i.type_id == TypeId::of::<$t>()) {
                    assert_eq!(
                        got.downcast_ref::<Vec<$t>>(),
                        want.downcast_ref::<Vec<$t>>()
                    );
                    return;
                }
            };
        }
        cmp_vec!(f32);
        cmp_vec!(i32);

        if type_info.is_struct() {
            assert_eq!(
                got.downcast_ref::<NestedSample>(),
                want.downcast_ref::<NestedSample>()
            );
            return;
        }
        panic!("no comparison arm for {}", type_info.type_name);
    }
}
