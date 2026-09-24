//! Engine script bytecode and its virtual machine.
//!
//! This is the only thing the engine core knows about scripting. Every
//! scripting language (Blueprints, TypeScript, ..) is a plugin that
//! compiles its sources to a [`Module`]; the engine verifies, links and
//! runs modules without knowing where they came from.
//!
//! - [`module`]: the serializable module format (functions over typed
//!   registers, explicit jumps, imports by name and signature).
//! - [`verify`]: structural and type verification.
//! - [`native`]: natives (Rust functions scripts call) and the
//!   [`NativeRegistry`] they are linked from, with a query API for
//!   frontends (`functions()`, `methods_for(type)`).
//! - [`adapters`] / [`stdlib`]: natives generated from reflection
//!   (reflected methods, fields) and SceneDB (component methods,
//!   accessors), and the standard library.
//! - [`library`]: hot-reloadable native libraries.
//! - [`link`] / [`interp`]: linking into a [`Program`] and running it.
//!
//! Scripts refer to the world through [`Value::Entity`] and
//! [`Value::Component`] (a liveness-checked `ComponentRef`), never
//! pointers. Types are named by stable script names ([`Type`]); see
//! [`types`] for how they bind to Rust types.

pub(crate) mod adapters;
pub mod error;
pub mod interp;
pub mod library;
pub mod link;
pub mod module;
pub mod native;
pub(crate) mod stdlib;
pub mod types;
pub mod value;
pub mod verify;

pub use error::{LinkError, ScriptError, ScriptErrorKind, VerifyError};
pub use interp::{Budget, Vm};
pub use library::{LibraryError, LibraryId, LibraryRegistrar, NativeLibraries};
pub use link::{FuncId, Instance, Program};
pub use module::{
    BinOp, Constant, Function, Import, Instr, Module, Param, Reg, Signature, UnOp, Variable,
    FORMAT_VERSION,
};
pub use native::{Host, NativeBuilder, NativeFn, NativeRegistration, NativeRegistry, Origin};
pub use types::{Obj, ScriptValue, Type, TypeRegistry};
pub use value::{Object, Value};
pub use verify::verify;

/// Used by this crate's macros. Not a stable API.
#[doc(hidden)]
pub mod __private {
    pub use inventory;
    pub use pulsar_reflection::methods::TypeRef;
    pub use pulsar_scenedb::component_id;
}
