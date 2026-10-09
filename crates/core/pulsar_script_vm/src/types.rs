//! Script types and their bindings to Rust types.
//!
//! [`Type`] is what modules, signatures and registers are written in. It is
//! serializable and names every non-builtin type by a **stable script
//! name** (never a `TypeId` or `ComponentId`, which are process-local).
//!
//! The [`TypeRegistry`] binds those names to Rust types, for converting
//! between [`Value`]s and the `Box<dyn Any>` arguments of reflected methods:
//!
//! - builtins (`bool`, integers, floats, `String`, [`Entity`]) are always
//!   bound;
//! - components are bound with [`script_component!`](crate::script_component);
//! - plain value types (vectors, colors, ..) with
//!   [`script_value_type!`](crate::script_value_type).
//!
//! Bindings come only from the engine binary (link-time `inventory`), never
//! from hot-reloadable native libraries: a live [`Value::Object`] carries a
//! clone function, which must not point into code that can be unloaded.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, LazyLock};

use pulsar_reflection::methods::TypeRef;
use pulsar_scenedb::{ComponentId, ComponentRef, Entity, World};
use serde::{Deserialize, Serialize};

use crate::value::{MapKey, Object, Value};

/// The type of a register, variable, parameter or return value.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Encode, bincode::Decode,
)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum Type {
    Unit,
    Bool,
    Int,
    Float,
    Str,
    Entity,
    /// Reference to a component of the named type on some entity.
    Component(String),
    /// A value of a registered value type (e.g. `Vec3`).
    Object(String),
    /// A growable list. Values have value semantics: copying one never
    /// aliases (the storage is shared copy-on-write).
    List(Box<Type>),
    /// A map with a `bool`, `int` or `string` key, ordered by key.
    Map(Box<Type>, Box<Type>),
    /// A fixed group of values of possibly different types; what a native
    /// with several results returns.
    Tuple(Vec<Type>),
}

impl Type {
    pub fn list(element: Type) -> Self {
        Self::List(Box::new(element))
    }

    pub fn map(key: Type, value: Type) -> Self {
        Self::Map(Box::new(key), Box::new(value))
    }

    /// Whether values of this type can be map keys.
    pub fn is_key(&self) -> bool {
        matches!(self, Self::Bool | Self::Int | Self::Str)
    }

    /// Whether a value of this type holds a registered value type anywhere
    /// (such values neither compare nor print).
    pub fn contains_object(&self) -> bool {
        match self {
            Self::Object(_) => true,
            Self::List(element) => element.contains_object(),
            Self::Map(key, value) => key.contains_object() || value.contains_object(),
            Self::Tuple(items) => items.iter().any(Self::contains_object),
            _ => false,
        }
    }

    /// Structural validity: a map's key type must be [`is_key`](Self::is_key).
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::List(element) => element.validate(),
            Self::Map(key, value) => {
                if !key.is_key() {
                    return Err(format!("{self}: a map key must be bool, int or string"));
                }
                value.validate()
            }
            Self::Tuple(items) => items.iter().try_for_each(Self::validate),
            _ => Ok(()),
        }
    }

    pub fn component(name: impl Into<String>) -> Self {
        Self::Component(name.into())
    }

    pub fn object(name: impl Into<String>) -> Self {
        Self::Object(name.into())
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Self::Int | Self::Float)
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unit => f.write_str("unit"),
            Self::Bool => f.write_str("bool"),
            Self::Int => f.write_str("int"),
            Self::Float => f.write_str("float"),
            Self::Str => f.write_str("string"),
            Self::Entity => f.write_str("entity"),
            Self::Component(name) => write!(f, "{name}&"),
            Self::Object(name) => f.write_str(name),
            Self::List(element) => write!(f, "list<{element}>"),
            Self::Map(key, value) => write!(f, "map<{key}, {value}>"),
            Self::Tuple(items) => {
                f.write_str("(")?;
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{item}")?;
                }
                f.write_str(")")
            }
        }
    }
}

