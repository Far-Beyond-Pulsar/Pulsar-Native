//! Runtime values held in registers and instance variables.

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use pulsar_scenedb::{ComponentRef, Entity};

use crate::types::Type;

/// A script value. Every variant is owned and `Clone`; reading a register
/// clones (strings share their buffer), so values are never aliased or
/// freed twice.
#[derive(Clone)]
pub enum Value {
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Arc<str>),
    Entity(Entity),
    /// Liveness-checked reference to a component; see [`ComponentRef`].
    Component(ComponentRef),
    Object(Object),
    /// Shared copy-on-write storage: cloning is cheap and a write never
    /// shows through another copy.
    List(Arc<Vec<Value>>),
    Map(Arc<BTreeMap<MapKey, Value>>),
    Tuple(Arc<[Value]>),
}

/// A map key: the script types a map can be keyed by, ordered.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MapKey {
    Bool(bool),
    Int(i64),
    Str(Arc<str>),
}

impl MapKey {
    /// The key a value is, if it is `bool`, `int` or `string`.
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Bool(b) => Some(Self::Bool(*b)),
            Value::Int(i) => Some(Self::Int(*i)),
            Value::Str(s) => Some(Self::Str(s.clone())),
            _ => None,
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Self::Bool(b) => Value::Bool(*b),
            Self::Int(i) => Value::Int(*i),
            Self::Str(s) => Value::Str(s.clone()),
        }
    }
}

impl Value {
    /// A list value.
    pub fn list(items: Vec<Value>) -> Self {
        Self::List(Arc::new(items))
    }

    pub fn tuple(items: Vec<Value>) -> Self {
        Self::Tuple(items.into())
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }

    /// Short description of the variant, for error messages.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Unit => "unit",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Str(_) => "string",
            Self::Entity(_) => "entity",
            Self::Component(_) => "component",
            Self::Object(obj) => obj.type_name(),
            Self::List(_) => "list",
            Self::Map(_) => "map",
            Self::Tuple(_) => "tuple",
        }
    }

    /// Whether this value can live in a register of type `ty`. Component
    /// and object names are checked by the verifier, not here.
    pub fn fits(&self, ty: &Type) -> bool {
        match (self, ty) {
            (Self::List(items), Type::List(element)) => items.iter().all(|item| item.fits(element)),
            (Self::Map(entries), Type::Map(key, value)) => {
                entries.iter().all(|(k, v)| k.to_value().fits(key) && v.fits(value))
            }
            (Self::Tuple(items), Type::Tuple(types)) => {
                items.len() == types.len() && items.iter().zip(types).all(|(item, ty)| item.fits(ty))
            }
            _ => matches!(
                (self, ty),
                (Self::Unit, Type::Unit)
                    | (Self::Bool(_), Type::Bool)
                    | (Self::Int(_), Type::Int)
                    | (Self::Float(_), Type::Float)
                    | (Self::Str(_), Type::Str)
                    | (Self::Entity(_), Type::Entity)
                    | (Self::Component(_), Type::Component(_))
                    | (Self::Object(_), Type::Object(_))
            ),
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_entity(&self) -> Option<Entity> {
        match self {
            Self::Entity(e) => Some(*e),
            _ => None,
        }
    }

    pub fn as_component(&self) -> Option<ComponentRef> {
        match self {
            Self::Component(c) => Some(*c),
            _ => None,
        }
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Self::Float(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Self::Str(v.into())
    }
}

impl From<Entity> for Value {
    fn from(v: Entity) -> Self {
        Self::Entity(v)
    }
}

impl From<ComponentRef> for Value {
    fn from(v: ComponentRef) -> Self {
        Self::Component(v)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unit => f.write_str("()"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => write!(f, "{v:?}"),
            Self::Str(v) => write!(f, "{v:?}"),
            Self::Entity(v) => write!(f, "{v:?}"),
            Self::Component(v) => write!(f, "{v:?}"),
            Self::Object(v) => write!(f, "{}(..)", v.type_name()),
            Self::List(items) => f.debug_list().entries(items.iter()).finish(),
            Self::Map(entries) => f.debug_map().entries(entries.iter().map(|(k, v)| (k, v))).finish(),
            Self::Tuple(items) => {
                let mut tuple = f.debug_tuple("");
                for item in items.iter() {
                    tuple.field(item);
                }
                tuple.finish()
            }
        }
    }
}

/// Equality as scripts see it: objects never compare equal (the verifier
/// rejects comparing them).
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unit, Self::Unit) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::Float(a), Self::Float(b)) => a == b,
            (Self::Str(a), Self::Str(b)) => a == b,
            (Self::Entity(a), Self::Entity(b)) => a == b,
            (Self::Component(a), Self::Component(b)) => a == b,
            (Self::List(a), Self::List(b)) => a == b,
            (Self::Map(a), Self::Map(b)) => a == b,
            (Self::Tuple(a), Self::Tuple(b)) => a == b,
            _ => false,
        }
    }
}

/// A value of a registered value type, boxed with its clone function.
pub struct Object {
    name: &'static str,
    value: Box<dyn Any + Send + Sync>,
    clone: fn(&(dyn Any + Send + Sync)) -> Box<dyn Any + Send + Sync>,
}

impl Object {
    pub fn new<T: Clone + Send + Sync + 'static>(name: &'static str, value: T) -> Self {
        fn clone<T: Clone + Send + Sync + 'static>(
            value: &(dyn Any + Send + Sync),
        ) -> Box<dyn Any + Send + Sync> {
            Box::new(value.downcast_ref::<T>().expect("object holds its own type").clone())
        }
        Self { name, value: Box::new(value), clone: clone::<T> }
    }

    /// The script type name.
    pub fn type_name(&self) -> &'static str {
        self.name
    }

    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        self.value.downcast_ref()
    }

    pub fn downcast_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.value.downcast_mut()
    }

    pub fn as_any(&self) -> &dyn Any {
        &*self.value
    }

    pub fn as_any_mut(&mut self) -> &mut dyn Any {
        &mut *self.value
    }
}

impl Clone for Object {
    fn clone(&self) -> Self {
        Self { name: self.name, value: (self.clone)(&*self.value), clone: self.clone }
    }
}
