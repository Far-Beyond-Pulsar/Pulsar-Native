//! Asset and Editor Registry System
//!
//! Clean trait-based system for registering file types and their editors
//! All file opening and editor creation goes through this registry

use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use anyhow::{Result, anyhow};

/// Trait for asset types that can be created, opened, and edited
pub trait AssetType: Send + Sync {
    /// Unique identifier for this asset type (e.g., "type_alias", "blueprint_class")
    fn type_id(&self) -> &'static str;
    
    /// Display name shown in UI
    fn display_name(&self) -> &'static str;
    
    /// Icon emoji or name
    fn icon(&self) -> &'static str;
    
    /// Description shown in tooltips
    fn description(&self) -> &'static str;
    
    /// File extensions this type handles (e.g., ["alias.json"] or ["rs"])
    /// First extension is used for new files
    fn extensions(&self) -> &[&'static str];
    
    /// Default directory for new files of this type (relative to project root)
    fn default_directory(&self) -> &'static str;
    
    /// Category for grouping in UI
    fn category(&self) -> AssetCategory;
    
    /// Generate a blank template file content
    fn generate_template(&self, name: &str) -> String;
    
    /// ID of the editor that should handle this asset type
    fn editor_id(&self) -> &'static str;
    
    /// Validate a file can be opened as this type
    fn can_open(&self, path: &Path) -> bool {
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy();
            self.extensions().iter().any(|e| ext_str.ends_with(e))
        } else {
            false
        }
    }
    
    /// Get full file name with extension
    fn file_name(&self, name: &str) -> String {
        let ext = self.extensions().first().unwrap_or(&"");
        if ext.is_empty() {
            name.to_string()
        } else {
            format!("{}.{}", name, ext)
        }
    }
}

/// Trait for editors that can open and edit asset types
/// Each editor implementation registers itself with the registry
pub trait EditorType: Send + Sync {
    /// Unique identifier for this editor (e.g., "type_alias_editor", "blueprint_editor")
    fn editor_id(&self) -> &'static str;
    
    /// Display name for the editor
    fn display_name(&self) -> &'static str;
    
    /// Icon for editor tabs
    fn icon(&self) -> &'static str;
    
    /// Clone this editor type definition
    fn clone_box(&self) -> Box<dyn EditorType>;
}

impl Clone for Box<dyn EditorType> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// Asset categories for UI organization
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetCategory {
    TypeSystem,
    Blueprints,
    Scripts,
    Scenes,
    Rendering,
    Audio,
    UI,
    Data,
    Config,
}

impl AssetCategory {
    pub fn display_name(&self) -> &'static str {
        match self {
            AssetCategory::TypeSystem => "Type System",
            AssetCategory::Blueprints => "Blueprints",
            AssetCategory::Scripts => "Scripts",
            AssetCategory::Scenes => "Scenes",
            AssetCategory::Rendering => "Rendering",
            AssetCategory::Audio => "Audio",
            AssetCategory::UI => "User Interface",
            AssetCategory::Data => "Data",
            AssetCategory::Config => "Configuration",
        }
    }
    
    pub fn icon(&self) -> &'static str {
        match self {
            AssetCategory::TypeSystem => "📝",
            AssetCategory::Blueprints => "🔷",
            AssetCategory::Scripts => "📜",
            AssetCategory::Scenes => "🎬",
            AssetCategory::Rendering => "🎨",
            AssetCategory::Audio => "🔊",
            AssetCategory::UI => "🖥️",
            AssetCategory::Data => "📊",
            AssetCategory::Config => "⚙️",
        }
    }
    
    pub fn all() -> Vec<AssetCategory> {
        vec![
            AssetCategory::TypeSystem,
            AssetCategory::Blueprints,
            AssetCategory::Scripts,
            AssetCategory::Scenes,
            AssetCategory::Rendering,
            AssetCategory::Audio,
            AssetCategory::UI,
            AssetCategory::Data,
            AssetCategory::Config,
        ]
    }
}

/// Type for editor factory functions
/// These are registered by the UI layer and handle actual editor instantiation
pub type EditorFactoryFn = dyn Fn(PathBuf) -> bool + Send + Sync;

/// Central registry for all asset types and editors
pub struct AssetRegistry {
    asset_types: RwLock<HashMap<String, Arc<dyn AssetType>>>,
    editors: RwLock<HashMap<String, Arc<dyn EditorType>>>,
    editor_factories: RwLock<HashMap<String, Arc<EditorFactoryFn>>>, // editor_id -> factory
    extension_map: RwLock<HashMap<String, Vec<String>>>, // ext -> type_ids
}