/// Conversion between a Rust type and script [`Value`]s.
#[derive(Clone, Copy)]
pub struct TypeBinding {
    pub rust: TypeRef,
    /// Script type, as a function so object bindings can name themselves.
    script: fn() -> Type,
    /// Clone a Rust value out as a script value.
    to_value: fn(&dyn Any) -> Value,
    /// Build the Rust value a native argument slot needs.
    from_value: fn(&Value) -> Result<Box<dyn Any>, String>,
    /// Read a `T` at a pointer (a reflected field).
    load: unsafe fn(*const u8) -> Value,
    /// Assign a `T` at a pointer (a reflected field), dropping the old one.
    store: unsafe fn(*mut u8, &Value) -> Result<(), String>,
}

impl TypeBinding {
    pub fn script_type(&self) -> Type {
        (self.script)()
    }

    pub fn to_value(&self, value: &dyn Any) -> Value {
        (self.to_value)(value)
    }

    pub fn from_value(&self, value: &Value) -> Result<Box<dyn Any>, String> {
        (self.from_value)(value)
    }

    /// Read the bound type at `ptr`.
    ///
    /// # Safety
    /// `ptr` must point to a live, aligned value of the bound Rust type.
    pub unsafe fn load(&self, ptr: *const u8) -> Value {
        (self.load)(ptr)
    }

    /// Assign `value` to the bound type at `ptr`.
    ///
    /// # Safety
    /// `ptr` must point to a live, aligned, uniquely borrowed value of the
    /// bound Rust type.
    pub unsafe fn store(&self, ptr: *mut u8, value: &Value) -> Result<(), String> {
        (self.store)(ptr, value)
    }

    /// Binding for a builtin [`ScriptValue`] type.
    pub fn builtin<T: ScriptValue>() -> Self {
        Self {
            rust: TypeRef::of::<T>(),
            script: T::script_type,
            to_value: |any| {
                // Only called with a `T` (the binding is keyed by T's TypeId).
                let value = any
                    .downcast_ref::<T>()
                    .expect("binding called with its own type");
                value.clone().into_value()
            },
            from_value: |value| {
                T::from_value(value)
                    .map(|v| Box::new(v) as Box<dyn Any>)
                    .ok_or_else(|| format!("expected {}, got {}", T::script_type(), value.kind()))
            },
            // SAFETY (both): upheld by the callers of `TypeBinding::load`/`store`.
            load: |ptr| unsafe { (*ptr.cast::<T>()).clone().into_value() },
            store: |ptr, value| {
                let value = T::from_value(value).ok_or_else(|| {
                    format!("expected {}, got {}", T::script_type(), value.kind())
                })?;
                unsafe { *ptr.cast::<T>() = value };
                Ok(())
            },
        }
    }
}

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

/// A component type visible to scripts under a stable name. Submitted by
/// [`script_component!`](crate::script_component).
pub struct ComponentRegistration {
    pub name: &'static str,
    pub ty: TypeRef,
    pub id: fn() -> ComponentId,
}

inventory::collect!(ComponentRegistration);

/// Components registered in bulk by another registry (e.g. the engine's
/// world component registry), rather than one `script_component!` each.
pub struct ComponentProvider {
    pub components: fn() -> Vec<ProvidedComponent>,
}

inventory::collect!(ComponentProvider);

/// One component from a [`ComponentProvider`]. Its Rust type is looked up
/// from the SceneDB `ComponentId`.
pub struct ProvidedComponent {
    pub name: &'static str,
    pub id: fn() -> ComponentId,
    pub addressing: ComponentAddressing,
}

/// How a script component reference reaches its value. A reference names
/// an entity; `resolve` maps it to the entity that holds the component's
/// value (`None` if there is none) and `object` to the object the
/// component belongs to. [`DIRECT`](Self::DIRECT) treats the referenced
/// entity as both; a provider whose components live on their own
/// entities (Pulsar-Native#1035) supplies its own.
#[derive(Clone, Copy, Debug)]
pub struct ComponentAddressing {
    pub resolve: fn(&World, Entity, ComponentId) -> Option<Entity>,
    pub object: fn(&World, Entity) -> Entity,
}

impl ComponentAddressing {
    /// The value lives on the referenced entity itself.
    pub const DIRECT: Self = Self {
        resolve: |world, entity, id| world.has_component(entity, id).then_some(entity),
        object: |_, entity| entity,
    };
}

