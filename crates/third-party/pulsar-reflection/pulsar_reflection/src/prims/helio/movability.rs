//! helio::Movability registration.

use crate::runtime_registry::RuntimeTypeRegistration;
use crate::runtime_types::{RuntimeTypeInfo, TypeStructure};
use crate::{ReflectError, ReflectResult, Reflectable, TypeDeserializer, TypeSerializer};
use helio::Movability;
use std::any::Any;

static MOVABILITY_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: std::any::TypeId::of::<Movability>(),
    type_name: "helio::Movability",
    size: std::mem::size_of::<Movability>(),
    align: std::mem::align_of::<Movability>(),
    structure: TypeStructure::Enum {
        variants: &["Static", "Stationary", "Movable"],
    },
    color: None,
};

impl Reflectable for Movability {
    fn type_info() -> &'static RuntimeTypeInfo
    where
        Self: Sized,
    {
        &MOVABILITY_TYPE_INFO
    }

    fn serialize(&self, serializer: &mut dyn TypeSerializer) -> ReflectResult<()> {
        let (name, index) = match self {
            Movability::Static => ("Static", 0),
            Movability::Stationary => ("Stationary", 1),
            Movability::Movable => ("Movable", 2),
        };
        serializer.serialize_enum(name, index)
    }

    fn deserialize(deserializer: &mut dyn TypeDeserializer) -> ReflectResult<Self>
    where
        Self: Sized,
    {
        let idx = deserializer.deserialize_enum(&["Static", "Stationary", "Movable"])?;
        match idx {
            0 => Ok(Movability::Static),
            1 => Ok(Movability::Stationary),
            2 => Ok(Movability::Movable),
            _ => Err(ReflectError::InvalidVariant {
                enum_name: "helio::Movability",
                variant: format!("index {}", idx),
            }),
        }
    }

    fn clone_any(&self) -> Box<dyn Any> {
        Box::new(*self)
    }
}

fn serialize_movability_json(value: &dyn Any) -> ReflectResult<serde_json::Value> {
    let movability =
        value
            .downcast_ref::<Movability>()
            .ok_or_else(|| ReflectError::TypeMismatch {
                expected: "helio::Movability",
                found: format!("{:?}", value.type_id()),
            })?;

    let as_str = match movability {
        Movability::Static => "Static",
        Movability::Stationary => "Stationary",
        Movability::Movable => "Movable",
    };

    Ok(serde_json::json!(as_str))
}

fn deserialize_movability_json(value: serde_json::Value) -> ReflectResult<Box<dyn Any>> {
    if let Some(name) = value.as_str() {
        let movability = match name {
            "Static" => Movability::Static,
            "Stationary" => Movability::Stationary,
            "Movable" => Movability::Movable,
            _ => {
                return Err(ReflectError::InvalidVariant {
                    enum_name: "helio::Movability",
                    variant: name.to_string(),
                });
            }
        };
        return Ok(Box::new(movability));
    }

    if let Some(index) = value.as_u64() {
        let movability = match index {
            0 => Movability::Static,
            1 => Movability::Stationary,
            2 => Movability::Movable,
            _ => {
                return Err(ReflectError::InvalidVariant {
                    enum_name: "helio::Movability",
                    variant: format!("index {}", index),
                });
            }
        };
        return Ok(Box::new(movability));
    }

    Err(ReflectError::TypeMismatch {
        expected: "helio::Movability",
        found: format!("{:?}", value),
    })
}

crate::inventory::submit! {
    RuntimeTypeRegistration {
        type_info: &MOVABILITY_TYPE_INFO,
        serialize_json: serialize_movability_json,
        deserialize_json: deserialize_movability_json,
    }
}
