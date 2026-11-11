//! Executable inventory from the SceneDB corrective plan (Pulsar-Native#1035).
//!
//! The closure ledger (`ledger.toml`, beside this crate's manifest)
//! has one row per exposed component class, GPU-bearing schema, GPU buffer
//! key, render pass crate and lifecycle call site. Each row carries two kinds
//! of column:
//!
//! - **facts** (registration flags, buffer producers/consumers, call-site
//!   counts) that this crate recomputes from the linked registries and the
//!   source tree, and that the `ledger` test requires to match exactly;
//! - **dispositions** (owner, target contract, test, status) that a person
//!   writes, and that the test requires to be present.
//!
//! A new registration, buffer, pass or call site therefore fails the test
//! until someone records where it belongs in the corrective plan.
//!
//! Each plugin owns the rows of what it ships, in its own
//! `plugins/vendor/<dir>/ledger.toml` (Pulsar-Native#1606, [`home`]); the
//! engine's rows are in this crate's. The test checks the engine's rows and
//! every installed plugin's, so a plugin repository changes its buffers
//! without editing Pulsar-Native's ledger.

// Rust only links a dependency that is referenced. These crates are needed
// solely for their link-time registrations (`inventory` statics), which the
// code below never names directly.
extern crate engine_backend;
extern crate helio_component;
extern crate installed_plugins;
extern crate pulsar_class;
extern crate pulsar_scene_model;

pub mod architecture;
pub mod home;
pub mod ledger;
pub mod linked;
pub mod source;

use std::path::PathBuf;

// std::fs allowed: the inventory reads the engine's own source checkout (a CI/dev check), not a project
#[allow(clippy::disallowed_methods)]
/// The Pulsar-Native repository root (this crate lives at `crates/core/scene_inventory`).
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repository root exists")
}

/// Path of the engine's closure ledger (each plugin's is beside its
/// descriptor: [`home::ledger_files`]).
pub fn ledger_path() -> PathBuf {
    repo_root().join(home::CORE_LEDGER)
}