/// A value type visible to scripts under a stable name. Submitted by
/// [`script_value_type!`](crate::script_value_type).
pub struct ValueTypeRegistration {
    pub name: &'static str,
    pub ty: TypeRef,
    pub default: fn() -> Object,
    pub binding: fn() -> TypeBinding,
    /// Parses the type's literal text form (see [`Constant::Value`](crate::Constant)).
    /// `None` for types that cannot appear as constants.
    pub decode: Option<fn(&str) -> Result<Object, String>>,
    /// The inverse of `decode`, for saving values; `None` if unsupported.
    pub encode: Option<fn(&Object) -> Result<String, String>>,
}

inventory::collect!(ValueTypeRegistration);

/// A serializer for a script value used as an opaque Gamma `Bytes` event
/// field. Its stable script name is the cross-module schema identity; Rust
/// `TypeId`s and function pointers never cross the DLL boundary.
pub struct EventValueCodecRegistration {
    pub name: &'static str,
    pub encode: fn(&Object) -> Result<Vec<u8>, String>,
    pub decode: fn(&[u8]) -> Result<Object, String>,
}

inventory::collect!(EventValueCodecRegistration);

/// Register a DLL-safe payload codec for an already registered script value
/// type. The custom codecs define the stable wire format; they must be
/// deterministic and compatible across engine/plugin versions.
#[macro_export]
macro_rules! script_event_codec {
    ($ty:ty, $name:expr, encode = $encode:expr, decode = $decode:expr $(,)?) => {
        $crate::__private::inventory::submit! {
            $crate::types::EventValueCodecRegistration {
                name: $name,
                encode: |object| {
                    let encode: fn(&$ty) -> ::std::result::Result<::std::vec::Vec<u8>, String> = $encode;
                    object.downcast_ref::<$ty>()
                        .ok_or_else(|| format!("expected a {}", $name))
                        .and_then(encode)
                },
                decode: |bytes| {
                    let decode: fn(&[u8]) -> ::std::result::Result<$ty, String> = $decode;
                    decode(bytes).map(|value| $crate::value::Object::new($name, value))
                },
            }
        }
    };
}

/// Register component `$ty` for scripts, as `$name` (default: the type's
/// identifier).
#[macro_export]
macro_rules! script_component {
    ($ty:ty) => {
        $crate::script_component!($ty, stringify!($ty));
    };
    ($ty:ty, $name:expr) => {
        $crate::__private::inventory::submit! {
            $crate::types::ComponentRegistration {
                name: $name,
                ty: $crate::__private::TypeRef {
                    id: ::std::any::TypeId::of::<$ty>,
                    name: ::std::any::type_name::<$ty>,
                },
                id: $crate::__private::component_id::<$ty>,
            }
        }
    };
}

/// Register value type `$ty` (`Clone + Default + Send + Sync`) for
/// scripts, as `$name` (default: the type's identifier).
#[macro_export]
macro_rules! script_value_type {
    ($ty:ty) => {
        $crate::script_value_type!($ty, stringify!($ty));
    };
    ($ty:ty, $name:expr) => {
        $crate::__private::inventory::submit! {
            $crate::types::ValueTypeRegistration {
                name: $name,
                ty: $crate::__private::TypeRef {
                    id: ::std::any::TypeId::of::<$ty>,
                    name: ::std::any::type_name::<$ty>,
                },
                default: || $crate::value::Object::new($name, <$ty as ::std::default::Default>::default()),
                binding: || $crate::types::TypeBinding::object::<$ty>($name),
                decode: None,
                encode: None,
            }
        }
    };
    // Decoder and encoder: the type can be a constant and be saved.
    ($ty:ty, $name:expr, decode = $decode:expr, encode = $encode:expr) => {
        $crate::__private::inventory::submit! {
            $crate::types::ValueTypeRegistration {
                name: $name,
                ty: $crate::__private::TypeRef {
                    id: ::std::any::TypeId::of::<$ty>,
                    name: ::std::any::type_name::<$ty>,
                },
                default: || $crate::value::Object::new($name, <$ty as ::std::default::Default>::default()),
                binding: || $crate::types::TypeBinding::object::<$ty>($name),
                decode: Some(|text| {
                    let decode: fn(&str) -> ::std::result::Result<$ty, String> = $decode;
                    decode(text).map(|value| $crate::value::Object::new($name, value))
                }),
                encode: Some(|object| {
                    let encode: fn(&$ty) -> String = $encode;
                    object
                        .downcast_ref::<$ty>()
                        .map(encode)
                        .ok_or_else(|| format!("expected a {}", $name))
                }),
            }
        }
    };
    // As above, with a literal decoder (`fn(&str) -> Result<$ty, String>`)
    // so the type can appear as a `Constant::Value`.
    ($ty:ty, $name:expr, decode = $decode:expr) => {
        $crate::__private::inventory::submit! {
            $crate::types::ValueTypeRegistration {
                name: $name,
                ty: $crate::__private::TypeRef {
                    id: ::std::any::TypeId::of::<$ty>,
                    name: ::std::any::type_name::<$ty>,
                },
                default: || $crate::value::Object::new($name, <$ty as ::std::default::Default>::default()),
                binding: || $crate::types::TypeBinding::object::<$ty>($name),
                decode: Some(|text| {
                    let decode: fn(&str) -> ::std::result::Result<$ty, String> = $decode;
                    decode(text).map(|value| $crate::value::Object::new($name, value))
                }),
                encode: None,
            }
        }
    };
}

