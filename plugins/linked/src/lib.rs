//! The plugins the editor links into its binary: `render_bps`, the render
//! graph's own plugin, and nothing else.
//!
//! Every other installed plugin is its own library, built by
//! `tools/plugin_builder` (one generated crate per plugin under
//! `plugins/dll/`) and loaded from `plugins/editor` at startup, so a plugin's
//! memory is its library's and its leaks can be told apart from the
//! editor's. `plugins/installed` still links every plugin for games and
//! tests, which do not load libraries.

pub use render_bps_plugin;
