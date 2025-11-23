//! Registry integration for Level Editor

use engine_fs::registry::{AssetType, EditorType, AssetCategory};

/// Editor type registration for Level Editor
#[derive(Clone)]
pub struct LevelEditorType;

impl EditorType for LevelEditorType {
    fn editor_id(&self) -> &'static str {
        "level_editor"
    }
    
    fn display_name(&self) -> &'static str {
        "Level Editor"
    }
    
    fn icon(&self) -> &'static str {
        "🗺️"
    }
    
    fn clone_box(&self) -> Box<dyn EditorType> {
        Box::new(self.clone())
    }
}

/// Asset type for Level files
#[derive(Clone)]
pub struct LevelAssetType;

impl AssetType for LevelAssetType {
    fn type_id(&self) -> &'static str {
        "level"
    }
    
    fn display_name(&self) -> &'static str {
        "Level"
    }
    
    fn icon(&self) -> &'static str {
        "🗺️"
    }
    
    fn description(&self) -> &'static str {
        "3D level scene file"
    }
    
    fn extensions(&self) -> &[&'static str] {
        &["level.json"]
    }
    
    fn default_directory(&self) -> &'static str {
        "levels"
    }
    
    fn category(&self) -> AssetCategory {
        AssetCategory::Scenes
    }
    
    fn generate_template(&self, name: &str) -> String {
        serde_json::json!({
            "name": name,
            "entities": [],
            "skybox": null
        }).to_string()
    }
    
    fn editor_id(&self) -> &'static str {
        "level_editor"
    }
}
