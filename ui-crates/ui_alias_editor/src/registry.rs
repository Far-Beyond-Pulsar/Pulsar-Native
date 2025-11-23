//! Registry integration for Type Alias Editor

use engine_fs::registry::{AssetType, EditorType, AssetCategory};

/// Editor type registration for Type Alias Editor
#[derive(Clone)]
pub struct TypeAliasEditorType;

impl EditorType for TypeAliasEditorType {
    fn editor_id(&self) -> &'static str {
        "type_alias_editor"
    }
    
    fn display_name(&self) -> &'static str {
        "Type Alias Editor"
    }
    
    fn icon(&self) -> &'static str {
        "📝"
    }
    
    fn clone_box(&self) -> Box<dyn EditorType> {
        Box::new(self.clone())
    }
}

/// Asset type registration for Type Alias files
#[derive(Clone)]
pub struct TypeAliasAssetType;

impl AssetType for TypeAliasAssetType {
    fn type_id(&self) -> &'static str {
        "type_alias"
    }
    
    fn display_name(&self) -> &'static str {
        "Type Alias"
    }
    
    fn icon(&self) -> &'static str {
        "📝"
    }
    
    fn description(&self) -> &'static str {
        "Rust type alias definition"
    }
    
    fn extensions(&self) -> &[&'static str] {
        &["alias.json"]
    }
    
    fn default_directory(&self) -> &'static str {
        "types/aliases"
    }
    
    fn category(&self) -> AssetCategory {
        AssetCategory::TypeSystem
    }
    
    fn generate_template(&self, name: &str) -> String {
        serde_json::json!({
            "name": name,
            "type": "String",
            "description": ""
        }).to_string()
    }
    
    fn editor_id(&self) -> &'static str {
        "type_alias_editor"
    }
}
