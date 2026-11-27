//! What the engine and plugins submit to bind Rust types to script names, and
//! the macros that submit them.

use pulsar_reflection::methods::TypeRef;
use pulsar_scenedb::{ComponentId, Entity, World};

use crate::value::Object;

use super::*;

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
/// entities supplies its own.
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

/// A Rust string newtype scripts read and write as a plain `string`
/// (an asset path, say). Submitted by
/// [`script_text_type!`](crate::script_text_type).
pub struct TextTypeRegistration {
    pub binding: fn() -> TypeBinding,
}

inventory::collect!(TextTypeRegistration);

/// Let scripts read and write `$ty` as a `string`. `$ty` is
/// `Clone + From<String> + Deref<Target = str>`.
#[macro_export]
macro_rules! script_text_type {
    ($ty:ty) => {
        $crate::__private::inventory::submit! {
            $crate::types::TextTypeRegistration {
                binding: || $crate::types::TypeBinding::text::<$ty>(),
            }
        }
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
