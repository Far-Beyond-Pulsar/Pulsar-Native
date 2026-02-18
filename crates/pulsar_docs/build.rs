/// Pulsar Documentation Generator
/// 
/// This build script automatically generates documentation at build time
/// by parsing workspace crates and creating markdown/JSON files.

#[path = "build/doc_generator/mod.rs"]
mod doc_generator;

use std::path::Path;
use std::fs;

fn main() {
    println!("cargo:warning=[pulsar_docs] Build script started");
    
    let workspace_root = Path::new("../..");
    let doc_dir = workspace_root.join("target/doc");
    
    println!("cargo:warning=[pulsar_docs] Workspace root: {:?}", workspace_root);
    println!("cargo:warning=[pulsar_docs] Doc directory: {:?}", doc_dir);
    
    // Check if AUTO_GENERATE_DOCS is disabled
    let auto_generate = std::env::var("AUTO_GENERATE_DOCS").unwrap_or_else(|_| "true".to_string()) == "true";
    println!("cargo:warning=[pulsar_docs] AUTO_GENERATE_DOCS = {}", auto_generate);
    
    // Ensure the doc directory exists (critical for rust-embed)
    if !doc_dir.exists() {
        println!("cargo:warning=[pulsar_docs] Creating doc directory");
        if let Err(e) = fs::create_dir_all(&doc_dir) {
            println!("cargo:warning=[pulsar_docs] Warning: Could not create doc directory: {}", e);
        }
    }
    
    if !auto_generate {
        println!("cargo:warning=[pulsar_docs] Skipping automatic doc generation");
        println!("cargo:warning=[pulsar_docs] Build script completed");
        return;
    }
    
    // Check if docs already exist with content
    let has_json_files = doc_dir.exists() && 
        std::fs::read_dir(&doc_dir)
            .ok()
            .and_then(|entries| {
                entries.filter_map(Result::ok)
                    .any(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
                    .then_some(())
            })
            .is_some();
    
    if has_json_files {
        println!("cargo:warning=[pulsar_docs] Documentation already exists, skipping generation");
        println!("cargo:warning=[pulsar_docs] To regenerate docs, delete target/doc and rebuild");
        println!("cargo:warning=[pulsar_docs] Build script completed successfully");
        return;
    }
    
    // Generate documentation
    println!("cargo:warning=[pulsar_docs] Generating workspace documentation...");
    
    match doc_generator::generate_workspace_docs(workspace_root, &doc_dir) {
        Ok(count) => {
            println!("cargo:warning=[pulsar_docs] ✓ Successfully generated docs for {} crates", count);
        }
        Err(e) => {
            println!("cargo:warning=[pulsar_docs] ✗ Failed to generate docs: {}", e);
            println!("cargo:warning=[pulsar_docs] Documentation will be unavailable in the UI");
        }
    }
    
    println!("cargo:warning=[pulsar_docs] Build script completed successfully");
}
