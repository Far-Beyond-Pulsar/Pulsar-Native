//! [`TypeRegistry`]: every Rust type scripts can see, built once from the
//! builtins and the `inventory` registrations.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use pulsar_scenedb::{ComponentRef, Entity};

use crate::value::{Object, Value};

use super::*;

/// Every Rust type scripts can see. Built once from the builtins and the
/// `inventory` registrations; see the module docs.
pub struct TypeRegistry {
    by_rust: HashMap<TypeId, TypeBinding>,
    components: HashMap<&'static str, ComponentBinding>,
    components_by_rust: HashMap<TypeId, &'static str>,
    objects: HashMap<&'static str, &'static ValueTypeRegistration>,
    pub(super) objects_by_rust: HashMap<TypeId, &'static str>,
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
