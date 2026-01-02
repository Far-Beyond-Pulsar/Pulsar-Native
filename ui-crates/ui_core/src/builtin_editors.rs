//! Central registration for all built-in editors.
//!
//! This module provides a single function to register all built-in editors
//! with the plugin manager's registries.

use plugin_manager::BuiltinEditorRegistry;
use std::sync::Arc;

/// Register all built-in editors with the registry.
///
/// This should be called during application initialization,
/// before any files are opened.
pub fn register_all_builtin_editors(registry: &mut BuiltinEditorRegistry) {
    tracing::info!("Registering all built-in editors...");
    
    // Type system editors
    // TODO: These will eventually be moved to plugins
    registry.register_provider(Arc::new(ui_struct_editor::StructEditorProvider));
    registry.register_provider(Arc::new(ui_enum_editor::EnumEditorProvider));
    registry.register_provider(Arc::new(ui_trait_editor::TraitEditorProvider));
    registry.register_provider(Arc::new(ui_alias_editor::AliasEditorProvider));
    
    // Code editors
    registry.register_provider(Arc::new(ui_script_editor::ScriptEditorProvider));
    
    // Specialized editors
    registry.register_provider(Arc::new(ui_daw_editor::DawEditorProvider));
    registry.register_provider(Arc::new(ui_editor_table::TableEditorProvider));
    
    tracing::info!("Built-in editor registration complete");
}
