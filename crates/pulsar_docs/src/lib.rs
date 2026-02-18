use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};


pub mod project_parser;

// RustEmbed scans the doc folder at compile time
// Uses a simple relative path (../../target/doc) from crates/pulsar_docs/ to workspace root
// This is more reliable than $CARGO_MANIFEST_DIR which may not interpolate correctly in attribute macros
// The build.rs script ensures this directory exists (even if empty) so rust-embed won't fail
// If files are present in target/doc, they will be embedded; if not, docs_available() returns false
#[derive(RustEmbed)]
#[folder = "../../target/doc"]
#[prefix = ""] 
// Only include json and md files to avoid scanning everything
#[include = "*.json"]
#[include = "*.md"]
#[include = "**/*.json"]
#[include = "**/*.md"]
pub struct DocAssets;

/// JSON index structures matching the build script
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CrateIndex {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub sections: Vec<Section>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Section {
    pub name: String,
    pub path: String,
    pub count: usize,
    pub items: Vec<IndexItem>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct IndexItem {
    pub name: String,
    pub path: String,
    pub doc_summary: Option<String>,
}

/// Get markdown content for any doc page
pub fn get_doc_content(path: &str) -> Option<String> {
    if let Some(content) = DocAssets::get(path) {
        std::str::from_utf8(&content.data).ok().map(String::from)
    } else {
        None
    }
}

/// Get the index.json for a crate
pub fn get_crate_index(crate_name: &str) -> Option<CrateIndex> {
    let index_path = format!("{}/index.json", crate_name);
    
    if let Some(content) = DocAssets::get(&index_path) {
        let json_str = std::str::from_utf8(&content.data).ok()?;
        serde_json::from_str(json_str).ok()
    } else {
        None
    }
}

/// Get list of all documented crates by scanning for index.json files
pub fn list_crates() -> Vec<String> {
    let mut crates = Vec::new();
    
    for file_path in DocAssets::iter() {
        let path = file_path.as_ref();
        if path.ends_with("/index.json") {
            let crate_name = path.trim_end_matches("/index.json");
            crates.push(crate_name.to_string());
        }
    }
    
    crates.sort();
    crates
}

/// Check if docs are available
pub fn docs_available() -> bool {
    !list_crates().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_docs_are_embedded() {
        // This test verifies that documentation was successfully embedded
        let available = docs_available();
        println!("docs_available: {}", available);
        
        let crates = list_crates();
        println!("Found {} crates", crates.len());
        for crate_name in crates.iter().take(5) {
            println!("  - {}", crate_name);
        }
        
        assert!(available, "Documentation should be embedded after build script runs");
        assert!(!crates.is_empty(), "Should have at least one documented crate");
    }
    
    #[test]
    fn test_can_load_crate_index() {
        let crates = list_crates();
        if let Some(first_crate) = crates.first() {
            let index = get_crate_index(first_crate);
            assert!(index.is_some(), "Should be able to load index.json for {}", first_crate);
            
            if let Some(idx) = index {
                println!("Loaded index for {}: {} sections", idx.name, idx.sections.len());
            }
        }
    }
}
