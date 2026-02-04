use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;

// ============================================================================
// FILE SYSTEM METADATA - .pulsar_fs_meta management
// ============================================================================

const METADATA_FILE: &str = ".pulsar_fs_meta";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ColorOverride {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl From<gpui::Hsla> for ColorOverride {
    fn from(hsla: gpui::Hsla) -> Self {
        let rgba = gpui::Rgba::from(hsla);
        Self {
            r: (rgba.r * 255.0) as u8,
            g: (rgba.g * 255.0) as u8,
            b: (rgba.b * 255.0) as u8,
            a: (rgba.a * 255.0) as u8,
        }
    }
}

impl From<ColorOverride> for gpui::Hsla {
    fn from(color: ColorOverride) -> Self {
        let hex = ((color.r as u32) << 16) | ((color.g as u32) << 8) | (color.b as u32);
        gpui::rgb(hex).into()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileMetadata {
    pub color_override: Option<ColorOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FolderMetadata {
    pub files: HashMap<String, FileMetadata>,
}

impl FolderMetadata {
    /// Load metadata from a folder's .pulsar_fs_meta file
    pub fn load(folder_path: &Path) -> Self {
        let metadata_path = folder_path.join(METADATA_FILE);
        
        if let Ok(contents) = fs::read_to_string(&metadata_path) {
            if let Ok(metadata) = serde_json::from_str(&contents) {
                return metadata;
            }
        }
        
        Self::default()
    }
    
    /// Save metadata to a folder's .pulsar_fs_meta file
    pub fn save(&self, folder_path: &Path) -> Result<(), std::io::Error> {
        let metadata_path = folder_path.join(METADATA_FILE);
        let contents = serde_json::to_string_pretty(self)?;
        fs::write(&metadata_path, contents)?;
        Ok(())
    }
    
    /// Get color override for a file
    pub fn get_color_override(&self, filename: &str) -> Option<gpui::Hsla> {
        self.files
            .get(filename)
            .and_then(|meta| meta.color_override.clone())
            .map(|color| color.into())
    }
    
    /// Set color override for a file
    pub fn set_color_override(&mut self, filename: &str, color: Option<gpui::Hsla>) {
        if let Some(color_val) = color {
            self.files
                .entry(filename.to_string())
                .or_insert_with(FileMetadata::default)
                .color_override = Some(color_val.into());
        } else {
            // Remove color override
            if let Some(meta) = self.files.get_mut(filename) {
                meta.color_override = None;
                
                // Remove entry if it has no metadata
                if meta.color_override.is_none() {
                    self.files.remove(filename);
                }
            }
        }
    }
    
    /// Rename a file in the metadata
    pub fn rename_file(&mut self, old_name: &str, new_name: &str) {
        if let Some(meta) = self.files.remove(old_name) {
            self.files.insert(new_name.to_string(), meta);
        }
    }
}

/// Manager for file system metadata across folders
pub struct FsMetadataManager {
    cache: HashMap<PathBuf, FolderMetadata>,
}

impl FsMetadataManager {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }
    
    /// Get metadata for a folder (loads and caches)
    pub fn get_folder_metadata(&mut self, folder_path: &Path) -> &mut FolderMetadata {
        self.cache
            .entry(folder_path.to_path_buf())
            .or_insert_with(|| FolderMetadata::load(folder_path))
    }
    
    /// Save metadata for a folder
    pub fn save_folder_metadata(&mut self, folder_path: &Path) -> Result<(), std::io::Error> {
        if let Some(metadata) = self.cache.get(folder_path) {
            metadata.save(folder_path)?;
        }
        Ok(())
    }
    
    /// Get color override for a specific file
    pub fn get_color_override(&mut self, file_path: &Path) -> Option<gpui::Hsla> {
        let folder = file_path.parent()?;
        let filename = file_path.file_name()?.to_str()?;
        
        self.get_folder_metadata(folder)
            .get_color_override(filename)
    }
    
    /// Set color override for a specific file
    pub fn set_color_override(&mut self, file_path: &Path, color: Option<gpui::Hsla>) -> Result<(), std::io::Error> {
        let folder = file_path.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "No parent folder")
        })?;
        
        let filename = file_path.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid filename")
            })?;
        
        self.get_folder_metadata(folder)
            .set_color_override(filename, color);
            
        self.save_folder_metadata(folder)?;
        Ok(())
    }
    
    /// Handle file rename in metadata
    pub fn rename_file(&mut self, old_path: &Path, new_path: &Path) -> Result<(), std::io::Error> {
        let folder = old_path.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "No parent folder")
        })?;
        
        let old_name = old_path.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid old filename")
            })?;
            
        let new_name = new_path.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid new filename")
            })?;
        
        self.get_folder_metadata(folder)
            .rename_file(old_name, new_name);
            
        self.save_folder_metadata(folder)?;
        Ok(())
    }
    
    /// Clear cache for a folder (useful after refreshing)
    pub fn clear_cache(&mut self, folder_path: &Path) {
        self.cache.remove(folder_path);
    }
}
