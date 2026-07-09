//! Subsystem Framework
//!
//! Re-exports from the shared `engine_subsystems` crate. The canonical
//! `Subsystem` trait, `SubsystemRegistry`, and related types live in
//! `crates/engine_subsystems` so they can be shared across `engine_backend`,
//! `plugin_manager`, and plugin DLLs without circular dependencies.
//!
//! See `crates/engine_subsystems/src/lib.rs` for full documentation.

pub use engine_subsystems::{
    Subsystem, SubsystemContext, SubsystemError, SubsystemId, SubsystemRegistry,
};
pub use engine_subsystems::SubsystemRegistry as Registry;


