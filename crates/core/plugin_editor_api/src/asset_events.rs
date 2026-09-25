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
//! shares it; see its docs for delivery rules.

pub use pulsar_events::assets::{
    publish_asset_updated, subscribe_asset_updates, AssetSubscription, AssetUpdated,
};
