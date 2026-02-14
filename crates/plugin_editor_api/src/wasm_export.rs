//! WASM plugin macro for registering plugins
//!
//! This provides a simple macro for plugin authors to register their plugin
//! when compiling to WASM.

/// Register a plugin with the WASM runtime
///
/// # Example
/// ```rust,ignore
/// use plugin_editor_api::*;
///
/// struct MyEditorPlugin;
///
/// impl EditorPlugin for MyEditorPlugin {
///     fn metadata(&self) -> PluginMetadata { ... }
///     // implement other trait methods
/// }
///
/// // For WASM builds
/// #[cfg(target_family = "wasm")]
/// register_plugin!(MyEditorPlugin);
/// ```
#[macro_export]
macro_rules! register_plugin {
    ($plugin_type:ty) => {
        // This macro is a placeholder for now.
        // In a full WASM implementation, this would:
        // 1. Use wit-bindgen to generate proper WIT exports
        // 2. Bridge between EditorPlugin trait and WIT interface
        // 3. Handle serialization/deserialization of complex types
        //
        // For the Blueprint plugin specifically, WASM compilation
        // is blocked by native dependencies (gpui graphics, etc).
        // The architecture requires separating:
        // - Plugin logic (can run in WASM)
        // - UI rendering (must stay on host)
        //
        // This is how Zed's extension system works - extensions
        // run in WASM and communicate with host for UI operations.
        
        compile_error!(
            "Full WASM plugin support requires architecture changes. \
            The Blueprint plugin has native UI dependencies (gpui) that \
            cannot compile to WASM. Solution: Separate plugin logic from \
            UI rendering, similar to Zed's extension system where extensions \
            run in WASM and send UI commands to the host."
        );
    };
}