impl TypeBinding {
    /// Binding for a registered value type. Used by `script_value_type!`.
    pub fn object<T: Clone + Send + Sync + 'static>(name: &'static str) -> Self {
        fn script<T: 'static>() -> Type {
            match TypeRegistry::global()
                .objects_by_rust
                .get(&TypeId::of::<T>())
            {
                Some(name) => Type::Object((*name).to_owned()),
                None => Type::Object(std::any::type_name::<T>().to_owned()),
            }
        }
        fn to_value<T: Clone + Send + Sync + 'static>(any: &dyn Any) -> Value {
            let value = any
                .downcast_ref::<T>()
                .expect("binding called with its own type");
            let name = TypeRegistry::global()
                .objects_by_rust
                .get(&TypeId::of::<T>())
                .copied()
                .unwrap_or(std::any::type_name::<T>());
            Value::Object(Object::new(name, value.clone()))
        }
        fn from_value<T: Clone + Send + Sync + 'static>(
            value: &Value,
        ) -> Result<Box<dyn Any>, String> {
            match value {
                Value::Object(obj) => obj
                    .downcast_ref::<T>()
                    .map(|v| Box::new(v.clone()) as Box<dyn Any>)
                    .ok_or_else(|| {
                        format!(
                            "expected {}, got {}",
                            std::any::type_name::<T>(),
                            obj.type_name()
                        )
                    }),
                other => Err(format!(
                    "expected {}, got {}",
                    std::any::type_name::<T>(),
                    other.kind()
                )),
            }
        }
        let _ = name;
        Self {
            rust: TypeRef::of::<T>(),
            script: script::<T>,
            to_value: to_value::<T>,
            from_value: from_value::<T>,
            // SAFETY (both): upheld by the callers of `TypeBinding::load`/`store`.
            load: |ptr| to_value::<T>(unsafe { &*ptr.cast::<T>() }),
            store: |ptr, value| {
                let boxed = from_value::<T>(value)?;
                let value = *boxed
                    .downcast::<T>()
                    .expect("from_value returns its own type");
                unsafe { *ptr.cast::<T>() = value };
                Ok(())
            },
        }
    }
}

/// A component type as scripts see it.
#[derive(Clone, Copy, Debug)]
pub struct ComponentBinding {
    pub name: &'static str,
    pub type_id: TypeId,
    id: fn() -> ComponentId,
    addressing: ComponentAddressing,
}

impl ComponentBinding {
    pub fn component_id(&self) -> ComponentId {
        (self.id)()
    }

    /// The entity holding the value a reference to `entity` names.
    pub fn resolve(&self, world: &World, entity: Entity) -> Option<Entity> {
        (self.addressing.resolve)(world, entity, self.component_id())
    }

    /// The object a reference to `entity` belongs to.
    pub fn object(&self, world: &World, entity: Entity) -> Entity {
        (self.addressing.object)(world, entity)
    }
}

/// Every Rust type scripts can see. Built once from the builtins and the
/// `inventory` registrations; see the module docs.
pub struct TypeRegistry {
    by_rust: HashMap<TypeId, TypeBinding>,
    components: HashMap<&'static str, ComponentBinding>,
    components_by_rust: HashMap<TypeId, &'static str>,
    objects: HashMap<&'static str, &'static ValueTypeRegistration>,
    objects_by_rust: HashMap<TypeId, &'static str>,
    ops: HashMap<&'static str, &'static ValueOpsRegistration>,
    event_codecs: HashMap<&'static str, &'static EventValueCodecRegistration>,
}

