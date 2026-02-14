//! WASM plugin macro for registering plugins
//!
//! This provides a simple macro for plugin authors to register their plugin
//! when compiling to WASM.

/// Register a plugin with the WASM runtime
///
/// # Example
/// ```rust
/// use pulsar_plugin_api::*;
///
/// struct MyEditorPlugin {
///     // plugin state
/// }
///
/// impl EditorPlugin for MyEditorPlugin {
///     fn metadata(&self) -> PluginMetadata { ... }
///     // implement other trait methods
/// }
///
/// // Register the plugin
/// register_plugin!(MyEditorPlugin);
/// ```
#[macro_export]
macro_rules! register_plugin {
    ($plugin_type:ty) => {
        // WIT component exports
        #[cfg(target_family = "wasm")]
        mod __plugin_exports {
            use super::*;

            #[no_mangle]
            pub extern "C" fn metadata() -> $crate::wasm::PluginMetadataWit {
                let plugin = <$plugin_type>::default();
                let meta = <$plugin_type as $crate::EditorPlugin>::metadata(&plugin);
                $crate::wasm::PluginMetadataWit::from(meta)
            }

            #[no_mangle]
            pub extern "C" fn on_load() {
                let mut plugin = <$plugin_type>::default();
                <$plugin_type as $crate::EditorPlugin>::on_load(&mut plugin);
            }

            #[no_mangle]
            pub extern "C" fn on_unload() {
                let mut plugin = <$plugin_type>::default();
                <$plugin_type as $crate::EditorPlugin>::on_unload(&mut plugin);
            }

            // Additional export functions will be generated here
        }
    };
}

/// WIT-compatible structures
#[cfg(target_family = "wasm")]
pub mod wasm {
    use super::*;

    /// Plugin metadata in WIT format
    #[repr(C)]
    pub struct PluginMetadataWit {
        pub name: *const u8,
        pub name_len: usize,
        pub version: *const u8,
        pub version_len: usize,
        pub author: *const u8,
        pub author_len: usize,
        pub description: *const u8,
        pub description_len: usize,
        // TODO: Handle extensions list
    }

    impl From<crate::PluginMetadata> for PluginMetadataWit {
        fn from(meta: crate::PluginMetadata) -> Self {
            // This is a simplified version
            // Full implementation needs proper string marshaling
            todo!("Implement proper WIT marshaling")
        }
    }
}
