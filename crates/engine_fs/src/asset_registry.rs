//! Asset Registry
//!
//! Maintains indexes for all asset types (structs, enums, traits, etc.)

use anyhow::Result;
use dashmap::DashMap;
use std::path::PathBuf;

/// Information about a registered struct
#[derive(Debug, Clone)]
pub struct StructInfo {
    pub name: String,
    pub file_path: PathBuf,
}

/// Information about a registered enum
#[derive(Debug, Clone)]
pub struct EnumInfo {
    pub name: String,
    pub file_path: PathBuf,
}

/// Information about a registered trait
#[derive(Debug, Clone)]
pub struct TraitInfo {
    pub name: String,
    pub file_path: PathBuf,
}

/// Central registry for all asset types
pub struct AssetRegistry {
    structs: DashMap<String, StructInfo>,
    enums: DashMap<String, EnumInfo>,
    traits: DashMap<String, TraitInfo>,
}

impl AssetRegistry {
    pub fn new() -> Self {
        Self {
            structs: DashMap::new(),
            enums: DashMap::new(),
            traits: DashMap::new(),
        }
    }
    
    /// Register a struct file
    pub fn register_struct(&self, file_path: &PathBuf) -> Result<()> {
        // TODO: Parse struct file and extract name
        // For now, use filename as placeholder
        if let Some(name) = file_path.file_stem() {
            let name = name.to_string_lossy().to_string();
            self.structs.insert(name.clone(), StructInfo {
                name,
                file_path: file_path.clone(),
            });
        }
        Ok(())
    }
    
    /// Register an enum file
    pub fn register_enum(&self, file_path: &PathBuf) -> Result<()> {
        if let Some(name) = file_path.file_stem() {
            let name = name.to_string_lossy().to_string();
            self.enums.insert(name.clone(), EnumInfo {
                name,
                file_path: file_path.clone(),
            });
        }
        Ok(())
    }
    
    /// Register a trait file
    pub fn register_trait(&self, file_path: &PathBuf) -> Result<()> {
        if let Some(name) = file_path.file_stem() {
            let name = name.to_string_lossy().to_string();
            self.traits.insert(name.clone(), TraitInfo {
                name,
                file_path: file_path.clone(),
            });
        }
        Ok(())
    }
    
    /// Get all structs
    pub fn get_all_structs(&self) -> Vec<StructInfo> {
        self.structs.iter().map(|e| e.value().clone()).collect()
    }
    
    /// Get all enums
    pub fn get_all_enums(&self) -> Vec<EnumInfo> {
        self.enums.iter().map(|e| e.value().clone()).collect()
    }
    
    /// Get all traits
    pub fn get_all_traits(&self) -> Vec<TraitInfo> {
        self.traits.iter().map(|e| e.value().clone()).collect()
    }
    
    /// Clear all registries
    pub fn clear(&self) {
        self.structs.clear();
        self.enums.clear();
        self.traits.clear();
    }
}

impl Default for AssetRegistry {
    fn default() -> Self {
        Self::new()
    }
}
