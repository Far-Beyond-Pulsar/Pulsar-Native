//! Methods on reflected types, keyed by [`TypeId`].
//!
//! A type registers methods with `#[reflect_methods]` on an `impl` block
//! (see [`crate::reflect_methods`]); each method marked `#[reflect_method]`
//! becomes a [`ReflectedMethod`]: its name, receiver, parameter and return
//! types, behavior flags, free-form attributes, and an `invoke` shim that
//! type-checks every argument before calling the real method.
//!
//! This registry is language-neutral. It knows nothing about any scripting
//! frontend; a frontend (or the script VM's linker) reads the signature to
//! decide how to present or call the method. The older name-keyed
//! registries ([`crate::ComponentMethodRegistration`],
//! [`crate::DynMethodRegistration`]) keep working alongside it.
//!
//! Lookups are by the receiver's `TypeId`, so "the methods callable on a
//! reference to X" is a single map lookup ([`methods_of`]). The map is built
//! once, on first use, from `inventory` registrations collected at link time.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;

use once_cell::sync::Lazy;

/// A Rust type named in a method signature. Function pointers rather than
/// values so a registration stays const-constructible.
#[derive(Clone, Copy)]
pub struct TypeRef {
    pub id: fn() -> TypeId,
    pub name: fn() -> &'static str,
}

impl TypeRef {
    pub fn of<T: Any>() -> Self {
        Self {
            id: TypeId::of::<T>,
            name: std::any::type_name::<T>,
        }
    }

    pub fn type_id(&self) -> TypeId {
        (self.id)()
    }

    pub fn type_name(&self) -> &'static str {
        (self.name)()
    }

    pub fn is<T: Any>(&self) -> bool {
        self.type_id() == TypeId::of::<T>()
    }
}

impl fmt::Debug for TypeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.type_name())
    }
}

impl PartialEq for TypeRef {
    fn eq(&self, other: &Self) -> bool {
        self.type_id() == other.type_id()
    }
}

impl Eq for TypeRef {}

/// How a method takes `self`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReceiverKind {
    /// Associated function: no `self`.
    None,
    /// `&self`.
    Ref,
    /// `&mut self`.
    Mut,
}

/// How a parameter is passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PassMode {
    /// By value: the argument is moved out of its slot.
    Value,
    /// `&T`: the argument stays in its slot.
    Ref,
    /// `&mut T`: the argument stays in its slot, and writes to it are
    /// visible to the caller after the call (out-parameters).
    Mut,
}

#[derive(Clone, Copy, Debug)]
pub struct ParamInfo {
    pub name: &'static str,
    /// The parameter's type without the reference, for `&T`/`&mut T`.
    pub ty: TypeRef,
    pub mode: PassMode,
}

/// What a method promises about its behavior. Frontends map these onto
/// their own concepts (e.g. a node without execution pins).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MethodFlags {
    /// Does not mutate the receiver, arguments or any other state.
    pub side_effect_free: bool,
    /// Returns the same output for the same inputs.
    pub deterministic: bool,
}

impl MethodFlags {
    pub const NONE: Self = Self {
        side_effect_free: false,
        deterministic: false,
    };
    pub const PURE: Self = Self {
        side_effect_free: true,
        deterministic: true,
    };
}

/// Why an `invoke` call was rejected or failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError {
    /// The method takes `self` and no receiver was given, or the receiver
    /// is `&` but the method needs `&mut`.
    ReceiverMissing {
        needed: ReceiverKind,
    },
    /// The receiver is not the method's `Self` type.
    ReceiverType {
        expected: &'static str,
    },
    ArgCount {
        expected: usize,
        found: usize,
    },
    ArgType {
        index: usize,
        expected: &'static str,
    },
    /// The method ran and returned `Err`; the message is its `Display`.
    Failed(String),
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReceiverMissing { needed } => write!(f, "method needs a {needed:?} receiver"),
            Self::ReceiverType { expected } => write!(f, "receiver is not a {expected}"),
            Self::ArgCount { expected, found } => {
                write!(f, "expected {expected} arguments, got {found}")
            }
            Self::ArgType { index, expected } => {
                write!(f, "argument {index} is not a {expected}")
            }
            Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for CallError {}

