//! [`TypeBinding`] and [`ComponentBinding`]: how one Rust type or component
//! converts to and from script values.

use std::any::{Any, TypeId};

use pulsar_reflection::methods::TypeRef;
use pulsar_scenedb::{ComponentId, Entity, World};

use crate::value::{Object, Value};

use super::*;

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
            // SAFETY: upheld by the callers of `TypeBinding::load`, which must
            // pass a live, aligned `T` (see its `# Safety`).
            load: |ptr| unsafe { (*ptr.cast::<T>()).clone().into_value() },
            store: |ptr, value| {
                let value = T::from_value(value).ok_or_else(|| {
                    format!("expected {}, got {}", T::script_type(), value.kind())
                })?;
                // SAFETY: upheld by the callers of `TypeBinding::store`, which
                // must pass a live, aligned, uniquely borrowed `T`; the old
                // value is dropped by the assignment.
                unsafe { *ptr.cast::<T>() = value };
                Ok(())
            },
        }
    }
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
            // SAFETY: upheld by the callers of `TypeBinding::load`, which must
            // pass a live, aligned `T` (see its `# Safety`).
            load: |ptr| to_value::<T>(unsafe { &*ptr.cast::<T>() }),
            store: |ptr, value| {
                let boxed = from_value::<T>(value)?;
                let value = *boxed
                    .downcast::<T>()
                    .expect("from_value returns its own type");
                // SAFETY: upheld by the callers of `TypeBinding::store`, which
                // must pass a live, aligned, uniquely borrowed `T`; the old
                // value is dropped by the assignment.
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
    pub(super) id: fn() -> ComponentId,
    pub(super) addressing: ComponentAddressing,
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
