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

mod binding;
mod registration;
mod registry;
mod script_value;

pub use binding::{ComponentBinding, TypeBinding};
pub use registration::{
    ComponentAddressing, ComponentProvider, ComponentRegistration, EventValueCodecRegistration,
    ProvidedComponent, ValueOpsRegistration, ValueTypeRegistration,
};
pub use registry::TypeRegistry;
pub use script_value::{Obj, Outcome, ScriptKey, ScriptValue};

use std::fmt;

use serde::{Deserialize, Serialize};

/// The type of a register, variable, parameter or return value.
///
/// The binary decoding is written by hand (below): it bounds the nesting
/// depth, which the derived one cannot.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, bincode::Encode)]
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

/// The deepest nesting of `List`, `Map` and `Tuple` a decoded [`Type`] may
/// have. Decoding recurses once per level, and a binary module is not
/// trusted: a few hundred thousand bytes claiming as many levels would
/// otherwise overflow the stack. Nothing legitimate nests types this deep.
pub const MAX_TYPE_DEPTH: usize = 32;

/// Decode a [`Type`] from the bytes the derived [`bincode::Encode`] writes
/// (the variant index as a `u32`, then the fields; a `Vec` as its length
/// then its items), refusing nesting past [`MAX_TYPE_DEPTH`].
fn decode_type<D: bincode::de::Decoder>(
    decoder: &mut D,
    depth: usize,
) -> Result<Type, bincode::error::DecodeError> {
    use bincode::error::{AllowedEnumVariants, DecodeError};
    use bincode::Decode;

    if depth > MAX_TYPE_DEPTH {
        return Err(DecodeError::Other("a type is nested too deeply"));
    }
    let variant = <u32 as Decode<D::Context>>::decode(decoder)?;
    Ok(match variant {
        0 => Type::Unit,
        1 => Type::Bool,
        2 => Type::Int,
        3 => Type::Float,
        4 => Type::Str,
        5 => Type::Entity,
        6 => Type::Component(<String as Decode<D::Context>>::decode(decoder)?),
        7 => Type::Object(<String as Decode<D::Context>>::decode(decoder)?),
        8 => Type::List(Box::new(decode_type(decoder, depth + 1)?)),
        9 => {
            let key = decode_type(decoder, depth + 1)?;
            let value = decode_type(decoder, depth + 1)?;
            Type::Map(Box::new(key), Box::new(value))
        }
        10 => {
            // As `Vec<Type>` decodes, claiming the items' size against the
            // configured limit before allocating for them.
            let len = <u64 as Decode<D::Context>>::decode(decoder)?;
            let len = usize::try_from(len).map_err(|_| DecodeError::OutsideUsizeRange(len))?;
            decoder.claim_container_read::<Type>(len)?;
            let mut items = Vec::with_capacity(len);
            for _ in 0..len {
                decoder.unclaim_bytes_read(std::mem::size_of::<Type>());
                items.push(decode_type(decoder, depth + 1)?);
            }
            Type::Tuple(items)
        }
        found => {
            return Err(DecodeError::UnexpectedVariant {
                found,
                type_name: "Type",
                allowed: &AllowedEnumVariants::Range { min: 0, max: 10 },
            })
        }
    })
}

impl<Context> bincode::Decode<Context> for Type {
    fn decode<D: bincode::de::Decoder<Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        decode_type(decoder, 0)
    }
}

impl<'de, Context> bincode::BorrowDecode<'de, Context> for Type {
    fn borrow_decode<D: bincode::de::BorrowDecoder<'de, Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        decode_type(decoder, 0)
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
