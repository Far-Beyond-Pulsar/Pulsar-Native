//! Unified Tab Management System
//!
//! Uses the trait registry to dynamically open and manage editor tabs

use gpui::*;
use std::path::PathBuf;
use std::collections::HashMap;
use anyhow::Result;

/// A tab in the editor, wrapping any editor instance
pub struct EditorTab {
    /// Unique ID for this tab
    pub id: String,
    
    /// Display title
    pub title: String,
    
    /// Icon for tab
    pub icon: String,
    
    /// File path being edited (if any)
    pub file_path: Option<PathBuf>,
    
    /// Editor type ID
    pub editor_id: String,
    
    /// The actual editor component (type-erased)
    pub editor: AnyElement,
    
    /// Is the file modified?
    pub is_modified: bool,
}

/// Manages all open tabs
pub struct TabManager {
    tabs: HashMap<String, EditorTab>,
    active_tab: Option<String>,
    next_id: usize,
}

impl TabManager {
    pub fn new() -> Self {
        Self {
            tabs: HashMap::new(),
            active_tab: None,
            next_id: 0,
        }
    }
    
    /// Generate a unique tab ID
    fn generate_id(&mut self) -> String {
        let id = format!("tab_{}", self.next_id);
        self.next_id += 1;
        id
    }
    
    /// Open a file in a new tab or focus existing tab
    pub fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut App) -> Result<String> {
        // Check if file is already open
        if let Some(existing_tab) = self.find_tab_by_path(&path) {
            self.active_tab = Some(existing_tab.clone());
            return Ok(existing_tab);
        }
        
        // Use registry to find appropriate editor
        let registry = engine_fs::global_registry();
        
        let editor_type = registry.find_editor_for_file(&path)
            .ok_or_else(|| anyhow::anyhow!("No editor found for file: {:?}", path))?;
        
        let asset_type = registry.find_asset_type_for_file(&path)
            .ok_or_else(|| anyhow::anyhow!("Unknown file type: {:?}", path))?;
        
        // Create editor instance
        // NOTE: This is where we'd create the actual editor component
        // For now, we'll need to add a way to create GPUI elements from editor instances
        // This will require updating the EditorType trait or adding a separate factory
        
        let tab_id = self.generate_id();
        
        let tab = EditorTab {
            id: tab_id.clone(),
            title: path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Untitled")
                .to_string(),
            icon: asset_type.icon().to_string(),
            file_path: Some(path.clone()),
            editor_id: editor_type.editor_id().to_string(),
            editor: div().into_any(), // Placeholder - will be replaced with actual editor
            is_modified: false,
        };
        
        self.tabs.insert(tab_id.clone(), tab);
        self.active_tab = Some(tab_id.clone());
        
        Ok(tab_id)
    }
    
    /// Find tab by file path
    fn find_tab_by_path(&self, path: &PathBuf) -> Option<String> {
        self.tabs.iter()
            .find(|(_, tab)| tab.file_path.as_ref() == Some(path))
            .map(|(id, _)| id.clone())
    }
    
    /// Get active tab
    pub fn active_tab(&self) -> Option<&EditorTab> {
        self.active_tab.as_ref().and_then(|id| self.tabs.get(id))
    }
    
    /// Get all tabs
    pub fn tabs(&self) -> impl Iterator<Item = &EditorTab> {
        self.tabs.values()
    }
    
    /// Close a tab
    pub fn close_tab(&mut self, tab_id: &str) -> Option<EditorTab> {
        let tab = self.tabs.remove(tab_id);
        
        // If closing active tab, switch to another
        if self.active_tab.as_deref() == Some(tab_id) {
            self.active_tab = self.tabs.keys().next().cloned();
        }
        
        tab
    }
    
    /// Set active tab
    pub fn set_active_tab(&mut self, tab_id: String) {
        if self.tabs.contains_key(&tab_id) {
            self.active_tab = Some(tab_id);
        }
    }
    
    /// Check if any tabs are modified
    pub fn has_unsaved_changes(&self) -> bool {
        self.tabs.values().any(|tab| tab.is_modified)
    }
}

impl Default for TabManager {
    fn default() -> Self {
        Self::new()
    }
}
