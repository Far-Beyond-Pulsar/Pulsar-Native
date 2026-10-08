//! Rebuildable project index connecting authored Blueprints to trait assets.
//!
//! The source of truth is each Blueprint's `blueprint_metadata.implemented_traits`
//! array. Entries are normalized project-relative trait asset paths such as
//! `types/traits/movable.trait.json`. This module stores only derived data in
//! `.pulsar/blueprint_trait_index.json` and can recreate it from project assets.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Component, Path};

use crate::virtual_fs;

const INDEX_RELATIVE_PATH: &str = ".pulsar/blueprint_trait_index.json";
const SCHEMA_VERSION: u32 = 1;
const BLUEPRINT_FORMAT_VERSION: u64 = 1;

/// One authored Blueprint and the trait assets it declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlueprintTraitEntry {
    /// Normalized path relative to the project root.
    pub blueprint_path: String,
    /// Normalized project-relative paths to valid `.trait.json` assets.
    #[serde(default)]
    pub implemented_traits: Vec<String>,
}

/// A persisted, rebuildable index of Blueprint trait implementations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlueprintTraitIndex {
    schema_version: u32,
    blueprints: Vec<BlueprintTraitEntry>,
}

impl BlueprintTraitIndex {
    /// Path of the derived index relative to a project root.
    pub const RELATIVE_PATH: &'static str = INDEX_RELATIVE_PATH;

    /// Load an existing index, rejecting unsupported or malformed schemas.
    pub fn load(project_root: &Path) -> Result<Self> {
        let path = project_root.join(INDEX_RELATIVE_PATH);
        let bytes = virtual_fs::read_file(&path).with_context(|| {
            format!(
                "Failed to read Blueprint trait index at '{}'",
                path.display()
            )
        })?;
        let index: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("Malformed Blueprint trait index at '{}'", path.display()))?;
        if index.schema_version != SCHEMA_VERSION {
            anyhow::bail!(
                "Unsupported Blueprint trait index schema version {} (expected {})",
                index.schema_version,
                SCHEMA_VERSION
            );
        }
        index.validate()?;
        Ok(index)
    }

    /// Rebuild the index from authored project assets and persist it.
    ///
    /// Malformed assets and references to missing or malformed traits are
    /// skipped with a warning. A failed project manifest or asset read aborts
    /// the rebuild before replacing the previous index.
    pub fn rebuild(project_root: &Path) -> Result<Self> {
        let manifest = virtual_fs::manifest(project_root)
            .with_context(|| format!("Failed to scan project '{}'", project_root.display()))?;

        let mut files = BTreeSet::new();
        for entry in manifest.into_iter().filter(|entry| !entry.is_dir) {
            if let Some(path) = normalize_relative_path(&entry.path) {
                files.insert(path);
            }
        }

        let mut valid_traits = BTreeSet::new();
        for relative in files.iter().filter(|path| is_trait_asset(path)) {
            let path = project_root.join(relative);
            let content = virtual_fs::read_file(&path)
                .with_context(|| format!("Failed to read trait asset '{}'", path.display()))?;
            if valid_trait_declaration(&content) {
                valid_traits.insert(relative.clone());
            } else {
                tracing::warn!(path = %relative, "Skipping malformed trait declaration");
            }
        }

        let mut blueprints = Vec::new();
        for relative in files.iter().filter(|path| is_blueprint_asset(path)) {
            let path = project_root.join(relative);
            let content = virtual_fs::read_file(&path)
                .with_context(|| format!("Failed to read Blueprint asset '{}'", path.display()))?;
            let Some(implemented_traits) = parse_implemented_traits(&content, &valid_traits) else {
                tracing::warn!(path = %relative, "Skipping malformed or unsupported Blueprint asset");
                continue;
            };
            blueprints.push(BlueprintTraitEntry {
                blueprint_path: relative.clone(),
                implemented_traits,
            });
        }
        blueprints.sort_by(|a, b| a.blueprint_path.cmp(&b.blueprint_path));

        let index = Self {
            schema_version: SCHEMA_VERSION,
            blueprints,
        };
        index.persist(project_root)?;
        Ok(index)
    }

    /// Load a usable index by rebuilding from source assets.
    ///
    /// Rebuilding every time is intentional: filesystem providers do not
    /// expose a consistent content timestamp contract, so trusting a cache
    /// could silently return stale trait selections.
    pub fn load_or_rebuild(project_root: &Path) -> Result<Self> {
        Self::rebuild(project_root)
    }

    /// Return all indexed Blueprint entries that implement `trait_path`.
    ///
    /// `trait_path` must be the normalized project-relative asset path.
    pub fn blueprints_for_trait(&self, trait_path: &str) -> Vec<&BlueprintTraitEntry> {
        let Some(trait_path) = normalize_relative_path(trait_path) else {
            return Vec::new();
        };
        self.blueprints
            .iter()
            .filter(|entry| entry.implemented_traits.binary_search(&trait_path).is_ok())
            .collect()
    }

    /// Return trait asset paths implemented by the Blueprint at `blueprint_path`.
    ///
    /// The lookup path is project-relative and accepts either slash style.
    pub fn traits_for_blueprint(&self, blueprint_path: &Path) -> Option<&[String]> {
        let blueprint_path = normalize_relative_path(&blueprint_path.to_string_lossy())?;
        self.blueprints
            .binary_search_by(|entry| entry.blueprint_path.cmp(&blueprint_path))
            .ok()
            .map(|index| self.blueprints[index].implemented_traits.as_slice())
    }

    /// Iterate through indexed Blueprints in deterministic path order.
    pub fn blueprints(&self) -> &[BlueprintTraitEntry] {
        &self.blueprints
    }

    fn persist(&self, project_root: &Path) -> Result<()> {
        let path = project_root.join(INDEX_RELATIVE_PATH);
        let parent = path
            .parent()
            .context("Blueprint trait index path has no parent directory")?;
        virtual_fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create '{}'", parent.display()))?;
        let mut bytes =
            serde_json::to_vec_pretty(self).context("Failed to serialize Blueprint trait index")?;
        bytes.push(b'\n');
        match virtual_fs::write_file_atomically(&path, &bytes) {
            Ok(()) => Ok(()),
            Err(error) if virtual_fs::is_remote() => {
                // Remote and P2P providers currently have no atomic publish
                // operation. The index is derived and can always be rebuilt.
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "Provider has no atomic index write; writing derived index directly"
                );
                virtual_fs::write_file(&path, &bytes)
                    .with_context(|| format!("Failed to write derived index '{}'", path.display()))
            }
            Err(error) => Err(error)
                .with_context(|| format!("Failed to atomically write '{}'", path.display())),
        }
    }

    fn validate(&self) -> Result<()> {
        let mut previous_blueprint: Option<&str> = None;
        for blueprint in &self.blueprints {
            let normalized = normalize_relative_path(&blueprint.blueprint_path)
                .context("Blueprint trait index contains an unsafe Blueprint path")?;
            if normalized != blueprint.blueprint_path
                || !(blueprint.blueprint_path.ends_with(".blueprint.json")
                    || Path::new(&blueprint.blueprint_path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        == Some("graph_save.json"))
            {
                anyhow::bail!("Blueprint trait index contains a non-canonical Blueprint path");
            }
            if previous_blueprint
                .is_some_and(|previous| previous >= blueprint.blueprint_path.as_str())
            {
                anyhow::bail!("Blueprint trait index paths are duplicated or not sorted");
            }
            previous_blueprint = Some(&blueprint.blueprint_path);

            let mut previous_trait: Option<&str> = None;
            for trait_path in &blueprint.implemented_traits {
                let normalized = normalize_relative_path(trait_path)
                    .context("Blueprint trait index contains an unsafe trait path")?;
                if normalized != *trait_path || !is_trait_asset(trait_path) {
                    anyhow::bail!("Blueprint trait index contains a non-canonical trait path");
                }
                if previous_trait.is_some_and(|previous| previous >= trait_path.as_str()) {
                    anyhow::bail!("Blueprint trait index trait paths are duplicated or not sorted");
                }
                previous_trait = Some(trait_path);
            }
        }
        Ok(())
    }
}

