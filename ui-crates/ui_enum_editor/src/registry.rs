//! Registry integration for Enum Editor

use engine_fs::registry::{AssetType, EditorType, AssetCategory};

/// Editor type registration for Enum Editor
#[derive(Clone)]
pub struct EnumEditorType;

impl EditorType for EnumEditorType {
    fn editor_id(&self) -> &'static str {
        "enum_editor"
    }
    
    fn display_name(&self) -> &'static str {
        "Enum Editor"
    }
    
    fn icon(&self) -> &'static str {
        "🔢"
    }
    
    fn clone_box(&self) -> Box<dyn EditorType> {
        Box::new(self.clone())
    }
}

/// Asset type registration for Enum files
#[derive(Clone)]
pub struct EnumAssetType;

impl AssetType for EnumAssetType {
    fn type_id(&self) -> &'static str {
        "enum"
    }
    
    fn display_name(&self) -> &'static str {
        "Enum"
    }
    
    fn icon(&self) -> &'static str {
        "🔢"
    }
    
    fn description(&self) -> &'static str {
        "Rust enum definition"
    }
    
    fn extensions(&self) -> &[&'static str] {
        &["enum.json"]
    }
    
    fn default_directory(&self) -> &'static str {
        "types/enums"
    }
    
    fn category(&self) -> AssetCategory {
        AssetCategory::TypeSystem
    }
    
    fn generate_template(&self, name: &str) -> String {
        serde_json::json!({
            "name": name,
            "variants": [],
            "description": ""
        }).to_string()
    }
    
    fn editor_id(&self) -> &'static str {
        "enum_editor"
    }
}