static GLOBAL: LazyLock<TypeRegistry> = LazyLock::new(TypeRegistry::collect);

impl TypeRegistry {
    pub fn global() -> &'static TypeRegistry {
        &GLOBAL
    }

    fn collect() -> Self {
        let mut registry = Self {
            by_rust: HashMap::new(),
            components: HashMap::new(),
            components_by_rust: HashMap::new(),
            objects: HashMap::new(),
            objects_by_rust: HashMap::new(),
            ops: HashMap::new(),
            event_codecs: HashMap::new(),
        };
        macro_rules! builtin {
            ($($ty:ty),*) => {$(
                registry.by_rust.insert(TypeId::of::<$ty>(), TypeBinding::builtin::<$ty>());
            )*};
        }
        builtin!(
            (),
            bool,
            i8,
            i16,
            i32,
            i64,
            isize,
            u8,
            u16,
            u32,
            u64,
            usize,
            f32,
            f64,
            String,
            Arc<str>,
            Entity
        );

        for reg in inventory::iter::<ComponentRegistration> {
            let binding = ComponentBinding {
                name: reg.name,
                type_id: reg.ty.type_id(),
                id: reg.id,
                addressing: ComponentAddressing::DIRECT,
            };
            if registry.components.insert(reg.name, binding).is_some() {
                tracing::error!("script component name `{}` registered twice", reg.name);
            }
            registry
                .components_by_rust
                .insert(binding.type_id, reg.name);
        }
        // Bulk providers fill in whatever explicit registrations did not.
        for provider in inventory::iter::<ComponentProvider> {
            for provided in (provider.components)() {
                let type_id = pulsar_scenedb::component::type_of((provided.id)());
                if registry.components.contains_key(provided.name)
                    || registry.components_by_rust.contains_key(&type_id)
                {
                    continue;
                }
                let binding = ComponentBinding {
                    name: provided.name,
                    type_id,
                    id: provided.id,
                    addressing: provided.addressing,
                };
                registry.components.insert(provided.name, binding);
                registry.components_by_rust.insert(type_id, provided.name);
            }
        }
        for reg in inventory::iter::<ValueTypeRegistration> {
            if registry.objects.insert(reg.name, reg).is_some() {
                tracing::error!("script value type name `{}` registered twice", reg.name);
            }
            registry.objects_by_rust.insert(reg.ty.type_id(), reg.name);
            registry.by_rust.insert(reg.ty.type_id(), (reg.binding)());
        }
        for ops in inventory::iter::<ValueOpsRegistration> {
            if registry.ops.insert(ops.name, ops).is_some() {
                tracing::error!(
                    "script value type `{}` has two sets of equality and display hooks",
                    ops.name
                );
            }
        }
        for codec in inventory::iter::<EventValueCodecRegistration> {
            if registry.event_codecs.insert(codec.name, codec).is_some() {
                tracing::error!(
                    "script value type `{}` has more than one event codec",
                    codec.name
                );
            }
        }
        registry
    }

    /// The binding for Rust type `ty`, if scripts can see it.
    pub fn binding(&self, ty: TypeId) -> Option<&TypeBinding> {
        self.by_rust.get(&ty)
    }

    /// The script type of Rust type `ty`, if scripts can see it.
    pub fn script_type_of(&self, ty: TypeId) -> Option<Type> {
        if let Some(name) = self.components_by_rust.get(&ty) {
            return Some(Type::Component((*name).to_owned()));
        }
        self.by_rust.get(&ty).map(TypeBinding::script_type)
    }

    pub fn component(&self, name: &str) -> Option<&ComponentBinding> {
        self.components.get(name)
    }

    /// The script name of component type `ty`.
    pub fn component_name(&self, ty: TypeId) -> Option<&'static str> {
        self.components_by_rust.get(&ty).copied()
    }

    pub fn components(&self) -> impl Iterator<Item = &ComponentBinding> {
        self.components.values()
    }

    pub fn value_types(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.objects.keys().copied()
    }

    pub fn value_type_registrations(
        &self,
    ) -> impl Iterator<Item = &'static ValueTypeRegistration> + '_ {
        self.objects.values().copied()
    }

    /// Decode the literal `json` of value type `ty` (a [`Constant::Value`](crate::Constant)).
    pub fn decode_value(&self, ty: &str, json: &str) -> Result<Value, String> {
        let registration = self
            .objects
            .get(ty)
            .ok_or_else(|| format!("unknown value type `{ty}`"))?;
        let decode = registration
            .decode
            .ok_or_else(|| format!("value type `{ty}` has no literal form"))?;
        decode(json).map(Value::Object)
    }

    /// The literal text of a value-type object (the inverse of
    /// [`decode_value`](Self::decode_value)).
    pub fn encode_value(&self, object: &Object) -> Result<String, String> {
        let name = object.type_name();
        let registration = self
            .objects
            .get(name)
            .ok_or_else(|| format!("unknown value type `{name}`"))?;
        let encode = registration
            .encode
            .ok_or_else(|| format!("value type `{name}` cannot be saved"))?;
        encode(object)
    }

    /// Encode a registered object as a payload for Gamma's `Bytes` field.
    /// No Rust type or vtable crosses the DLL boundary.
    pub fn encode_event_value(&self, object: &Object) -> Result<Vec<u8>, String> {
        let codec = self.event_codecs.get(object.type_name()).ok_or_else(|| {
            format!(
                "value type `{}` has no DLL-safe event codec",
                object.type_name()
            )
        })?;
        let payload = (codec.encode)(object)?;
        let name = object.type_name().as_bytes();
        let name_len = u16::try_from(name.len())
            .map_err(|_| format!("event value type name `{}` is too long", object.type_name()))?;
        let mut bytes = Vec::with_capacity(8 + name.len() + payload.len());
        bytes.extend_from_slice(b"PSEV");
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&name_len.to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }

    /// Decode a Gamma `Bytes` event payload at the local script boundary.
    pub fn decode_event_value(&self, name: &str, bytes: &[u8]) -> Result<Object, String> {
        let codec = self
            .event_codecs
            .get(name)
            .ok_or_else(|| format!("value type `{name}` has no DLL-safe event codec"))?;
        if bytes.len() < 8 || &bytes[..4] != b"PSEV" {
            return Err("invalid event value envelope (missing PSEV header)".into());
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != 1 {
            return Err(format!(
                "unsupported event value envelope version {version}"
            ));
        }
        let name_len = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        let name_end = 8usize
            .checked_add(name_len)
            .ok_or_else(|| "event value type name length overflow".to_owned())?;
        let encoded_name = bytes
            .get(8..name_end)
            .ok_or_else(|| "truncated event value type name".to_owned())?;
        let encoded_name = std::str::from_utf8(encoded_name)
            .map_err(|_| "event value type name is not UTF-8".to_owned())?;
        if encoded_name != name {
            return Err(format!(
                "event payload is `{encoded_name}`, expected `{name}`"
            ));
        }
        let object = (codec.decode)(&bytes[name_end..])?;
        if object.type_name() != name {
            return Err(format!(
                "event codec `{name}` decoded an object named `{}`",
                object.type_name()
            ));
        }
        Ok(object)
    }

    /// Whether `ty` names something that exists.
    pub fn is_known(&self, ty: &Type) -> bool {
        match ty {
            Type::Component(name) => self.components.contains_key(name.as_str()),
            Type::Object(name) => self.objects.contains_key(name.as_str()),
            Type::List(element) => self.is_known(element),
            Type::Map(key, value) => self.is_known(key) && self.is_known(value),
            Type::Tuple(items) => items.iter().all(|item| self.is_known(item)),
            _ => true,
        }
    }

    /// The value a register or variable of type `ty` starts with: zero,
    /// `false`, `""`, [`Entity::DANGLING`], a component reference that
    /// resolves to nothing, or the value type's `Default`.
    pub fn default_value(&self, ty: &Type) -> Option<Value> {
        Some(match ty {
            Type::Unit => Value::Unit,
            Type::Bool => Value::Bool(false),
            Type::Int => Value::Int(0),
            Type::Float => Value::Float(0.0),
            Type::Str => Value::Str(Arc::from("")),
            Type::Entity => Value::Entity(Entity::DANGLING),
            Type::Component(name) => {
                let binding = self.components.get(name.as_str())?;
                Value::Component(ComponentRef::new(Entity::DANGLING, binding.component_id()))
            }
            Type::Object(name) => Value::Object((self.objects.get(name.as_str())?.default)()),
            Type::List(element) => {
                self.default_value(element)?;
                Value::List(Arc::new(Vec::new()))
            }
            Type::Map(key, value) => {
                self.default_value(key)?;
                self.default_value(value)?;
                Value::Map(Arc::new(Default::default()))
            }
            Type::Tuple(items) => Value::Tuple(
                items
                    .iter()
                    .map(|item| self.default_value(item))
                    .collect::<Option<Vec<_>>>()?
                    .into(),
            ),
        })
    }
}

