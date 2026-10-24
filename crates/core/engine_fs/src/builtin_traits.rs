//! Trait assets every project has without authoring them.
//!
//! Traits are project assets (`*.trait.json`), so a built-in one is a file the
//! engine writes when it is missing. [`ensure`] runs at the start of every
//! [`BlueprintTraitIndex`](crate::BlueprintTraitIndex) rebuild, so a trait is
//! registered before the index looks for implementors, in new projects and in
//! ones that predate it. An existing file is never overwritten: a project may
//! document or extend its copy.

use std::path::Path;

use anyhow::{Context, Result};

use crate::virtual_fs;

/// Project-relative path of the `IntoGraph` trait.
///
/// Helio builds a renderer from anything that implements `IntoGraph` (a
/// built-in pipeline, a render graph asset). A Blueprint declaring this trait
/// states that it describes a render pipeline.
pub const INTO_GRAPH_TRAIT_PATH: &str = "types/traits/into_graph.trait.json";

const INTO_GRAPH_TRAIT: &str = r#"{
  "schemaVersion": 1,
  "typeKind": "trait",
  "name": "IntoGraph",
  "displayName": "Into Graph",
  "description": "Describes a render pipeline: builds a Helio render graph from the renderer's construction context. Render graph assets (.rendergraph) implement it implicitly.",
  "methods": [
    {
      "name": "into_graph",
      "signature": {
        "params": [],
        "return": { "kind": "Path", "path": "helio::RenderGraph" }
      },
      "doc": "Builds the render graph. Called when the renderer is created and again whenever it is rebuilt (resize, shader reload)."
    }
  ],
  "meta": { "builtin": true }
}
"#;

const BUILTIN: &[(&str, &str)] = &[(INTO_GRAPH_TRAIT_PATH, INTO_GRAPH_TRAIT)];

/// Writes each built-in trait missing from `project_root`.
pub fn ensure(project_root: &Path) -> Result<()> {
    for (relative, content) in BUILTIN {
        let path = project_root.join(relative);
        if virtual_fs::exists(&path).unwrap_or(false) {
            continue;
        }
        if let Some(parent) = path.parent() {
            virtual_fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create '{}'", parent.display()))?;
        }
        virtual_fs::write_file(&path, content.as_bytes())
            .with_context(|| format!("Failed to write built-in trait '{}'", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_trait_is_a_valid_canonical_declaration() {
        let value: serde_json::Value = serde_json::from_str(INTO_GRAPH_TRAIT).unwrap();
        assert_eq!(value["typeKind"], "trait");
        assert_eq!(value["schemaVersion"], 1);
        assert!(value["methods"].is_array());
        assert!(value.get("meta").is_some());
    }
}
