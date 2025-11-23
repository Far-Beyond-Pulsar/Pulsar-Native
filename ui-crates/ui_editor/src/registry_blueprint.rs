//! Registry integration for Blueprint Editor

use engine_fs::registry::{AssetType, EditorType, AssetCategory};

/// Editor type registration for Blueprint Editor
#[derive(Clone)]
pub struct BlueprintEditorType;

impl EditorType for BlueprintEditorType {
    fn editor_id(&self) -> &'static str {
        "blueprint_editor"
    }
    
    fn display_name(&self) -> &'static str {
        "Blueprint Editor"
    }
    
    fn icon(&self) -> &'static str {
        "🔷"
    }
    
    fn clone_box(&self) -> Box<dyn EditorType> {
        Box::new(self.clone())
    }
}

/// Asset type for Blueprint Class files
#[derive(Clone)]
pub struct BlueprintClassAssetType;

impl AssetType for BlueprintClassAssetType {
    fn type_id(&self) -> &'static str {
        "blueprint_class"
    }
    
    fn display_name(&self) -> &'static str {
        "Blueprint Class"
    }
    
    fn icon(&self) -> &'static str {
        "🔷"
    }
    
    fn description(&self) -> &'static str {
        "Visual scripting blueprint class"
    }
    
    fn extensions(&self) -> &[&'static str] {
        &["bpc.json"]
    }
    
    fn default_directory(&self) -> &'static str {
        "blueprints/classes"
    }
    
    fn category(&self) -> AssetCategory {
        AssetCategory::Blueprints
    }
    
    fn generate_template(&self, name: &str) -> String {
        serde_json::json!({
            "name": name,
            "nodes": [],
            "connections": []
        }).to_string()
    }
    
    fn editor_id(&self) -> &'static str {
        "blueprint_editor"
    }
}

/// Asset type for Blueprint Function files
#[derive(Clone)]
pub struct BlueprintFunctionAssetType;

impl AssetType for BlueprintFunctionAssetType {
    fn type_id(&self) -> &'static str {
        "blueprint_function"
    }
    
    fn display_name(&self) -> &'static str {
        "Blueprint Function"
    }
    
    fn icon(&self) -> &'static str {
        "🔹"
    }
    
    fn description(&self) -> &'static str {
        "Visual scripting blueprint function"
    }
    
    fn extensions(&self) -> &[&'static str] {
        &["bpf.json"]
    }
    
    fn default_directory(&self) -> &'static str {
        "blueprints/functions"
    }
    
    fn category(&self) -> AssetCategory {
        AssetCategory::Blueprints
    }
    
    fn generate_template(&self, name: &str) -> String {
        serde_json::json!({
            "name": name,
            "nodes": [],
            "connections": []
        }).to_string()
    }
    
    fn editor_id(&self) -> &'static str {
        "blueprint_editor"
    }
}
