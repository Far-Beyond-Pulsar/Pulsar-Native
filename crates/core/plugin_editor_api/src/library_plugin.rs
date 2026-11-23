//! A plugin root built as a library of its own.
//!
//! A plugin's root crate registers what the plugin contributes through its
//! `build` (`pulsar_plugins::installed!`). Built with its `dll` feature, the
//! root is also a native library the editor loads from `plugins/editor`:
//! the plugin's own repository builds and releases it (`cargo build -p
//! <root> --release --features dll`), and `tools/plugin_builder` builds every
//! installed plugin that way together with the editor.
//!
//! The library carries two things the loader reads:
//!
//! - the root's descriptor and registrations, exported as data
//!   ([`export_plugin_build!`]: `pulsar_plugins::export_library!`), and its
//!   copy of Helio attached to the host's before any of its code runs
//!   (`helio_core::runtime`);
//! - an editor plugin entry point (`export_plugin!`), which the loader
//!   requires of every library. A root that only registers through its `build`
//!   exports an empty one ([`export_library_plugin!`]); a root with an editor
//!   plugin of its own exports that (`export_plugin!`) and adds
//!   [`export_plugin_build!`] beside it.
//!
//! A root writes, at the end of its `lib.rs`:
//!
//! ```ignore
//! #[cfg(feature = "dll")]
//! plugin_editor_api::export_library_plugin!();
//! ```
//!
//! and in its `Cargo.toml` the library crate type and the feature:
//!
//! ```toml
//! [lib]
//! crate-type = ["cdylib", "rlib"]
//! [features]
//! dll = ["dep:plugin_editor_api", "plugin_editor_api/editor"]
//! ```
//!
//! Without `dll` (a game, the installed set) the root is an ordinary library
//! and exports no symbol, so several roots can link into one binary.

/// Exports the root's descriptor and registrations as data, and the entry
/// point that attaches this library's Helio to the host's. Use it beside an
/// `export_plugin!` of the root's own, or through [`export_library_plugin!`].
#[doc(hidden)]
#[cfg(feature = "shared-dylibs")]
#[macro_export]
macro_rules! __link_shared_dylibs {
    () => {
        // A reference makes this library link the shared UI, reflection, SceneDB and
        // renderer libraries instead of embedding its own copy of them.
        #[allow(unused_imports)]
        use $crate::helio_dylib as _;
        #[allow(unused_imports)]
        use $crate::profiling_dylib as _;
        #[allow(unused_imports)]
        use $crate::reflection_dylib as _;
        #[allow(unused_imports)]
        use $crate::scenedb_dylib as _;
        #[allow(unused_imports)]
        use $crate::ui_dylib as _;
        #[allow(unused_imports)]
        use $crate::wgpui_dylib as _;
    };
}

#[doc(hidden)]
#[cfg(not(feature = "shared-dylibs"))]
#[macro_export]
macro_rules! __link_shared_dylibs {
    () => {};
}

#[macro_export]
macro_rules! export_plugin_build {
    () => {
        $crate::__link_shared_dylibs!();
        $crate::pulsar_plugins::export_library!();
        $crate::helio_core::export_runtime_attach!();
    };
}

/// Exports this library as one plugin root's build: an editor plugin with no
/// file types or editors of its own, named after the root's descriptor, and
/// [`export_plugin_build!`]. The root is linked into the same crate (it is the
/// crate), so its descriptor is the one linked.
#[macro_export]
macro_rules! export_library_plugin {
    () => {
        /// The library's editor plugin: nothing but what the loader requires.
        #[derive(Default)]
        pub struct LibraryPlugin;

        impl $crate::plugin::EditorPlugin for LibraryPlugin {
            fn metadata(&self) -> $crate::metadata::PluginMetadata {
                let descriptor = $crate::pulsar_plugins::library::linked_descriptor();
                let id = descriptor.as_ref().map_or("unknown", |d| d.id.as_str());
                let name = descriptor.as_ref().map_or("Plugin", |d| d.name.as_str());
                $crate::metadata::PluginMetadata {
                    id: $crate::identifiers::PluginId::new(id),
                    name: name.into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                    author: "Pulsar Team".into(),
                    description: format!("{name}, loaded as a library"),
                }
            }

            fn file_types(&self) -> Vec<$crate::file_types::FileTypeDefinition> {
                Vec::new()
            }

            fn editors(&self) -> Vec<$crate::metadata::EditorMetadata> {
                Vec::new()
            }
        }

        impl $crate::editor_element::EditorPluginEditor for LibraryPlugin {
            fn register_editors(
                &'static self,
                _registry: &mut $crate::editor_element::EditorFactoryRegistry,
            ) {
            }
        }

        impl $crate::statusbar::EditorPluginStatusbar for LibraryPlugin {}
        impl $crate::ai::EditorPluginAi for LibraryPlugin {}
        impl $crate::components::EditorPluginComponents for LibraryPlugin {
            fn component_definitions(&self) -> Vec<$crate::components::ComponentDefinition> {
                Vec::new()
            }
        }

        $crate::export_plugin!(LibraryPlugin);
        $crate::export_plugin_build!();
    };
}
