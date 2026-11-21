//! [`ScriptValue`]: the Rust types typed natives can take and return, with the
//! impls for the builtin types, collections and [`Obj`].

use std::any::TypeId;
use std::sync::Arc;

use pulsar_scenedb::Entity;

use crate::value::{MapKey, Object, Value};

use super::*;

/// A Rust type with a builtin script representation. Implemented for
/// `()`, `bool`, every integer type (as `int`, range-checked), `f32`/`f64`
/// (as `float`), `String`, `Arc<str>` and [`Entity`]. Typed natives take
/// and return these (plus [`Obj`] for value types).
pub trait ScriptValue: Clone + Sized + 'static {
    fn script_type() -> Type;
    fn from_value(value: &Value) -> Option<Self>;
    fn into_value(self) -> Value;
}

impl ScriptValue for () {
    fn script_type() -> Type {
        Type::Unit
    }
    fn from_value(value: &Value) -> Option<Self> {
        matches!(value, Value::Unit).then_some(())
    }
    fn into_value(self) -> Value {
        Value::Unit
    }
}

impl ScriptValue for bool {
    fn script_type() -> Type {
        Type::Bool
    }
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    fn into_value(self) -> Value {
        Value::Bool(self)
    }
}

macro_rules! int_value {
    ($($ty:ty),*) => {$(
        impl ScriptValue for $ty {
            fn script_type() -> Type {
                Type::Int
            }
            fn from_value(value: &Value) -> Option<Self> {
                match value {
                    Value::Int(i) => <$ty>::try_from(*i).ok(),
                    _ => None,
                }
            }
            fn into_value(self) -> Value {
                // Saturate values beyond i64 (only u64/usize/u128-sized can be).
                Value::Int(i64::try_from(self).unwrap_or(i64::MAX))
            }
        }
    )*};
}
int_value!(i8, i16, i32, i64, isize, i128, u8, u16, u32, u64, usize, u128);

impl ScriptValue for f64 {
    fn script_type() -> Type {
        Type::Float
    }
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }
    fn into_value(self) -> Value {
        Value::Float(self)
    }
}

impl ScriptValue for f32 {
    fn script_type() -> Type {
        Type::Float
    }
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Float(f) => Some(*f as f32),
            _ => None,
        }
    }
    fn into_value(self) -> Value {
        Value::Float(self as f64)
    }
}

impl ScriptValue for Arc<str> {
    fn script_type() -> Type {
        Type::Str
    }
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Str(s) => Some(s.clone()),
            _ => None,
        }
    }
    fn into_value(self) -> Value {
        Value::Str(self)
    }
}

impl ScriptValue for String {
    fn script_type() -> Type {
        Type::Str
    }
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Str(s) => Some(s.to_string()),
            _ => None,
        }
    }
    fn into_value(self) -> Value {
        Value::Str(self.into())
    }
}

impl ScriptValue for Entity {
    fn script_type() -> Type {
        Type::Entity
    }
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Entity(e) => Some(*e),
            _ => None,
        }
    }
    fn into_value(self) -> Value {
        Value::Entity(self)
    }
}

impl<T: ScriptValue> ScriptValue for Vec<T> {
    fn script_type() -> Type {
        Type::list(T::script_type())
    }
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::List(items) => items.iter().map(T::from_value).collect(),
            _ => None,
        }
    }
    fn into_value(self) -> Value {
        Value::List(Arc::new(self.into_iter().map(T::into_value).collect()))
    }
}

/// A fixed-size array is a list whose length the native checks.
impl<T: ScriptValue, const N: usize> ScriptValue for [T; N] {
    fn script_type() -> Type {
        Type::list(T::script_type())
    }
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::List(items) if items.len() == N => items
                .iter()
                .map(T::from_value)
                .collect::<Option<Vec<_>>>()?
                .try_into()
                .ok(),
            _ => None,
        }
    }
    fn into_value(self) -> Value {
        Value::List(Arc::new(self.into_iter().map(T::into_value).collect()))
    }
}

macro_rules! tuple_value {
    ($(($($name:ident $index:tt),+))*) => {$(
        impl<$($name: ScriptValue),+> ScriptValue for ($($name,)+) {
            fn script_type() -> Type {
                Type::Tuple(vec![$($name::script_type()),+])
            }
            fn from_value(value: &Value) -> Option<Self> {
                match value {
                    Value::Tuple(items) => Some(($($name::from_value(items.get($index)?)?,)+)),
                    _ => None,
                }
            }
            fn into_value(self) -> Value {
                Value::Tuple(vec![$(self.$index.into_value()),+].into())
            }
        }
    )*};
}
tuple_value! {
    (A 0, B 1)
    (A 0, B 1, C 2)
    (A 0, B 1, C 2, D 3)
    (A 0, B 1, C 2, D 3, E 4)
    (A 0, B 1, C 2, D 3, E 4, F 5)
}

