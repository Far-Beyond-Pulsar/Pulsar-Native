//! Traits and gameplay roles.
//!
//! A **trait** is a named contract: methods an implementer promises to
//! provide (`*.trait.json`, language-agnostic project data). A **role** is a
//! job a level hands to an implementer of a trait (the game mode, the player
//! controller). This crate is the one owner of both:
//!
//! | Module | What |
//! |---|---|
//! | [`records`] | a trait asset, parsed tolerantly ([`TraitRecord`]) |
//! | [`db`] | the resolved model: maps, supertrait closure, effective implementers ([`TraitDb`]) |
//! | [`sources`] | where implementers come from: [`TraitImplementerSource`] registrations (Blueprints from the Blueprint plugin, native components from [`NativeTraitImpl`]), and the [`Binding`] the runtime calls through |
//! | [`index`] | the project's one index over every source and the mounted traits ([`TraitIndex`], `.pulsar/trait_index.json`) |
//! | [`conformance`] | does an implementer provide what its traits require? |
//! | [`roles`] | the role registry ([`RoleRegistration`]) and a level's resolved roles ([`WorldRoles`]) |
//! | [`builtin`] | the engine's own trait files (the `pulsar/` mount) and roles |
//!
//! The engine's and the plugins' traits are files in their read-only VFS
//! mounts (`engine_fs::virtual_fs::mounts`), namespaced by their owner
//! (`pulsar.GameMode`, `<plugin id>.<Trait>`) and versioned
//! (`meta.contractVersion`). The runtime dispatches trait methods through
//! `pulsar_game`'s `RuntimeTraits`, to a script class's handler or a native
//! component's method alike. See `.agents/TRAITS.md`.

pub mod builtin;
pub mod conformance;
pub mod db;
pub mod index;
pub mod records;
pub mod roles;
pub mod sources;

pub use db::{Declaration, Implementation, TraitDb};
pub use index::{resolve_declared, ImplementerEntry, TraitIndex, TRAIT_ASSET_KIND};
pub use records::{TraitMethodRecord, TraitParamRecord, TraitRecord};
pub use roles::{BindingStatus, RoleBinding, RoleRegistration, RoleSelection, RoleStart, WorldRoles};
pub use sources::{Binding, Declared, NativeTraitImpl, Scanned, TraitImplementerSource};

/// The key a project's snapshot is published under (shared with plugins
/// through `pulsar_reflection`).
pub use db::scope_key;

#[doc(hidden)]
pub use inventory;
#[doc(hidden)]
pub use pulsar_plugins;
