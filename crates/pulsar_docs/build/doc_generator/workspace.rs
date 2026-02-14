/// Workspace discovery module
/// 
/// Discovers and analyzes Cargo workspaces to find all crates that need documentation.
/// Parses Cargo.toml files to extract metadata about each crate.

use std::path::{Path, PathBuf};
use std::error::Error;
use serde::Deserialize;

/// Information about a crate in the workspace
#[derive(Debug, Clone)]
pub struct CrateInfo {
    /// Name of the crate
    pub name: String,
    /// Version of the crate
    pub version: String,
    /// Root directory of the crate (contains Cargo.toml)
    pub root_dir: PathBuf,
    /// Path to the lib.rs or main.rs file
    pub entry_point: PathBuf,
    /// Description from Cargo.toml
    pub description: Option<String>,
    /// Authors from Cargo.toml
    pub authors: Vec<String>,
    /// Whether this is a binary crate
    pub is_bin: bool,
}

#[derive(Deserialize)]
struct CargoToml {
    package: Option<Package>,
    workspace: Option<Workspace>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    version: String,
    description: Option<String>,
    authors: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct Workspace {
    members: Option<Vec<String>>,
}

/// Discover all crates in a Cargo workspace
/// 
/// # Arguments
/// * `workspace_root` - Root directory of the workspace
/// 
/// # Returns
/// Vector of CrateInfo for each crate found
pub fn discover_crates(workspace_root: &Path) -> Result<Vec<CrateInfo>, Box<dyn Error>> {
    let mut crates = Vec::new();
    
    // Read the workspace Cargo.toml
    let workspace_toml_path = workspace_root.join("Cargo.toml");
    let workspace_toml_content = std::fs::read_to_string(&workspace_toml_path)?;
    let workspace_toml: CargoToml = toml::from_str(&workspace_toml_content)?;
    
    // Get workspace members
    let members = workspace_toml
        .workspace
        .and_then(|w| w.members)
        .unwrap_or_default();
    
    // Process each member
    for member_path in members {
        let member_dir = workspace_root.join(&member_path);
        
        if !member_dir.exists() {
            // Might be a glob pattern, expand it
            if let Ok(expanded) = expand_glob(workspace_root, &member_path) {
                for dir in expanded {
                    if let Ok(crate_info) = parse_crate_info(&dir) {
                        // Skip pulsar_docs to avoid documenting itself
                        if crate_info.name != "pulsar_docs" {
                            crates.push(crate_info);
                        }
                    }
                }
            }
            continue;
        }
        
        // Parse this crate
        if let Ok(crate_info) = parse_crate_info(&member_dir) {
            // Skip pulsar_docs to avoid documenting itself
            if crate_info.name != "pulsar_docs" {
                crates.push(crate_info);
            }
        }
    }
    
    Ok(crates)
}

/// Expand a glob pattern to find crate directories
fn expand_glob(workspace_root: &Path, pattern: &str) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut dirs = Vec::new();
    
    // Simple glob handling for patterns like "crates/*" or "ui-crates/*"
    if pattern.ends_with("/*") {
        let base_dir = workspace_root.join(&pattern[..pattern.len()-2]);
        
        if base_dir.exists() {
            for entry in std::fs::read_dir(base_dir)? {
                let entry = entry?;
                if entry.path().is_dir() {
                    let cargo_toml = entry.path().join("Cargo.toml");
                    if cargo_toml.exists() {
                        dirs.push(entry.path());
                    }
                }
            }
        }
    }
    
    Ok(dirs)
}

/// Parse a single crate's Cargo.toml and determine its structure
fn parse_crate_info(crate_dir: &Path) -> Result<CrateInfo, Box<dyn Error>> {
    let cargo_toml_path = crate_dir.join("Cargo.toml");
    let cargo_toml_content = std::fs::read_to_string(&cargo_toml_path)?;
    let cargo_toml: CargoToml = toml::from_str(&cargo_toml_content)?;
    
    let package = cargo_toml.package.ok_or("No package section in Cargo.toml")?;
    
    // Find entry point (lib.rs or main.rs)
    let src_dir = crate_dir.join("src");
    let lib_rs = src_dir.join("lib.rs");
    let main_rs = src_dir.join("main.rs");
    
    let (entry_point, is_bin) = if lib_rs.exists() {
        (lib_rs, false)
    } else if main_rs.exists() {
        (main_rs, true)
    } else {
        return Err("No lib.rs or main.rs found".into());
    };
    
    Ok(CrateInfo {
        name: package.name,
        version: package.version,
        root_dir: crate_dir.to_path_buf(),
        entry_point,
        description: package.description,
        authors: package.authors.unwrap_or_default(),
        is_bin,
    })
}