/// `Option<T>` is `(present: bool, value: T)`; the value is `T`'s default
/// when absent.
impl<T: ScriptValue + Default> ScriptValue for Option<T> {
    fn script_type() -> Type {
        Type::Tuple(vec![Type::Bool, T::script_type()])
    }
    fn from_value(value: &Value) -> Option<Self> {
        match <(bool, T)>::from_value(value)? {
            (true, item) => Some(Some(item)),
            (false, _) => Some(None),
        }
    }
    fn into_value(self) -> Value {
        match self {
            Some(item) => (true, item),
            None => (false, T::default()),
        }
        .into_value()
    }
}

/// A fallible native result kept as a value: `(ok: bool, value: T, error:
/// string)`. `value` is `T`'s default on an error and `error` is empty on
/// success. (A native that should fail the script call returns a plain
/// `Result` instead.)
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome<T>(pub Result<T, String>);

impl<T: ScriptValue + Default> ScriptValue for Outcome<T> {
    fn script_type() -> Type {
        Type::Tuple(vec![Type::Bool, T::script_type(), Type::Str])
    }
    fn from_value(value: &Value) -> Option<Self> {
        match <(bool, T, String)>::from_value(value)? {
            (true, item, _) => Some(Self(Ok(item))),
            (false, _, message) => Some(Self(Err(message))),
        }
    }
    fn into_value(self) -> Value {
        match self.0 {
            Ok(item) => (true, item, String::new()),
            Err(message) => (false, T::default(), message),
        }
        .into_value()
    }
}

/// A Rust type usable as a script map key: `bool`, the integer types and
/// strings.
pub trait ScriptKey: ScriptValue + Ord {}
impl ScriptKey for bool {}
impl ScriptKey for String {}
impl ScriptKey for Arc<str> {}
macro_rules! int_key {
    ($($ty:ty),*) => {$(impl ScriptKey for $ty {})*};
}
int_key!(i8, i16, i32, i64, isize, i128, u8, u16, u32, u64, usize, u128);

macro_rules! map_value {
    ($map:ident $(, $bound:path)*) => {
        impl<K: ScriptKey $(+ $bound)*, V: ScriptValue> ScriptValue for std::collections::$map<K, V> {
            fn script_type() -> Type {
                Type::map(K::script_type(), V::script_type())
            }
            fn from_value(value: &Value) -> Option<Self> {
                match value {
                    Value::Map(entries) => entries
                        .iter()
                        .map(|(key, value)| Some((K::from_value(&key.to_value())?, V::from_value(value)?)))
                        .collect(),
                    _ => None,
                }
            }
            fn into_value(self) -> Value {
                Value::Map(Arc::new(
                    self.into_iter()
                        .filter_map(|(key, value)| Some((MapKey::from_value(&key.into_value())?, value.into_value())))
                        .collect(),
                ))
            }
        }
    };
}
map_value!(BTreeMap);
map_value!(HashMap, std::hash::Hash);

macro_rules! set_value {
    ($set:ident $(, $bound:path)*) => {
        /// A set is a list of its members: unique, in order.
        impl<K: ScriptKey $(+ $bound)*> ScriptValue for std::collections::$set<K> {
            fn script_type() -> Type {
                Type::list(K::script_type())
            }
            fn from_value(value: &Value) -> Option<Self> {
                match value {
                    Value::List(items) => items.iter().map(K::from_value).collect(),
                    _ => None,
                }
            }
            fn into_value(self) -> Value {
                let mut keys: Vec<MapKey> = self.into_iter().filter_map(|key| MapKey::from_value(&key.into_value())).collect();
                keys.sort();
                Value::list(keys.iter().map(MapKey::to_value).collect())
            }
        }
    };
}
set_value!(BTreeSet);
set_value!(HashSet, std::hash::Hash);

/// A value of a type registered with [`script_value_type!`](crate::script_value_type),
/// for typed natives: `|v: Obj<Vec3>| v.0.length()`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Obj<T>(pub T);

impl<T: Clone + Send + Sync + 'static> ScriptValue for Obj<T> {
    fn script_type() -> Type {
        match TypeRegistry::global().binding(TypeId::of::<T>()) {
            Some(binding) => binding.script_type(),
            None => Type::Object(std::any::type_name::<T>().to_owned()),
        }
    }
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Object(obj) => obj.downcast_ref::<T>().cloned().map(Obj),
            _ => None,
        }
    }
    fn into_value(self) -> Value {
        match TypeRegistry::global()
            .objects_by_rust
            .get(&TypeId::of::<T>())
        {
            Some(name) => Value::Object(Object::new(name, self.0)),
            None => Value::Object(Object::new(std::any::type_name::<T>(), self.0)),
        }
    }
}
