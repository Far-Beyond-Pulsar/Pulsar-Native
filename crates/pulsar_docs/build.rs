/// Pulsar Documentation Generator
/// 
/// Documentation is generated manually using a standalone tool.
/// This build script only checks if docs exist and provides instructions if not.

use std::path::Path;

fn main() {
    println!("cargo:warning=[pulsar_docs] Build script started");
    
    let doc_dir = Path::new("../../target/doc");
    println!("cargo:warning=[pulsar_docs] Checking for docs at: {:?}", doc_dir);
    
    // Check if SKIP_DOC_CHECK is set
    let skip_check = std::env::var("SKIP_DOC_CHECK").unwrap_or_default() == "true";
    println!("cargo:warning=[pulsar_docs] SKIP_DOC_CHECK = {}", skip_check);
    
    if skip_check {
        println!("cargo:warning=[pulsar_docs] Skipping doc check");
        return;
    }
    
    // Check if docs exist
    println!("cargo:warning=[pulsar_docs] Checking if doc directory exists...");
    let docs_exist = doc_dir.exists() && 
                     std::fs::read_dir(doc_dir).map(|mut d| d.next().is_some()).unwrap_or(false);
    
    println!("cargo:warning=[pulsar_docs] Docs exist: {}", docs_exist);
    
    if !docs_exist {
        // Docs don't exist - fail with instructions
        println!("cargo:warning=━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("cargo:warning=  Pulsar documentation needs to be generated!");
        println!("cargo:warning=━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("cargo:warning=");
        println!("cargo:warning=To generate documentation, run:");
        println!("cargo:warning=  cargo run --bin pulsar-doc-gen --release");
        println!("cargo:warning=");
        println!("cargo:warning=Or skip this check with:");
        println!("cargo:warning=  SKIP_DOC_CHECK=true cargo build");
        println!("cargo:warning=");
        panic!("Documentation missing - see instructions above");
    }
    
    println!("cargo:warning=[pulsar_docs] Build script completed successfully");
}
