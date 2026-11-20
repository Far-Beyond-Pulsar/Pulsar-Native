//! Script classes bound to entities.
//!
//! A **class** is a linked [`Module`](pulsar_script_vm::Module) (compiled by any scripting frontend).
//! An **instance** is one copy of a class's variables, identified by an
//! object id and optionally bound to an [`Entity`](pulsar_scenedb::Entity). The [`ScriptRuntime`]
//! owns the native registry, the loaded native libraries, the classes and
//! the instances, and runs lifecycle events:
//!
//! | event        | exported function signature | when                          |
//! |--------------|-----------------------------|-------------------------------|
//! | `begin_play` | `() -> unit`                | first dispatch after spawning |
//! | `tick`       | `(float delta_time) -> unit`| every frame                   |
//! | `end_play`   | `() -> unit`                | despawn / shutdown            |
//!
//! Any other exported function is a custom event, sent with
//! [`ScriptRuntime::send_event`]. A class need not export any of them.
//!
//! Latent calls: a function that waits (the VM's `Wait`) is suspended and
//! resumed by [`ScriptRuntime::tick_all`] once enough game time has
//! passed; an instance can have several waiting at once. Despawning an
//! instance drops its waiting calls. Reloading a class keeps a waiting call
//! when the new code has the same layout for every function it is
//! suspended in (same name, parameter, return and register types, and
//! instruction count, still waiting / calling at the same pc: see
//! `pulsar_script_vm::Continuation::rebase`), and drops it with a warning
//! naming the class and function otherwise ([`ReloadReport`]).
//!
//! Errors: a script error carries the class and the VM trace
//! with each frame's source location from the module's debug info; a link
//! error the function (and node) that uses what failed to link.
//! [`RuntimeError::details`] breaks either down for an editor.
//!
//! Events: with an [`EventHost`] attached
//! ([`ScriptRuntime::set_event_host`]), a class's declared events are
//! registered with it before the class links, handlers are linked against
//! its catalog, and natives publish through it (`event::emit`, ...). Which
//! instance subscribes to what, and when handlers run, is the host's
//! business (the engine's script driver): the runtime only exposes each
//! class's checked [`subscriptions`](ScriptRuntime::subscriptions) and
//! [`call_function`](ScriptRuntime::call_function) to run a handler.
//!
//! Hot reload: [`ScriptRuntime::reload_class`] swaps a class's code and
//! carries each instance's variables over by stable id (name for data from
//! before ids), runs the class's optional `migrate` hook when its version
//! rises, reports every change, and is all-or-nothing (see
//! [`pulsar_script_vm::migrate`]). [`SavedState`] saves and restores an
//! instance through the same migration;
//! loading, reloading or unloading a native library relinks every class
//! (see [`RelinkReport`]).

// `RuntimeError` carries the VM's rich error site by value; these are cold paths.
#![allow(clippy::result_large_err)]

mod class;
mod error;
mod event_host;
mod json;
mod latent;
mod limits;
mod phase;
mod reload;
mod runtime;
mod state;

pub use class::{BEGIN_PLAY, END_PLAY, TICK};
pub use error::{ErrorDetails, RuntimeError};
pub use event_host::EventHost;
pub use json::{value_from_json, value_from_json_with};
pub use limits::{ScriptLimits, DEFAULT_BUDGET};
pub use phase::{PhaseStats, DEFAULT_PARALLEL_THRESHOLD};
pub use reload::{ChangeKind, DroppedCall, RelinkReport, ReloadReport, VariableChange};
pub use runtime::{InstanceRuntimeStats, ScriptRuntime};
pub use state::{value_to_json, RestoreReport, SavedState, SavedVariable};