/// The receiver passed to [`ReflectedMethod::invoke`].
pub enum Receiver<'a> {
    None,
    Ref(&'a dyn Any),
    Mut(&'a mut dyn Any),
}

impl<'a> Receiver<'a> {
    /// Shared view of the receiver as `T`. Used by generated shims.
    pub fn downcast_ref<T: Any>(&self) -> Result<&T, CallError> {
        let value: &dyn Any = match self {
            Self::None => {
                return Err(CallError::ReceiverMissing {
                    needed: ReceiverKind::Ref,
                })
            }
            Self::Ref(value) => *value,
            Self::Mut(value) => &**value,
        };
        value.downcast_ref::<T>().ok_or(CallError::ReceiverType {
            expected: std::any::type_name::<T>(),
        })
    }

    /// Mutable view of the receiver as `T`. Used by generated shims.
    pub fn downcast_mut<T: Any>(&mut self) -> Result<&mut T, CallError> {
        match self {
            Self::Mut(value) => value.downcast_mut::<T>().ok_or(CallError::ReceiverType {
                expected: std::any::type_name::<T>(),
            }),
            _ => Err(CallError::ReceiverMissing {
                needed: ReceiverKind::Mut,
            }),
        }
    }
}

/// Call shim: validates the receiver and every argument, then calls the
/// method. By-value arguments are moved out of their slots (leaving `()`);
/// `&`/`&mut` arguments stay. Returns `None` for a `()` return.
pub type InvokeFn =
    fn(Receiver<'_>, &mut [Box<dyn Any>]) -> Result<Option<Box<dyn Any>>, CallError>;

/// A method's signature and metadata, independent of how it is invoked.
/// Shared by [`ReflectedMethod`] and methods registered elsewhere (e.g.
/// SceneDB component methods that receive the world), so frontends present
/// every callable the same way.
#[derive(Clone, Copy)]
pub struct MethodInfo {
    pub name: &'static str,
    pub doc: &'static str,
    /// The script-visible parameters (not the receiver or other context).
    pub params: &'static [ParamInfo],
    /// `None` for `()`. For a method returning `Result<T, E>`, this is `T`;
    /// an `Err` surfaces as [`CallError::Failed`].
    pub ret: Option<TypeRef>,
    pub flags: MethodFlags,
    /// Free-form `key = "value"` attributes (e.g. `category`), passed
    /// through untouched for frontends.
    pub attrs: &'static [(&'static str, &'static str)],
}

impl MethodInfo {
    pub fn attr(&self, key: &str) -> Option<&'static str> {
        self.attrs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }
}

impl fmt::Debug for MethodInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MethodInfo")
            .field("name", &self.name)
            .field("params", &self.params)
            .field("ret", &self.ret)
            .field("flags", &self.flags)
            .field("attrs", &self.attrs)
            .finish_non_exhaustive()
    }
}

/// One method of a reflected type.
pub struct ReflectedMethod {
    pub info: MethodInfo,
    pub receiver: ReceiverKind,
    pub invoke: InvokeFn,
}

impl ReflectedMethod {
    pub fn name(&self) -> &'static str {
        self.info.name
    }

    pub fn call(
        &self,
        receiver: Receiver<'_>,
        args: &mut [Box<dyn Any>],
    ) -> Result<Option<Box<dyn Any>>, CallError> {
        (self.invoke)(receiver, args)
    }
}

impl fmt::Debug for ReflectedMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReflectedMethod")
            .field("info", &self.info)
            .field("receiver", &self.receiver)
            .finish_non_exhaustive()
    }
}

/// One `#[reflect_methods]` impl block's methods. A type may have several.
pub struct TypeMethodRegistration {
    pub ty: TypeRef,
    pub methods: &'static [ReflectedMethod],
}

inventory::collect!(TypeMethodRegistration);

/// Every registered method, grouped by receiver type.
pub struct MethodRegistry {
    by_type: HashMap<TypeId, TypeMethods>,
}

/// The methods registered for one type, in registration order within each
/// impl block, with impl blocks in link order.
pub struct TypeMethods {
    pub ty: TypeRef,
    pub methods: Vec<&'static ReflectedMethod>,
}