/// Equality and printing for a registered value type, so scripts can use
/// `==`, `!=` and string conversion on it. Without these a value type does
/// neither (comparing it is refused at link time, and a value that is
/// compared anyway is never equal). Submitted by
/// [`script_value_ops!`](crate::script_value_ops).
pub struct ValueOpsRegistration {
    /// The type's script name, as given to `script_value_type!`.
    pub name: &'static str,
    pub eq: Option<fn(&Object, &Object) -> bool>,
    pub display: Option<fn(&Object) -> String>,
}

inventory::collect!(ValueOpsRegistration);

/// Give value type `$ty` (already registered with `script_value_type!`)
/// script equality and/or printing:
/// `script_value_ops!(Vec3, "Vec3", eq = |a, b| a == b, display = |v| v.to_string())`.
#[macro_export]
macro_rules! script_value_ops {
    ($ty:ty, $name:expr $(, eq = $eq:expr)? $(, display = $display:expr)? $(,)?) => {
        $crate::__private::inventory::submit! {
            $crate::types::ValueOpsRegistration {
                name: $name,
                eq: $crate::script_value_ops!(@eq $ty $(, $eq)?),
                display: $crate::script_value_ops!(@display $ty $(, $display)?),
            }
        }
    };
    (@eq $ty:ty) => { None };
    (@eq $ty:ty, $eq:expr) => {
        Some(|a, b| {
            let eq: fn(&$ty, &$ty) -> bool = $eq;
            match (a.downcast_ref::<$ty>(), b.downcast_ref::<$ty>()) {
                (Some(a), Some(b)) => eq(a, b),
                _ => false,
            }
        })
    };
    (@display $ty:ty) => { None };
    (@display $ty:ty, $display:expr) => {
        Some(|v| {
            let display: fn(&$ty) -> String = $display;
            v.downcast_ref::<$ty>().map(display).unwrap_or_default()
        })
    };
}