impl AssetRegistry {
    pub fn new() -> Self {
        Self {
            asset_types: RwLock::new(HashMap::new()),
            editors: RwLock::new(HashMap::new()),
            editor_factories: RwLock::new(HashMap::new()),
            extension_map: RwLock::new(HashMap::new()),
        }
    }
    
    /// Register an asset type
    pub fn register_asset_type(&self, asset_type: Arc<dyn AssetType>) {
        let type_id = asset_type.type_id().to_string();
        
        // Register in main map
        self.asset_types.write().unwrap().insert(type_id.clone(), asset_type.clone());
        
        // Register extensions
        let mut ext_map = self.extension_map.write().unwrap();
        for ext in asset_type.extensions() {
            ext_map.entry(ext.to_string())
                .or_insert_with(Vec::new)
                .push(type_id.clone());
        }
    }
    
    /// Register an editor type
    pub fn register_editor(&self, editor: Arc<dyn EditorType>) {
        let editor_id = editor.editor_id().to_string();
        self.editors.write().unwrap().insert(editor_id, editor);
    }
    
    /// Register an editor factory function (UI layer)
    /// Returns true if file was opened successfully, false otherwise
    pub fn register_editor_factory(&self, editor_id: &str, factory: Arc<EditorFactoryFn>) {
        self.editor_factories.write().unwrap().insert(editor_id.to_string(), factory);
    }
    
    /// Get asset type by ID
    pub fn get_asset_type(&self, type_id: &str) -> Option<Arc<dyn AssetType>> {
        self.asset_types.read().unwrap().get(type_id).cloned()
    }
    
    /// Get editor by ID
    pub fn get_editor(&self, editor_id: &str) -> Option<Arc<dyn EditorType>> {
        self.editors.read().unwrap().get(editor_id).cloned()
    }
    
    /// Find asset type that can handle a file
    pub fn find_asset_type_for_file(&self, path: &Path) -> Option<Arc<dyn AssetType>> {
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_string();
            if let Some(type_ids) = self.extension_map.read().unwrap().get(&ext_str) {
                for type_id in type_ids {
                    if let Some(asset_type) = self.get_asset_type(type_id) {
                        if asset_type.can_open(path) {
                            return Some(asset_type);
                        }
                    }
                }
            }
        }
        None
    }
    
    /// Find editor for a file
    pub fn find_editor_for_file(&self, path: &Path) -> Option<Arc<dyn EditorType>> {
        if let Some(asset_type) = self.find_asset_type_for_file(path) {
            self.get_editor(asset_type.editor_id())
        } else {
            None
        }
    }
    
    /// Open a file using the appropriate registered editor factory
    /// Returns true if opened successfully, false if no editor found
    pub fn open_file(&self, path: PathBuf) -> bool {
        if let Some(asset_type) = self.find_asset_type_for_file(&path) {
            let editor_id = asset_type.editor_id();
            if let Some(factory) = self.editor_factories.read().unwrap().get(editor_id) {
                return factory(path);
            }
        }
        false
    }
    
    /// Get all asset types
    pub fn get_all_asset_types(&self) -> Vec<Arc<dyn AssetType>> {
        self.asset_types.read().unwrap().values().cloned().collect()
    }
    
    /// Get asset types by category
    pub fn get_asset_types_by_category(&self, category: AssetCategory) -> Vec<Arc<dyn AssetType>> {
        self.asset_types.read().unwrap()
            .values()
            .filter(|t| t.category() == category)
            .cloned()
            .collect()
    }
    
    /// Get all editors
    pub fn get_all_editors(&self) -> Vec<Arc<dyn EditorType>> {
        self.editors.read().unwrap().values().cloned().collect()
    }
    
    /// Create a new file of a specific type
    pub fn create_new_file(&self, type_id: &str, name: &str, directory: Option<&Path>) -> Result<PathBuf> {
        let asset_type = self.get_asset_type(type_id)
            .ok_or_else(|| anyhow!("Unknown asset type: {}", type_id))?;
        
        // Determine directory
        let dir = if let Some(d) = directory {
            d.to_path_buf()
        } else {
            PathBuf::from(asset_type.default_directory())
        };
        
        // Create directory if needed
        std::fs::create_dir_all(&dir)?;
        
        // Generate file path
        let file_name = asset_type.file_name(name);
        let file_path = dir.join(&file_name);
        
        // Generate and write template
        let template = asset_type.generate_template(name);
        std::fs::write(&file_path, template)?;
        
        Ok(file_path)
    }
}

impl Default for AssetRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Global registry instance
static GLOBAL_REGISTRY: once_cell::sync::Lazy<AssetRegistry> = 
    once_cell::sync::Lazy::new(|| AssetRegistry::new());

/// Get the global asset registry
pub fn global_registry() -> &'static AssetRegistry {
    &GLOBAL_REGISTRY
}
