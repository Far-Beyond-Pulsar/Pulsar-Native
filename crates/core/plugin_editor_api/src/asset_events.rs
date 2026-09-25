//! Asset-update notifications.
//!
//! A plugin that rewrites an asset publishes an [`AssetUpdated`] once the
//! files are on disk; the editor and the game subscribe by
//! [`AssetKind`](crate::AssetKind) and refresh what they built from it.
//! For example, the Blueprint editor publishes one for a class after saving
//! or compiling it, and the level editor rebuilds every placed instance of
//! that class.
//!
//! The bus lives in the GPUI-free `pulsar_events` crate so the game runtime
//! shares it; see its docs for delivery rules. It is the host's Gamma bus:
//! a plugin built as a separate dynamic library is attached to it when the
//! editor loads the plugin (`export_plugin!` exports the entry point), so
//! its publishes reach the editor and the editor's reach it
//! (Pulsar-Native#930).

pub use pulsar_events::assets::{
    publish_asset_updated, subscribe_asset_updates, AssetSubscription, AssetUpdated,
};