impl TypeRegistry {
    /// Whether `==` is defined for values of `ty`: every part of it, down
    /// to value types, has equality.
    pub fn supports_eq(&self, ty: &Type) -> bool {
        match ty {
            Type::Object(name) => self
                .ops
                .get(name.as_str())
                .is_some_and(|ops| ops.eq.is_some()),
            Type::List(element) => self.supports_eq(element),
            Type::Map(key, value) => self.supports_eq(key) && self.supports_eq(value),
            Type::Tuple(items) => items.iter().all(|item| self.supports_eq(item)),
            _ => true,
        }
    }

    /// Whether string conversion is defined for `ty`, as for
    /// [`supports_eq`](Self::supports_eq).
    pub fn supports_display(&self, ty: &Type) -> bool {
        match ty {
            Type::Object(name) => self
                .ops
                .get(name.as_str())
                .is_some_and(|ops| ops.display.is_some()),
            Type::List(element) => self.supports_display(element),
            Type::Map(key, value) => self.supports_display(key) && self.supports_display(value),
            Type::Tuple(items) => items.iter().all(|item| self.supports_display(item)),
            _ => true,
        }
    }

    /// `a == b` for two objects: `false` when the type has no equality.
    pub fn objects_equal(&self, a: &Object, b: &Object) -> bool {
        a.type_name() == b.type_name()
            && self
                .ops
                .get(a.type_name())
                .and_then(|ops| ops.eq)
                .is_some_and(|eq| eq(a, b))
    }

    /// An object's text: its registered display, or just the type's name.
    pub fn display_object(&self, object: &Object) -> String {
        match self.ops.get(object.type_name()).and_then(|ops| ops.display) {
            Some(display) => display(object),
            None => object.type_name().to_owned(),
        }
    }
}
