//! The plugins the editor links into its binary.
//!
//! By default only `render_bps`, the render graph's own plugin. Every other
//! installed plugin is its own library, built by `tools/plugin_builder` (one
//! generated crate per plugin under `plugins/dll/`) and loaded from
//! `plugins/editor` at startup, so a plugin's memory is its library's and its
//! leaks can be told apart from the editor's. `plugins/installed` still links
//! every plugin for games and tests, which do not load libraries.
//!
//! With the `all` feature (the editor's `link-plugins`) every installed plugin
//! is linked instead, with its editor registrations: the plugins register
//! through their roots' `build` exactly as a game links them, there is one copy
//! of each (the plugin loader skips the library of a plugin that is linked,
//! `plugin_manager`), and nothing is loaded from `plugins/editor` but plugins
//! that are only libraries (hub-installed, third party).
//!
//! Nothing here names a plugin: the linked set is `render_bps` or, with `all`,
//! `plugins/installed` (generated from the descriptors, and checked against
//! this crate by `plugins/installed/tests/linked_set.rs`).

pub use render_bps_plugin;

/// Every installed plugin, linked (feature `all`): the roots through
/// `installed_plugins`, which re-exports each.
#[cfg(feature = "all")]
pub use installed_plugins;

/// Whether this build links every installed plugin (feature `all`) rather than
/// loading their libraries.
pub const ALL: bool = cfg!(feature = "all");
