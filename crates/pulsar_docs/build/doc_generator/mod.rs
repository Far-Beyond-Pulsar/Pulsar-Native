/// Main documentation generator module
/// 
/// Orchestrates the entire documentation generation process

pub mod workspace;
pub mod parser;
pub mod extractor;
pub mod markdown;
pub mod types;

use std::path::Path;
use std::error::Error;

pub use workspace::{CrateInfo, discover_crates};
pub use parser::{ParsedCrate, parse_crate};
pub use extractor::extract_documentation;
pub use markdown::generate_hierarchical_docs;

/// Generate documentation for an entire workspace
/// 
/// # Arguments
/// * `workspace_root` - Root directory of the Cargo workspace
/// * `output_dir` - Directory to write generated markdown files
/// 
/// # Returns
/// Number of crates successfully documented
pub fn generate_workspace_docs(
    workspace_root: &Path,
    output_dir: &Path,
) -> Result<usize, Box<dyn Error>> {
    // Step 1: Discover all crates in the workspace
    let crates = workspace::discover_crates(workspace_root)?;
    
    tracing::debug!("cargo:warning=Found {} crates in workspace", crates.len());
    
    // Step 2: Parse and generate documentation for each crate
    let mut success_count = 0;
    
    for crate_info in crates {
        tracing::debug!("cargo:warning=Processing crate: {}", crate_info.name);
        
        match generate_crate_docs(&crate_info, output_dir) {
            Ok(()) => {
                success_count += 1;
                tracing::debug!("cargo:warning=  ✓ Generated docs for {}", crate_info.name);
            }
            Err(e) => {
                tracing::debug!("cargo:warning=  ✗ Failed to generate docs for {}: {}", crate_info.name, e);
            }
        }
    }
    
    Ok(success_count)
}

/// Generate documentation for a single crate
fn generate_crate_docs(
    crate_info: &workspace::CrateInfo,
    output_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    // Step 1: Parse all source files in the crate
    let parsed_crate = parser::parse_crate(crate_info)?;
    
    // Step 2: Extract documentation from the parsed AST
    let docs = extractor::extract_documentation(&parsed_crate)?;
    
    // Step 3: Generate hierarchical folder structure with individual pages
    markdown::generate_hierarchical_docs(&docs, crate_info, output_dir)?;
    
    Ok(())
}
