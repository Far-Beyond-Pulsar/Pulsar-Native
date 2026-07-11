use std::sync::Arc;

use crate::file_types::FileTypeDefinition;
use crate::identifiers::{EditorId, FileTypeId};
use crate::metadata::{EditorMetadata, PluginMetadata};
use crate::version::VersionInfo;

// ============================================================================
// Core Plugin Trait
// ============================================================================

/// Core trait that all editor plugins must implement.
///
/// Minimal required surface — only methods every plugin needs.  Optional
/// capabilities live in extension subtraits:
///
/// | Trait | Methods |
/// |-------|---------|
/// | [`EditorPluginEditor`](crate::editor_element::EditorPluginEditor) | `create_editor` |
/// | [`EditorPluginStatusbar`](crate::statusbar::EditorPluginStatusbar) | `statusbar_buttons`, `accepted_drop_kinds` |
/// | [`EditorPluginAi`](crate::ai::EditorPluginAi) | `ai_tools`, `execute_ai_tool`, `capabilities_for_file` |
/// | [`EditorPluginComponents`](crate::components::EditorPluginComponents) | `component_definitions`, `component_factories` |
/// | [`EditorPluginSubsystems`](crate::subsystems::EditorPluginSubsystems) | `subsystems` |
///
/// For DLL-loaded plugins the
/// [`export_plugin!`](crate::plugin::export_plugin) macro automatically
/// provides default (no-op) implementations for every extension trait.
pub trait EditorPlugin: Send + Sync {
    /// Get the version information for this plugin.
    fn version_info(&self) -> VersionInfo {
        VersionInfo::current()
    }

    /// Get metadata about this plugin.
    fn metadata(&self) -> PluginMetadata;

    /// Get all file types this plugin supports.
    fn file_types(&self) -> Vec<FileTypeDefinition>;

    /// Get all editor types this plugin provides.
    fn editors(&self) -> Vec<EditorMetadata>;

    /// Called when the plugin is loaded.  Override to perform
    /// one-time initialisation.
    fn on_load(&mut self) {}
}

// ============================================================================
// Combined Plugin Trait
// ============================================================================

/// Combined trait encompassing all optional plugin capabilities.
///
/// This is the trait that DLL-loaded plugins must implement. It combines
/// the base `EditorPlugin` with all optional capability traits, allowing
/// the plugin manager to query any capability through a single trait object.
pub trait EditorPluginFull:
    EditorPlugin
    + crate::editor_element::EditorPluginEditor
    + crate::statusbar::EditorPluginStatusbar
    + crate::ai::EditorPluginAi
    + crate::components::EditorPluginComponents
    + crate::subsystems::EditorPluginSubsystems
{
}

// ============================================================================
// Plugin Constructor
// ============================================================================

/// Type alias for the plugin constructor function.
///
/// Plugins must export a function with this signature named `_plugin_create`.
///
/// # Safety
///
/// The returned reference has `'static` lifetime because the plugin is never unloaded.
/// This is safe because the plugin code remains valid for the process lifetime.
pub type PluginCreate =
    unsafe extern "C" fn(theme_ptr: *const std::ffi::c_void) -> &'static mut dyn EditorPluginFull;

// ============================================================================
// Plugin Declaration and Export Macro
// ============================================================================

