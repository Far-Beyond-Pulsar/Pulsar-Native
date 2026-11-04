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

// Rust only links a dependency that is referenced. These crates are needed
// solely for their link-time registrations (`inventory` statics), which the
// code below never names directly.
extern crate engine_backend;
extern crate helio_component;
extern crate input_plugin;
extern crate physics_plugin;
extern crate pulsar_class;
extern crate pulsar_physics;
extern crate pulsar_scene_model;
extern crate voxel_component;
extern crate decals_component;
extern crate lens_component;
extern crate particles_component;
extern crate portals_component;
extern crate splines_component;
extern crate sprites_component;
extern crate fog_component;
extern crate foliage_component;
extern crate sky_component;
extern crate post_component;
extern crate water_component;

pub mod architecture;
pub mod ledger;
pub mod linked;
pub mod source;

use std::path::PathBuf;

/// The Pulsar-Native repository root (this crate lives at `crates/core/scene_inventory`).
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repository root exists")
}

/// Path of the checked-in closure ledger.
pub fn ledger_path() -> PathBuf {
    repo_root().join("crates/core/scene_inventory/ledger.toml")
}
