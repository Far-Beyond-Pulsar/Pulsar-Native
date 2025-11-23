//! Registry integration for DAW Editor

use engine_fs::registry::{AssetType, EditorType, AssetCategory};

/// Editor type registration for DAW Editor
#[derive(Clone)]
pub struct DawEditorType;

impl EditorType for DawEditorType {
    fn editor_id(&self) -> &'static str {
        "daw_editor"
    }
    
    fn display_name(&self) -> &'static str {
        "DAW Editor"
    }
    
    fn icon(&self) -> &'static str {
        "🎵"
    }
    
    fn clone_box(&self) -> Box<dyn EditorType> {
        Box::new(self.clone())
    }
}

/// Asset type for DAW Project files
#[derive(Clone)]
pub struct DawProjectAssetType;

impl AssetType for DawProjectAssetType {
    fn type_id(&self) -> &'static str {
        "daw_project"
    }
    
    fn display_name(&self) -> &'static str {
        "DAW Project"
    }
    
    fn icon(&self) -> &'static str {
        "🎵"
    }
    
    fn description(&self) -> &'static str {
        "Digital audio workstation project"
    }
    
    fn extensions(&self) -> &[&'static str] {
        &["daw.json"]
    }
    
    fn default_directory(&self) -> &'static str {
        "audio/projects"
    }
    
    fn category(&self) -> AssetCategory {
        AssetCategory::Audio
    }
    
    fn generate_template(&self, name: &str) -> String {
        serde_json::json!({
            "name": name,
            "tracks": [],
            "tempo": 120,
            "time_signature": "4/4"
        }).to_string()
    }
    
    fn editor_id(&self) -> &'static str {
        "daw_editor"
    }
}
