//! Game project generation: the one owner of every file the engine
//! generates in a game project.
//!
//! ```text
//! <project>/
//!   Pulsar.toml              user: name, version, plugins
//!   scripts/<crate>/         user: gameplay crates (engine crates as { workspace = true })
//!   Cargo.toml               generated, committed: the virtual workspace, every path relative
//!   Cargo.lock               seeded from the engine's, then Cargo's; committed
//!   .pulsar/sdk/             generated per machine: the engine SDK mirror (sdk)
//!   .pulsar/build/game/      generated: the game crate (glue)
//!   .pulsar/build/generated.json   what was generated, and its hashes (apply)
//! ```
//!
//! [`sync`] brings all of it up to date: it mirrors the engine SDK
//! ([`sdk`]), reads the project into a [`model::ProjectModel`], renders every
//! generated file in memory ([`render`]), and applies the result against the
//! generation record ([`apply`]). No Cargo file anywhere holds a path
//! outside the project. See `.agents/PROJECT_GENERATION_PLAN.md`.

pub mod apply;
pub mod descriptor;
pub mod error;
mod hash;
pub mod model;
pub mod paths;
pub mod render;
pub mod sdk;
mod sync;
mod toml_out;

pub use error::SyncError;
pub use sdk::EngineSdk;
pub use sync::{sync, sync_with, SyncOptions, SyncReport};
