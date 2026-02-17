/// Pulsar Documentation Generator
/// 
/// Documentation is generated manually using a standalone tool.
/// This build script ensures the docs directory exists and provides instructions if docs aren't present.

use std::path::Path;
use std::fs;

fn main() {
    println!("cargo:warning=[pulsar_docs] Build script started");
    
    let doc_dir = Path::new("../../target/doc");
    println!("cargo:warning=[pulsar_docs] Checking for docs at: {:?}", doc_dir);
    
    // Check if SKIP_DOC_CHECK is set
    let skip_check = std::env::var("SKIP_DOC_CHECK").unwrap_or_default() == "true";
    println!("cargo:warning=[pulsar_docs] SKIP_DOC_CHECK = {}", skip_check);
    
    // Ensure the doc directory exists (even if empty)
    // This is critical for rust-embed to work properly
    if !doc_dir.exists() {
        println!("cargo:warning=[pulsar_docs] Creating empty doc directory for rust-embed");
        if let Err(e) = fs::create_dir_all(doc_dir) {
            println!("cargo:warning=[pulsar_docs] Warning: Could not create doc directory: {}", e);
        }
    }
    
    if skip_check {
        println!("cargo:warning=[pulsar_docs] Skipping doc check");
        println!("cargo:warning=[pulsar_docs] Build script completed successfully");
        return;
    }
    
    // Check if docs exist
    println!("cargo:warning=[pulsar_docs] Checking if doc directory has content...");
    let docs_exist = doc_dir.exists() && 
                     std::fs::read_dir(doc_dir).map(|mut d| d.next().is_some()).unwrap_or(false);
    
    println!("cargo:warning=[pulsar_docs] Docs exist with content: {}", docs_exist);
    
    if !docs_exist {
        // Docs don't exist - print warning with instructions
        println!("cargo:warning=━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("cargo:warning=  ⚠️  Pulsar documentation is not yet generated!");
        println!("cargo:warning=━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("cargo:warning=");
        println!("cargo:warning=To generate documentation, run:");
        println!("cargo:warning=  cargo run --bin pulsar-doc-gen --release");
        println!("cargo:warning=");
        println!("cargo:warning=Or skip this check with:");
        println!("cargo:warning=  SKIP_DOC_CHECK=true cargo build");
        println!("cargo:warning=");
        println!("cargo:warning=Note: Building will proceed, but docs_available() will return false.");
        println!("cargo:warning=━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    }
    
    println!("cargo:warning=[pulsar_docs] Build script completed successfully");
}
