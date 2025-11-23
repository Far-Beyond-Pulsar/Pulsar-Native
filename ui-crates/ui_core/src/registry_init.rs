//! Registry initialization - Register all asset types
//! 
//! This is the ONLY file that references concrete editor/asset type definitions.
//! All other code uses the trait-based registry system.

use engine_fs::registry::global_registry;
use std::sync::Arc;

// Import asset type and editor type definitions from each editor crate
use ui_alias_editor::{TypeAliasAssetType, TypeAliasEditorType};
use ui_editor::{
    BlueprintEditorType, BlueprintClassAssetType, BlueprintFunctionAssetType,
    ScriptEditorType, RustScriptAssetType, LuaScriptAssetType, ShaderAssetType,
    DawEditorType, DawProjectAssetType,
    LevelEditorType, LevelAssetType,
};
use ui_struct_editor::{StructEditorType, StructAssetType};
use ui_enum_editor::{EnumEditorType, EnumAssetType};
use ui_trait_editor::{TraitEditorType, TraitAssetType};
use ui_editor_table::{TableEditorType, DatabaseAssetType};

/// Register all asset types and editor types with the global registry
/// This should be called once during app initialization
/// 
/// Note: This only registers metadata. The actual editor opening functions
/// are registered separately in PulsarApp::new_internal() because they need
/// access to the app instance.
pub fn register_all_asset_types() {
    let registry = global_registry();
    
    // ==================== BLUEPRINT ====================
    registry.register_editor(Arc::new(BlueprintEditorType));
    registry.register_asset_type(Arc::new(BlueprintClassAssetType));
    registry.register_asset_type(Arc::new(BlueprintFunctionAssetType));
    
    // ==================== SCRIPT ====================
    registry.register_editor(Arc::new(ScriptEditorType));
    registry.register_asset_type(Arc::new(RustScriptAssetType));
    registry.register_asset_type(Arc::new(LuaScriptAssetType));
    registry.register_asset_type(Arc::new(ShaderAssetType));
    
    // ==================== TYPE SYSTEM ====================
    registry.register_editor(Arc::new(TypeAliasEditorType));
    registry.register_asset_type(Arc::new(TypeAliasAssetType));
    
    registry.register_editor(Arc::new(StructEditorType));
    registry.register_asset_type(Arc::new(StructAssetType));
    
    registry.register_editor(Arc::new(EnumEditorType));
    registry.register_asset_type(Arc::new(EnumAssetType));
    
    registry.register_editor(Arc::new(TraitEditorType));
    registry.register_asset_type(Arc::new(TraitAssetType));
    
    // ==================== DAW ====================
    registry.register_editor(Arc::new(DawEditorType));
    registry.register_asset_type(Arc::new(DawProjectAssetType));
    
    // ==================== LEVEL ====================
    registry.register_editor(Arc::new(LevelEditorType));
    registry.register_asset_type(Arc::new(LevelAssetType));
    
    // ==================== TABLE/DATABASE ====================
    registry.register_editor(Arc::new(TableEditorType));
    registry.register_asset_type(Arc::new(DatabaseAssetType));
}