/// Macro to export a plugin from a dynamic library.
///
/// This generates the necessary FFI functions for the plugin to be loaded
/// by the engine.
///
/// # Safety
///
/// This macro generates `unsafe extern "C"` functions. The safety contract is:
///
/// 1. **Plugin is never unloaded**: The returned `&'static mut dyn EditorPlugin` reference
///    is valid forever because we never call dlclose/FreeLibrary.
///
/// 2. **Theme pointer validity**: The engine must ensure the Theme pointer remains valid.
///    This is guaranteed by the engine keeping Theme in stable storage.
///
/// 3. **Version checking**: Plugins must be built with compatible versions (checked at runtime).
///
/// # Example
///
/// ```rust,ignore
/// struct MyPlugin;
/// impl EditorPlugin for MyPlugin { /* ... */ }
///
/// export_plugin!(MyPlugin);
/// ```
#[macro_export]
macro_rules! export_plugin {
    ($plugin_type:ty) => {
        // Static storage for synced Theme data from main app
        static SYNCED_THEME: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

        // Wrapper that implements all plugin traits for DLL export.
        // This ensures the exported trait object has access to all methods.
        #[allow(non_camel_case_types)]
        struct __PluginExport($plugin_type);

        impl $crate::plugin::EditorPlugin for __PluginExport {
            fn version_info(&self) -> $crate::version::VersionInfo {
                $crate::plugin::EditorPlugin::version_info(&self.0)
            }
            fn metadata(&self) -> $crate::metadata::PluginMetadata {
                $crate::plugin::EditorPlugin::metadata(&self.0)
            }
            fn file_types(&self) -> Vec<$crate::file_types::FileTypeDefinition> {
                $crate::plugin::EditorPlugin::file_types(&self.0)
            }
            fn editors(&self) -> Vec<$crate::metadata::EditorMetadata> {
                $crate::plugin::EditorPlugin::editors(&self.0)
            }
            fn on_load(&mut self) {
                $crate::plugin::EditorPlugin::on_load(&mut self.0)
            }
        }

        impl $crate::editor_element::EditorPluginEditor for __PluginExport {
            fn register_editors(
                &'static self,
                registry: &mut $crate::editor_element::EditorFactoryRegistry,
            ) {
                $crate::editor_element::EditorPluginEditor::register_editors(&self.0, registry)
            }
        }

        // ── Optional capabilities — delegate to inner plugin type ────────────
        //
        // Each impl block requires the inner type `$plugin_type` to implement
        // the corresponding trait.  Plugin authors add empty `impl` blocks
        // for traits they don't need custom behaviour from.
        //
        // Example:
        //   impl EditorPluginStatusbar for MyPlugin {}
        //   impl EditorPluginAi for MyPlugin {}

        impl $crate::statusbar::EditorPluginStatusbar for __PluginExport {
            fn statusbar_buttons(&self) -> Vec<$crate::statusbar::StatusbarButtonDefinition> {
                $crate::statusbar::EditorPluginStatusbar::statusbar_buttons(&self.0)
            }
            fn accepted_drop_kinds(&self) -> Vec<$crate::AssetKind> {
                $crate::statusbar::EditorPluginStatusbar::accepted_drop_kinds(&self.0)
            }
        }

        impl $crate::ai::EditorPluginAi for __PluginExport {
            fn ai_tools(&self) -> Vec<$crate::ai::AiToolDefinition> {
                $crate::ai::EditorPluginAi::ai_tools(&self.0)
            }
            fn execute_ai_tool(
                &self,
                file_path: &std::path::Path,
                tool_name: &str,
                tool_args: $crate::JsonValue,
            ) -> std::result::Result<$crate::JsonValue, $crate::error::PluginError> {
                $crate::ai::EditorPluginAi::execute_ai_tool(
                    &self.0, file_path, tool_name, tool_args,
                )
            }
            fn capabilities_for_file(&self, file_path: &std::path::Path) -> Vec<String> {
                $crate::ai::EditorPluginAi::capabilities_for_file(&self.0, file_path)
            }
        }

        impl $crate::components::EditorPluginComponents for __PluginExport {
            fn component_definitions(&self) -> Vec<$crate::components::ComponentDefinition> {
                $crate::components::EditorPluginComponents::component_definitions(&self.0)
            }
            fn component_factories(&self) -> Vec<(String, $crate::components::ComponentFactory)> {
                $crate::components::EditorPluginComponents::component_factories(&self.0)
            }
        }

        impl $crate::subsystems::EditorPluginSubsystems for __PluginExport {
            fn subsystems(&self) -> Vec<Box<dyn $crate::subsystems::Subsystem>> {
                $crate::subsystems::EditorPluginSubsystems::subsystems(&self.0)
            }
        }

        impl $crate::plugin::EditorPluginFull for __PluginExport {}

        /// Create the plugin instance.
        ///
        /// # Safety
        ///
        /// This function returns a `&'static mut dyn EditorPluginFull` which is safe because:
        /// 1. The plugin is never unloaded (PermanentLibrary prevents dlclose/FreeLibrary)
        /// 2. The returned reference is leaked intentionally
        /// 3. All plugin code remains valid for process lifetime
        ///
        /// The theme_ptr must point to valid Theme data that remains stable for the process
        /// lifetime. This is the caller's (engine's) responsibility.
        #[no_mangle]
        pub unsafe extern "C" fn _plugin_create(
            theme_ptr: *const std::ffi::c_void,
        ) -> &'static mut dyn $crate::plugin::EditorPluginFull {
            // Validate theme pointer
            if theme_ptr.is_null() {
                panic!("[Plugin] ERROR: Received null theme pointer from host!");
            }

            // Store theme pointer (fail if already set)
            if SYNCED_THEME.set(theme_ptr as usize).is_err() {
                panic!("[Plugin] ERROR: Theme pointer already initialized!");
            }

            // Register our theme accessor with the ui crate
            ui::theme::Theme::register_plugin_accessor(plugin_theme_unsafe);

            // Create the plugin instance wrapped for full trait access
            let plugin = <$plugin_type>::default();
            let wrapper = __PluginExport(plugin);
            let boxed: Box<dyn $crate::plugin::EditorPluginFull> = Box::new(wrapper);

            // Leak the box to get a 'static reference
            Box::leak(boxed)
        }

        /// Internal accessor for plugin theme (called by ui crate).
        ///
        /// # Safety
        ///
        /// SAFETY: Returns None if theme pointer is null or not initialized.
        /// The caller (ui crate) must handle None gracefully.
        unsafe fn plugin_theme_unsafe() -> Option<&'static ui::theme::Theme> {
            let ptr = SYNCED_THEME.get().copied()? as *const std::ffi::c_void;

            if ptr.is_null() {
                return None;
            }

            // SAFETY: The engine contract guarantees this pointer remains valid.
            Some(&*(ptr as *const ui::theme::Theme))
        }

        /// Get the plugin's version information.
        #[no_mangle]
        pub extern "C" fn _plugin_version() -> $crate::version::VersionInfo {
            $crate::version::VersionInfo::current()
        }

        /// Initialize the plugin's globals from the main app.
        #[no_mangle]
        pub unsafe extern "C" fn _plugin_init_globals(theme_ptr: *const std::ffi::c_void) {
            if theme_ptr.is_null() {
                tracing::error!("[Plugin] ERROR: Received null theme pointer in init_globals!");
                return;
            }

            if SYNCED_THEME.get().is_none() {
                SYNCED_THEME.set(theme_ptr as usize);
            }
        }
    };
}
