//! Standard wrapper registrations and Reflectable implementations.

use crate::runtime_types::{RuntimeTypeInfo, TypeStructure, WrapperType};
use crate::type_traits::{ReflectError, ReflectResult};
use crate::{FieldInfo, Reflectable, TypeDeserializer, TypeSerializer};
use once_cell::sync::Lazy;
use std::any::{Any, TypeId};

#[derive(Default)]
struct StdWrapperTypeInfo {
    entries: dashmap::DashMap<TypeId, &'static RuntimeTypeInfo>,
}

static STD_WRAPPER_TYPE_INFO: Lazy<StdWrapperTypeInfo> =
    Lazy::new(StdWrapperTypeInfo::default);

fn get_or_insert_wrapper_type_info<T: Reflectable + Clone + Send + Sync + 'static>(
    wrapper_kind: WrapperType,
) -> &'static RuntimeTypeInfo {
    let type_id = TypeId::of::<T>();
    if let Some(entry) = STD_WRAPPER_TYPE_INFO.entries.get(&type_id) {
        return *entry;
    }

    let info = Box::leak(Box::new(RuntimeTypeInfo {
        type_id,
        type_name: std::any::type_name::<T>(),
        size: std::mem::size_of::<T>(),
        align: std::mem::align_of::<T>(),
        structure: TypeStructure::Wrapper {
            wrapper_kind,
            inner: T::type_info(),
        },
    }));

    STD_WRAPPER_TYPE_INFO.entries.insert(type_id, info);
    info
}

impl<T> Reflectable for Vec<T>
where
    T: Reflectable + Clone + Send + Sync + 'static,
{
    fn type_info() -> &'static RuntimeTypeInfo
    where
        Self: Sized,
    {
        get_or_insert_wrapper_type_info::<Self>(WrapperType::Vec)
    }

    fn serialize(&self, serializer: &mut dyn TypeSerializer) -> ReflectResult<()> {
        let values: Vec<&dyn Any> = self.iter().map(|value| value as &dyn Any).collect();
        serializer.serialize_array(&values, T::type_info())
    }

    fn deserialize(deserializer: &mut dyn TypeDeserializer) -> ReflectResult<Self>
    where
        Self: Sized,
    {
        let values = deserializer.deserialize_array(T::type_info())?;
        values
            .into_iter()
            .map(|value| {
                value
                    .downcast::<T>()
                    .map(|v| *v)
                    .map_err(|boxed| ReflectError::TypeMismatch {
                        expected: std::any::type_name::<T>(),
                        found: format!("{:?}", (&*boxed).type_id()),
                    })
            })
            .collect()
    }

    fn clone_any(&self) -> Box<dyn Any> {
        Box::new(self.clone())
    }
}

impl<T> Reflectable for Option<T>
where
    T: Reflectable + Clone + Send + Sync + 'static,
{
    fn type_info() -> &'static RuntimeTypeInfo
    where
        Self: Sized,
    {
        get_or_insert_wrapper_type_info::<Self>(WrapperType::Option)
    }

    fn serialize(&self, serializer: &mut dyn TypeSerializer) -> ReflectResult<()> {
        match self {
            Some(value) => {
                let has_value = true;
                let fields: [(&str, &dyn Any); 2] = [
                    ("has_value", &has_value as &dyn Any),
                    ("value", value as &dyn Any),
                ];
                serializer.serialize_struct(&fields)
            }
            None => {
                let has_value = false;
                let fields: [(&str, &dyn Any); 1] = [("has_value", &has_value as &dyn Any)];
                serializer.serialize_struct(&fields)
            }
        }
    }

    fn deserialize(deserializer: &mut dyn TypeDeserializer) -> ReflectResult<Self>
    where
        Self: Sized,
    {
        let has_value_fields = [FieldInfo::new("has_value", <bool as Reflectable>::type_info(), 0)];
        let has_value_map = deserializer.deserialize_struct(&has_value_fields)?;

        let has_value = has_value_map
            .get("has_value")
            .and_then(|value| value.downcast_ref::<bool>())
            .copied()
            .ok_or_else(|| ReflectError::TypeMismatch {
                expected: "bool",
                found: "missing or invalid has_value".to_string(),
            })?;

        if !has_value {
            return Ok(None);
        }

        let value_fields = [FieldInfo::new("value", T::type_info(), 0)];
        let value_map = deserializer.deserialize_struct(&value_fields)?;

        let value = value_map
            .into_iter()
            .next()
            .ok_or_else(|| ReflectError::MissingField {
                struct_name: "Option",
                field_name: "value",
            })?
            .1
            .downcast::<T>()
            .map(|v| *v)
            .map_err(|boxed| ReflectError::TypeMismatch {
                expected: std::any::type_name::<T>(),
                found: format!("{:?}", (&*boxed).type_id()),
            })?;

        Ok(Some(value))
    }

    fn clone_any(&self) -> Box<dyn Any> {
        Box::new(self.clone())
    }
}
