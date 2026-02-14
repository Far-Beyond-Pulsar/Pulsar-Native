// Build script for generating WIT bindings
// Based on Zed's extension_host build.rs

use anyhow::Result;

fn main() -> Result<()> {
    // WIT files changed, rebuild
    println!("cargo:rerun-if-changed=wit");
    
    // Note: We'll use wasmtime's bindgen! macro instead of build-time generation
    // This is simpler and matches how Zed does it
    
    Ok(())
}
