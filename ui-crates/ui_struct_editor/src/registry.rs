//! Registry integration for Struct Editor

use engine_fs::registry::{AssetType, EditorType, AssetCategory};

/// Editor type registration for Struct Editor
#[derive(Clone)]
pub struct StructEditorType;

impl EditorType for StructEditorType {
    fn editor_id(&self) -> &'static str {
        "struct_editor"
    }
    
    fn display_name(&self) -> &'static str {
        "Struct Editor"
    }
    
    fn icon(&self) -> &'static str {
        "📦"
    }
    
    fn clone_box(&self) -> Box<dyn EditorType> {
        Box::new(self.clone())
    }
}

/// Asset type registration for Struct files
#[derive(Clone)]
pub struct StructAssetType;

impl AssetType for StructAssetType {
    fn type_id(&self) -> &'static str {
        "struct"
    }
    
    fn display_name(&self) -> &'static str {
        "Struct"
    }
    
    fn icon(&self) -> &'static str {
        "📦"
    }
    
    fn description(&self) -> &'static str {
        "Rust struct definition"
    }
    
    fn extensions(&self) -> &[&'static str] {
        &["struct.json"]
    }
    
    fn default_directory(&self) -> &'static str {
        "types/structs"
    }
    
    fn category(&self) -> AssetCategory {
        AssetCategory::TypeSystem
    }
    
    fn generate_template(&self, name: &str) -> String {
        serde_json::json!({
            "name": name,
            "fields": [],
            "description": ""
        }).to_string()
    }
    
    fn editor_id(&self) -> &'static str {
        "struct_editor"
    }
}
