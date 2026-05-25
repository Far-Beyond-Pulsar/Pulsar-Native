//! glam::Mat4 registration.

use crate::runtime_registry::RuntimeTypeRegistration;
use crate::runtime_types::{RuntimeTypeInfo, TypeStructure};
use crate::{ReflectError, ReflectResult, Reflectable, TypeDeserializer, TypeSerializer};
use glam::Mat4;
use std::any::Any;

static MAT4_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: std::any::TypeId::of::<Mat4>(),
    type_name: "glam::Mat4",
    size: std::mem::size_of::<Mat4>(),
    align: std::mem::align_of::<Mat4>(),
    structure: TypeStructure::Primitive,
};

impl Reflectable for Mat4 {
    fn type_info() -> &'static RuntimeTypeInfo
    where
        Self: Sized,
    {
        &MAT4_TYPE_INFO
    }

    fn serialize(&self, serializer: &mut dyn TypeSerializer) -> ReflectResult<()> {
        let cols = self.to_cols_array();
        let values: Vec<&dyn Any> = cols.iter().map(|v| v as &dyn Any).collect();
        serializer.serialize_array(&values, <f32 as Reflectable>::type_info())
    }

    fn deserialize(deserializer: &mut dyn TypeDeserializer) -> ReflectResult<Self>
    where
        Self: Sized,
    {
        let values = deserializer.deserialize_array(<f32 as Reflectable>::type_info())?;
        if values.len() != 16 {
            return Err(ReflectError::TypeMismatch {
                expected: "[f32; 16]",
                found: format!("array length {}", values.len()),
            });
        }

        let mut cols = [0.0f32; 16];
        for (idx, value) in values.into_iter().enumerate() {
            cols[idx] = value
                .downcast::<f32>()
                .map(|v| *v)
                .map_err(|boxed| ReflectError::TypeMismatch {
                    expected: "f32",
                    found: format!("{:?}", (&*boxed).type_id()),
                })?;
        }

        Ok(Mat4::from_cols_array(&cols))
    }

    fn clone_any(&self) -> Box<dyn Any> {
        Box::new(*self)
    }
}

fn serialize_mat4_json(value: &dyn Any) -> ReflectResult<serde_json::Value> {
    let mat = value
        .downcast_ref::<Mat4>()
        .ok_or_else(|| ReflectError::TypeMismatch {
            expected: "glam::Mat4",
            found: format!("{:?}", value.type_id()),
        })?;

    Ok(serde_json::json!(mat.to_cols_array()))
}

fn deserialize_mat4_json(value: serde_json::Value) -> ReflectResult<Box<dyn Any>> {
    let arr = value.as_array().ok_or_else(|| ReflectError::TypeMismatch {
        expected: "[f32; 16]",
        found: format!("{:?}", value),
    })?;

    if arr.len() != 16 {
        return Err(ReflectError::TypeMismatch {
            expected: "[f32; 16]",
            found: format!("array length {}", arr.len()),
        });
    }

    let mut cols = [0.0f32; 16];
    for (idx, item) in arr.iter().enumerate() {
        cols[idx] = item.as_f64().unwrap_or(0.0) as f32;
    }

    Ok(Box::new(Mat4::from_cols_array(&cols)) as Box<dyn Any>)
}

crate::inventory::submit! {
    RuntimeTypeRegistration {
        type_info: &MAT4_TYPE_INFO,
        serialize_json: serialize_mat4_json,
        deserialize_json: deserialize_mat4_json,
    }
}
