//! A plugin library that is only a plugin root's build.
//!
//! The loader requires every library to carry a plugin entry point
//! (`export_plugin!`). A plugin whose root only registers through its `build`
//! (every object kind, language and tool that adds editors through
//! registrations) has no editor plugin of its own to export, so
//! [`export_library_plugin!`] exports an empty one beside the root's build:
//! the library's only content is what the root registers, imported through
//! `pulsar_plugins::export_library!`. `tools/plugin_builder` generates one
//! such crate per plugin, so each plugin is its own DLL.

/// Exports this library as one plugin root's build: an editor plugin with no
/// file types or editors of its own (`$id` and `$name` are the plugin's
/// descriptor id and name), and the root's descriptor and registrations
/// (`pulsar_plugins::export_library!`). Link the root in the same crate
/// (`use my_plugin as _;`).
#[macro_export]
macro_rules! export_library_plugin {
    ($id:literal, $name:literal) => {
        /// The library's editor plugin: nothing but what the loader requires.
        #[derive(Default)]
        pub struct LibraryPlugin;

        impl $crate::plugin::EditorPlugin for LibraryPlugin {
            fn metadata(&self) -> $crate::metadata::PluginMetadata {
                $crate::metadata::PluginMetadata {
                    id: $crate::identifiers::PluginId::new($id),
                    name: $name.into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                    author: "Pulsar Team".into(),
                    description: concat!($name, ", loaded as a library").into(),
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
        $crate::pulsar_plugins::export_library!();
    };
}
