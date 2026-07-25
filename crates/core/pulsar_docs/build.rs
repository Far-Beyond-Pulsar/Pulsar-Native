// Pulsar Documentation Generator
//
// This build script automatically generates documentation at build time
// by parsing workspace crates and creating markdown/JSON files.

#[path = "build/doc_generator/mod.rs"]
mod doc_generator;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=AUTO_GENERATE_DOCS");

    if let Err(error) = generate_docs() {
        panic!("[pulsar_docs] {error}");
    }
}

fn generate_docs() -> Result<(), String> {
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| "Cargo did not provide CARGO_MANIFEST_DIR".to_string())?;
    let workspace_root = find_workspace_root(&manifest_dir)?;
    let doc_dir = workspace_root.join("target").join("doc");

    fs::create_dir_all(&doc_dir).map_err(|error| {
        format!(
            "could not create documentation directory {}: {error}",
            doc_dir.display()
        )
    })?;

    let auto_generate = env::var("AUTO_GENERATE_DOCS")
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(true);
    if !auto_generate {
        println!("cargo:warning=[pulsar_docs] automatic documentation generation is disabled");
        return Ok(());
    }

    let count = doc_generator::generate_workspace_docs(&workspace_root, &doc_dir)
        .map_err(|error| format!("workspace documentation generation failed: {error}"))?;
    if count == 0 {
        return Err(format!(
            "workspace documentation generation produced no crates in {}",
            doc_dir.display()
        ));
    }

    println!("cargo:warning=[pulsar_docs] generated documentation for {count} crates");
    Ok(())
}

fn find_workspace_root(manifest_dir: &Path) -> Result<PathBuf, String> {
    for candidate in manifest_dir.ancestors() {
        let manifest_path = candidate.join("Cargo.toml");
        let Ok(contents) = fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = toml::from_str::<toml::Value>(&contents) else {
            continue;
        };
        if manifest.get("workspace").is_some() {
            return candidate.canonicalize().map_err(|error| {
                format!(
                    "could not canonicalize workspace root {}: {error}",
                    candidate.display()
                )
            });
        }
    }

    Err(format!(
        "could not find a Cargo workspace above {}",
        manifest_dir.display()
    ))
}