fn normalize_relative_path(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return None;
    }
    let parts = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn is_trait_asset(path: &str) -> bool {
    // Trait definitions are project assets and may be stored anywhere in the
    // project. `types/traits/` is the default creation location, not a validity
    // constraint; older projects may already have root-level trait files.
    path.ends_with(".trait.json")
}

fn is_blueprint_asset(path: &str) -> bool {
    path.ends_with(".blueprint.json")
        || Path::new(path).file_name().and_then(|name| name.to_str()) == Some("graph_save.json")
}

fn valid_trait_declaration(bytes: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    let Some(name) = object.get("name").and_then(Value::as_str) else {
        return false;
    };
    if name.trim().is_empty() || !object.get("methods").is_some_and(Value::is_array) {
        return false;
    }

    // Canonical type-system assets use camel-case envelope keys. The older
    // engine_fs template uses snake-case `display_name` and `visibility`.
    // Require one complete, recognizable shape rather than accepting any JSON
    // object that happens to contain a name and methods array.
    let has_canonical_marker =
        object.contains_key("schemaVersion") || object.contains_key("typeKind");
    if has_canonical_marker {
        return object.get("schemaVersion").and_then(Value::as_u64) == Some(1)
            && object.get("typeKind").and_then(Value::as_str) == Some("trait")
            && object
                .get("displayName")
                .and_then(Value::as_str)
                .is_some_and(|display_name| !display_name.trim().is_empty())
            && object.contains_key("meta");
    }

    object
        .get("display_name")
        .and_then(Value::as_str)
        .is_some_and(|display_name| !display_name.trim().is_empty())
}

fn parse_implemented_traits(bytes: &[u8], valid_traits: &BTreeSet<String>) -> Option<Vec<String>> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    if value.get("format_version")?.as_u64()? != BLUEPRINT_FORMAT_VERSION {
        return None;
    }
    if !value.get("main_graph")?.is_object() || !value.get("variables")?.is_array() {
        return None;
    }

    let Some(raw_traits) = value
        .get("blueprint_metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("implemented_traits"))
    else {
        return Some(Vec::new());
    };
    let raw_traits = raw_traits.as_array()?;

    let mut implemented_traits = BTreeSet::new();
    for trait_value in raw_traits {
        let Some(trait_path) = trait_value.as_str().and_then(normalize_relative_path) else {
            tracing::warn!("Ignoring invalid Blueprint trait reference");
            continue;
        };
        if !is_trait_asset(&trait_path) {
            tracing::warn!(
                trait_path,
                "Ignoring non-canonical Blueprint trait reference"
            );
            continue;
        }
        if valid_traits.contains(&trait_path) {
            implemented_traits.insert(trait_path);
        } else {
            tracing::warn!(
                trait_path,
                "Ignoring missing or malformed Blueprint trait reference"
            );
        }
    }
    Some(implemented_traits.into_iter().collect())
}
