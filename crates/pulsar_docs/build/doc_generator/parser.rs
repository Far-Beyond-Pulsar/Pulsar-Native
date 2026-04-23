use super::workspace::CrateInfo;
/// Source code parser module
///
/// Parses Rust source files using syn to build an AST that can be analyzed
/// for documentation extraction.
use std::error::Error;
use std::fs;
use std::path::Path;
use syn::{File, Item};
use walkdir::WalkDir;

/// Parsed crate containing all source files
#[derive(Debug)]
pub struct ParsedCrate {
    pub crate_info: CrateInfo,
    pub files: Vec<ParsedFile>,
}

/// A single parsed source file
#[derive(Debug)]
pub struct ParsedFile {
    pub path: std::path::PathBuf,
    pub ast: File,
}

/// Parse all source files in a crate
///
/// # Arguments
/// * `crate_info` - Information about the crate to parse
///
/// # Returns
/// ParsedCrate containing all parsed files
pub fn parse_crate(crate_info: &CrateInfo) -> Result<ParsedCrate, Box<dyn Error>> {
    let mut files = Vec::new();

    // Start with the entry point
    files.push(parse_file(&crate_info.entry_point)?);

    // Parse all other .rs files in src/
    let src_dir = crate_info.root_dir.join("src");

    for entry in WalkDir::new(src_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("rs"))
    {
        let path = entry.path();

        // Skip if we already parsed it (entry point)
        if path == crate_info.entry_point {
            continue;
        }

        if let Ok(parsed) = parse_file(path) {
            files.push(parsed);
        }
    }

    Ok(ParsedCrate {
        crate_info: crate_info.clone(),
        files,
    })
}

/// Parse a single Rust source file
fn parse_file(path: &Path) -> Result<ParsedFile, Box<dyn Error>> {
    let content = fs::read_to_string(path)?;
    let ast = syn::parse_file(&content)?;

    Ok(ParsedFile {
        path: path.to_path_buf(),
        ast,
    })
}
