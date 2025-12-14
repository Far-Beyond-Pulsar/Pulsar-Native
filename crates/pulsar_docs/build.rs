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

use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    println!("cargo:rerun-if-changed=../../crates/");
    println!("cargo:rerun-if-changed=../../ui-crates/");
    
    // Ensure the embedded docs folder always exists so `rust-embed` can compile in debug builds.
    let output_dir = Path::new("../../target/doc");
    let _ = fs::create_dir_all(output_dir);

    // Only generate docs in release builds
    if std::env::var("PROFILE").unwrap_or_default() != "release" {
        println!("cargo:warning=Skipping doc generation in debug mode");
        return;
    }
    
    println!("cargo:warning=Starting Pulsar documentation generation...");
    
    let workspace_root = Path::new("../../");
    
    match doc_generator::generate_workspace_docs(workspace_root, output_dir) {
        Ok(count) => {
            println!("cargo:warning=Successfully generated documentation for {} crates", count);
        }
        Err(e) => {
            println!("cargo:warning=Documentation generation failed: {}", e);
        }
    }
}