impl MethodRegistry {
    fn collect() -> Self {
        let mut by_type: HashMap<TypeId, TypeMethods> = HashMap::new();
        for registration in inventory::iter::<TypeMethodRegistration> {
            let entry = by_type
                .entry(registration.ty.type_id())
                .or_insert_with(|| TypeMethods {
                    ty: registration.ty,
                    methods: Vec::new(),
                });
            for method in registration.methods {
                if entry
                    .methods
                    .iter()
                    .any(|m| m.info.name == method.info.name)
                {
                    // Overloads are not supported: names are how scripts
                    // bind to methods. Keep the first, loudly.
                    tracing::error!(
                        "duplicate reflected method {}::{}; keeping the first",
                        registration.ty.type_name(),
                        method.info.name
                    );
                    continue;
                }
                entry.methods.push(method);
            }
        }
        Self { by_type }
    }

    pub fn methods_of(&self, ty: TypeId) -> &[&'static ReflectedMethod] {
        self.by_type.get(&ty).map_or(&[], |t| t.methods.as_slice())
    }

    pub fn find(&self, ty: TypeId, name: &str) -> Option<&'static ReflectedMethod> {
        self.methods_of(ty)
            .iter()
            .copied()
            .find(|m| m.info.name == name)
    }

    /// Every type with at least one registered method.
    pub fn types(&self) -> impl Iterator<Item = &TypeMethods> {
        self.by_type.values()
    }
}

pub static METHOD_REGISTRY: Lazy<MethodRegistry> = Lazy::new(MethodRegistry::collect);

/// Methods registered on the type with `TypeId` `ty` (empty if none).
pub fn methods_of(ty: TypeId) -> &'static [&'static ReflectedMethod] {
    METHOD_REGISTRY.methods_of(ty)
}

/// Methods registered on `T`.
pub fn methods_for<T: Any>() -> &'static [&'static ReflectedMethod] {
    methods_of(TypeId::of::<T>())
}

/// The method `name` on the type with `TypeId` `ty`.
pub fn find_method(ty: TypeId, name: &str) -> Option<&'static ReflectedMethod> {
    METHOD_REGISTRY.find(ty, name)
}

/// Support code for `#[reflect_methods]` shims. Not a stable API.
#[doc(hidden)]
pub mod __private {
    use super::*;

    pub fn check_arg_count(args: &[Box<dyn Any>], expected: usize) -> Result<(), CallError> {
        if args.len() == expected {
            Ok(())
        } else {
            Err(CallError::ArgCount {
                expected,
                found: args.len(),
            })
        }
    }

    pub fn check_arg<T: Any>(args: &[Box<dyn Any>], index: usize) -> Result<(), CallError> {
        if args[index].is::<T>() {
            Ok(())
        } else {
            Err(CallError::ArgType {
                index,
                expected: std::any::type_name::<T>(),
            })
        }
    }

    /// Move a (pre-checked) by-value argument out of its slot.
    pub fn take<T: Any>(slot: &mut Box<dyn Any>) -> T {
        let boxed = std::mem::replace(slot, Box::new(()));
        *boxed
            .downcast::<T>()
            .unwrap_or_else(|_| unreachable!("argument type pre-checked"))
    }

    pub fn borrow<T: Any>(slot: &Box<dyn Any>) -> &T {
        slot.downcast_ref::<T>()
            .unwrap_or_else(|| unreachable!("argument type pre-checked"))
    }

    pub fn borrow_mut<T: Any>(slot: &mut Box<dyn Any>) -> &mut T {
        slot.downcast_mut::<T>()
            .unwrap_or_else(|| unreachable!("argument type pre-checked"))
    }

    /// Box a return value; `()` becomes `None`.
    pub fn ret<T: Any>(value: T) -> Option<Box<dyn Any>> {
        if TypeId::of::<T>() == TypeId::of::<()>() {
            None
        } else {
            Some(Box::new(value))
        }
    }

    pub fn ret_result<T: Any, E: fmt::Display>(
        value: Result<T, E>,
    ) -> Result<Option<Box<dyn Any>>, CallError> {
        value.map(ret).map_err(|e| CallError::Failed(e.to_string()))
    }
}
