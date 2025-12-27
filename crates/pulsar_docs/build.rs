/// Pulsar Documentation Generator
/// 
/// A comprehensive documentation generator that parses Rust source code and generates
/// well-structured Markdown documentation with:
/// - Full type information
/// - Documentation comments
/// - Source code examples
/// - Module hierarchy
/// - Cross-references
/// 
/// This runs at build time to generate documentation that is embedded into the binary.

#[path = "build/doc_generator/mod.rs"]
mod doc_generator;

use std::path::Path;

fn main() {
    tracing::info!("cargo:rerun-if-changed=../../Cargo.toml");
    tracing::info!("cargo:rerun-if-changed=../../crates/");
    tracing::info!("cargo:rerun-if-changed=../../ui-crates/");
    
    // Only generate docs in release builds
    if std::env::var("PROFILE").unwrap_or_default() != "release" {
        tracing::info!("cargo:warning=Skipping doc generation in debug mode");
        return;
    }
    
    tracing::info!("cargo:warning=Starting Pulsar documentation generation...");
    
    let workspace_root = Path::new("../../");
    let output_dir = Path::new("../../target/doc");
    
    match doc_generator::generate_workspace_docs(workspace_root, output_dir) {
        Ok(count) => {
            tracing::info!("cargo:warning=Successfully generated documentation for {} crates", count);
        }
        Err(e) => {
            tracing::info!("cargo:warning=Documentation generation failed: {}", e);
        }
    }
}
