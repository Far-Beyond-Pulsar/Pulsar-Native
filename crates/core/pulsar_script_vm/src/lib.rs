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
//! - [`events`]: declared events, handler subscriptions and the `event::*`
//!   natives, over traits the engine's event hub implements.
//!
//! Scripts refer to the world through [`Value::Entity`] and
//! [`Value::Component`] (a liveness-checked `ComponentRef`), never
//! pointers. Types are named by stable script names ([`Type`]); see
//! [`types`] for how they bind to Rust types.

pub(crate) mod adapters;
pub mod capability;
pub mod error;
pub mod events;
pub mod interp;
pub mod library;
pub mod link;
pub mod module;
pub mod native;
pub(crate) mod stdlib;
pub mod types;
pub mod value;
pub mod verify;

pub use capability::{CapabilityPolicy, CAPABILITY_ATTR};
pub use error::{LinkError, ScriptError, ScriptErrorKind, VerifyError};
pub use interp::{Budget, Completion, Continuation, Vm, DEFAULT_MAX_DEPTH};
pub use library::{
    ForwardingAllocator, HostAllocator, LibraryError, LibraryId, LibraryRegistrar, NativeLibraries,
};
pub use events::{EventCatalog, EventSignature, EventSink, EventTarget};
pub use link::{FuncId, Instance, LinkedSubscription, Program};
pub use module::{
    BinOp, Constant, DebugInfo, DebugRange, ErrorSite, EventDecl, EventField, EventRef, Function,
    Import, Instr, Module, ModuleDecodeError, Param, Reg, Signature, SourceLoc, Subscription,
    SubscriptionScope, UnOp, Variable, BINARY_MAGIC, FORMAT_VERSION, MIN_FORMAT_VERSION,
};
pub use native::{
    Host, NativeBuilder, NativeFn, NativeProvider, NativeRegistration, NativeRegistry, Origin,
    PolyNative,
};
pub use types::{ComponentProvider, Obj, ProvidedComponent, ScriptValue, Type, TypeRegistry};
pub use value::{Object, Value};
pub use verify::verify;

/// Used by this crate's macros. Not a stable API.
#[doc(hidden)]
pub mod __private {
    pub use inventory;
    pub use pulsar_reflection::methods::TypeRef;
    pub use pulsar_scenedb::component_id;
}
