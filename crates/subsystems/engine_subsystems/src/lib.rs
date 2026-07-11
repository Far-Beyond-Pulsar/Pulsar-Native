//! # Engine Subsystems Framework
//!
//! Provides a trait-based framework for engine subsystems with:
//!
//! - **Dependency declaration and resolution** — topological sort via Kahn's
//!   algorithm ([`Subsystem::dependencies`], [`SubsystemRegistry::resolve_dependencies`])
//! - **Lifecycle management** — init, shutdown, per-frame updates
//!   ([`SubsystemRegistry::init_all`], [`SubsystemRegistry::shutdown_all`],
//!   [`SubsystemRegistry::update_all`])
//! - **Type-erased subsystem registry** — all subsystems stored as
//!   `Box<dyn Subsystem>`, downcast via `Any` for concrete access
//! - **Shared context** — [`SubsystemContext`] passed to every subsystem
//!   during initialization
//!
//! ## Architecture
//!
//! The registry stores all subsystems as `Box<dyn Subsystem>` — type-erased.
//! Subsystem consumers downcast to the concrete type they know via `Any`.
//!
//! ## Crate dependencies
//!
//! This crate has no engine or UI dependencies — only `thiserror` for
//! error types and `tracing` for logging. Both `engine_backend` (built-in
//! subsystems) and `plugin_editor_api` (plugin-provided subsystems) depend
//! on this crate.
//!
//! ## Module structure
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`id`] | [`SubsystemId`] newtype |
//! | [`context`] | [`SubsystemContext`] — cross-subsystem shared context |
//! | [`error`] | [`SubsystemError`] — typed error enum |
//! | [`trait_def`] | [`Subsystem`] trait — lifecycle hooks + `Any` bound |
//! | [`registry`] | [`SubsystemRegistry`] — registration, dependency resolution, lifecycle |
//!
//! ## Key consumers
//!
//! | Consumer | Role |
//! |----------|------|
//! | `engine_backend` | Owns the primary registry; injects built-in & plugin subsystems |
//! | `plugin_manager` | Collects `Box<dyn Subsystem>` from plugin DLLs |
//! | `plugin_editor_api` | Defines `EditorPluginSubsystems` trait for plugins |

mod context;
mod error;
mod id;
mod registry;
mod trait_def;

#[cfg(test)]
mod tests;

pub use context::SubsystemContext;
pub use error::SubsystemError;
pub use id::SubsystemId;
pub use registry::SubsystemRegistry;
pub use trait_def::Subsystem;
