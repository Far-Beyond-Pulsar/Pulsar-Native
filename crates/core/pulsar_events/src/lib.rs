//! Engine-wide event plumbing for Pulsar.
//!
//! [`assets`]: an asset-update bus. Anything that rewrites an asset (a
//! Blueprint editor saving a class, a mesh importer, a material editor)
//! publishes an [`AssetUpdated`]; the level editor, the game runtime and
//! other tools subscribe by [`AssetKind`] and refresh what they built from
//! it. The bus carries no asset-specific logic: kinds are the shared
//! `ui_types_common::AssetKind`, including plugin-defined `Custom` kinds.
//!
//! This crate is GPUI-free so the game runtime can use it; editor plugins
//! reach it through `plugin_editor_api`, which re-exports the asset API.

pub mod assets;

pub use assets::{AssetSubscription, AssetUpdated, publish_asset_updated, subscribe_asset_updates};
pub use ui_types_common::AssetKind;
