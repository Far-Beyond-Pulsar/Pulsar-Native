//! Registry integration for Trait Editor

use engine_fs::registry::{AssetType, EditorType, AssetCategory};

/// Editor type registration for Trait Editor
#[derive(Clone)]
pub struct TraitEditorType;

impl EditorType for TraitEditorType {
    fn editor_id(&self) -> &'static str {
        "trait_editor"
    }
    
    fn display_name(&self) -> &'static str {
        "Trait Editor"
    }
    
    fn icon(&self) -> &'static str {
        "🎭"
    }
    
    fn clone_box(&self) -> Box<dyn EditorType> {
        Box::new(self.clone())
    }
}

/// Asset type registration for Trait files
#[derive(Clone)]
pub struct TraitAssetType;

impl AssetType for TraitAssetType {
    fn type_id(&self) -> &'static str {
        "trait"
    }
    
    fn display_name(&self) -> &'static str {
        "Trait"
    }
    
    fn icon(&self) -> &'static str {
        "🎭"
    }
    
    fn description(&self) -> &'static str {
        "Rust trait definition"
    }
    
    fn extensions(&self) -> &[&'static str] {
        &["trait.json"]
    }
    
    fn default_directory(&self) -> &'static str {
        "types/traits"
    }
    
    fn category(&self) -> AssetCategory {
        AssetCategory::TypeSystem
    }
    
    fn generate_template(&self, name: &str) -> String {
        serde_json::json!({
            "name": name,
            "methods": [],
            "description": ""
        }).to_string()
    }
    
    fn editor_id(&self) -> &'static str {
        "trait_editor"
    }
}
