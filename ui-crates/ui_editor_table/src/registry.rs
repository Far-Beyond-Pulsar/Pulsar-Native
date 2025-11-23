//! Registry integration for Table/Database Editor

use engine_fs::registry::{AssetType, EditorType, AssetCategory};

/// Editor type registration for Table Editor
#[derive(Clone)]
pub struct TableEditorType;

impl EditorType for TableEditorType {
    fn editor_id(&self) -> &'static str {
        "table_editor"
    }
    
    fn display_name(&self) -> &'static str {
        "Table Editor"
    }
    
    fn icon(&self) -> &'static str {
        "🗄️"
    }
    
    fn clone_box(&self) -> Box<dyn EditorType> {
        Box::new(self.clone())
    }
}

/// Asset type registration for Database files
#[derive(Clone)]
pub struct DatabaseAssetType;

impl AssetType for DatabaseAssetType {
    fn type_id(&self) -> &'static str {
        "database"
    }
    
    fn display_name(&self) -> &'static str {
        "Database"
    }
    
    fn icon(&self) -> &'static str {
        "🗄️"
    }
    
    fn description(&self) -> &'static str {
        "SQLite database file"
    }
    
    fn extensions(&self) -> &[&'static str] {
        &[".db", ".sqlite", ".sqlite3"]
    }
    
    fn default_directory(&self) -> &'static str {
        "data/databases"
    }
    
    fn category(&self) -> AssetCategory {
        AssetCategory::Data
    }
    
    fn generate_template(&self, _name: &str) -> String {
        // Database files are binary, so we can't generate a text template
        // This will be handled specially by the file system
        String::new()
    }
    
    fn editor_id(&self) -> &'static str {
        "table_editor"
    }
}
