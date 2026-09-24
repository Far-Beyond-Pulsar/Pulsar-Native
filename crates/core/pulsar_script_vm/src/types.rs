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
use pulsar_scenedb::{ComponentId, ComponentRef, Entity};
use serde::{Deserialize, Serialize};

use crate::value::{Object, Value};

/// The type of a register, variable, parameter or return value.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
}

impl Type {
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
                let value = any.downcast_ref::<T>().expect("binding called with its own type");
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
                let value = T::from_value(value)
                    .ok_or_else(|| format!("expected {}, got {}", T::script_type(), value.kind()))?;
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
int_value!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

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
        match TypeRegistry::global().objects_by_rust.get(&TypeId::of::<T>()) {
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
}

/// A value type visible to scripts under a stable name. Submitted by
/// [`script_value_type!`](crate::script_value_type).
pub struct ValueTypeRegistration {
    pub name: &'static str,
    pub ty: TypeRef,
    pub default: fn() -> Object,
    pub binding: fn() -> TypeBinding,
}

inventory::collect!(ValueTypeRegistration);

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
            }
        }
    };
}

impl TypeBinding {
    /// Binding for a registered value type. Used by `script_value_type!`.
    pub fn object<T: Clone + Send + Sync + 'static>(name: &'static str) -> Self {
        fn script<T: 'static>() -> Type {
            match TypeRegistry::global().objects_by_rust.get(&TypeId::of::<T>()) {
                Some(name) => Type::Object((*name).to_owned()),
                None => Type::Object(std::any::type_name::<T>().to_owned()),
            }
        }
        fn to_value<T: Clone + Send + Sync + 'static>(any: &dyn Any) -> Value {
            let value = any.downcast_ref::<T>().expect("binding called with its own type");
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
                    .ok_or_else(|| format!("expected {}, got {}", std::any::type_name::<T>(), obj.type_name())),
                other => Err(format!("expected {}, got {}", std::any::type_name::<T>(), other.kind())),
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
                let value = *boxed.downcast::<T>().expect("from_value returns its own type");
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
}

impl ComponentBinding {
    pub fn component_id(&self) -> ComponentId {
        (self.id)()
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
        };
        macro_rules! builtin {
            ($($ty:ty),*) => {$(
                registry.by_rust.insert(TypeId::of::<$ty>(), TypeBinding::builtin::<$ty>());
            )*};
        }
        builtin!((), bool, i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64, String, Arc<str>, Entity);

        for reg in inventory::iter::<ComponentRegistration> {
            let binding = ComponentBinding { name: reg.name, type_id: reg.ty.type_id(), id: reg.id };
            if registry.components.insert(reg.name, binding).is_some() {
                tracing::error!("script component name `{}` registered twice", reg.name);
            }
            registry.components_by_rust.insert(binding.type_id, reg.name);
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
                let binding = ComponentBinding { name: provided.name, type_id, id: provided.id };
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

    pub fn value_type_registrations(&self) -> impl Iterator<Item = &'static ValueTypeRegistration> + '_ {
        self.objects.values().copied()
    }

    /// Whether `ty` names something that exists.
    pub fn is_known(&self, ty: &Type) -> bool {
        match ty {
            Type::Component(name) => self.components.contains_key(name.as_str()),
            Type::Object(name) => self.objects.contains_key(name.as_str()),
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
        })
    }
}
