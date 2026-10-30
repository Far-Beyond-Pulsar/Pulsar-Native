//! Rebuildable project index connecting authored Blueprints to trait assets.
//!
//! The sources of truth are the trait assets (`*.trait.json`) and each
//! Blueprint's `blueprint_metadata`: `implemented_trait_ids` (stable trait
//! ids) and `implemented_traits` (project-relative paths, the pre-id format
//! and a fallback hint). This module stores only derived data in
//! `.pulsar/blueprint_trait_index.json` and can recreate it from project
//! assets. The resolved model is a [`TraitDb`]; see `.agents/TRAITS.md`.
//!
//! A trait's identity is its id, not its path:
//!
//! - the id is embedded in the trait asset (`"id"`). An asset without one
//!   gets one when the index is rebuilt (a copy that duplicates another's id
//!   gets a fresh one), falling back to the project's durable asset-id
//!   manifest (`.pulsar/asset_ids.json`, kind `trait`) when the file cannot
//!   be rewritten;
//! - Blueprints reference ids. A path reference still resolves (an asset
//!   from before ids is "path only" until it is next saved) so nothing
//!   breaks, and a renamed or moved trait keeps its implementers;
//! - the index resolves id -> current path.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::trait_db::TraitDb;
use crate::trait_records::{
    embedded_id, is_blueprint_asset, is_trait_asset, normalize_relative_path, parse_trait_record,
    valid_trait_declaration, TraitRecord,
};
use crate::virtual_fs;

const INDEX_RELATIVE_PATH: &str = ".pulsar/blueprint_trait_index.json";
const ASSET_ID_MANIFEST: &str = ".pulsar/asset_ids.json";
const TYPE_INDEX_RELATIVE_PATH: &str = "type-index/index.json";
const SCHEMA_VERSION: u32 = 2;
/// Stand-in path of an engine trait no project file provides.
const BUILTIN_PATH_PREFIX: &str = "builtin:";
/// Blueprint `format_version`s the editor writes and reads (graphy-blueprint
/// `codec::is_version_supported`). Version 1 is the original; 2 is current.
const BLUEPRINT_FORMAT_VERSIONS: [u64; 2] = [1, 2];
/// Kind under which trait ids are recorded in the asset-id systems.
pub const TRAIT_ASSET_KIND: &str = "trait";

/// One authored Blueprint and the traits it declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlueprintTraitEntry {
    /// Normalized path relative to the project root.
    pub blueprint_path: String,
    /// Current project-relative paths of the traits it declares, sorted. Kept
    /// for readers that predate trait ids; ids are authoritative.
    #[serde(default)]
    pub implemented_traits: Vec<String>,
    /// Ids of the traits it declares that exist, sorted.
    #[serde(default)]
    pub implemented_trait_ids: Vec<String>,
    /// References (ids or paths) that name no trait now.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<String>,
    /// Declares traits by path only: saved before trait ids existed. Saving
    /// it with the Blueprint editor writes ids and clears this.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub path_only: bool,
}

#[derive(Serialize, Deserialize)]
struct Persisted {
    schema_version: u32,
    traits: Vec<TraitRecord>,
    blueprints: Vec<BlueprintTraitEntry>,
}

/// A persisted, rebuildable index of traits and Blueprint implementations.
#[derive(Debug, Clone)]
pub struct BlueprintTraitIndex {
    traits: Vec<TraitRecord>,
    blueprints: Vec<BlueprintTraitEntry>,
    blueprint_pos: HashMap<String, usize>,
    db: TraitDb,
}

impl PartialEq for BlueprintTraitIndex {
    fn eq(&self, other: &Self) -> bool {
        self.traits == other.traits && self.blueprints == other.blueprints
    }
}

/// The references one Blueprint makes to traits, as authored.
pub(crate) struct Declared {
    pub(crate) ids: Vec<String>,
    pub(crate) paths: Vec<String>,
}

/// Source facts read once per full scan.
#[derive(Default)]
struct ScanContext {
    /// Path -> id from the durable asset-id manifest, kind `trait`.
    manifest_ids: HashMap<String, String>,
    /// Trait names listed by the type system's `type-index/index.json`.
    type_index_traits: BTreeSet<String>,
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
        let persisted: Persisted = serde_json::from_slice(&bytes)
            .with_context(|| format!("Malformed Blueprint trait index at '{}'", path.display()))?;
        if persisted.schema_version != SCHEMA_VERSION {
            anyhow::bail!(
                "Unsupported Blueprint trait index schema version {} (expected {})",
                persisted.schema_version,
                SCHEMA_VERSION
            );
        }
        Self::validate(&persisted.traits, &persisted.blueprints)?;
        let index = Self::assemble(persisted.traits, persisted.blueprints);
        index.publish(project_root);
        Ok(index)
    }

    /// Rebuild the index from authored project assets and persist it.
    ///
    /// Malformed assets and references to missing or malformed traits are
    /// skipped with a warning. A failed project manifest or asset read aborts
    /// the rebuild before replacing the previous index. Trait assets without
    /// an id are given one (see the module docs).
    pub fn rebuild(project_root: &Path) -> Result<Self> {
        // Built-in traits are registered before implementors are looked up. A
        // failure to write one (a read-only project) must not stop the index;
        // the database still serves the engine's definition below.
        if let Err(error) = crate::builtin_traits::ensure(project_root) {
            tracing::warn!("Could not register built-in traits: {error:#}");
        }
        let manifest = virtual_fs::manifest(project_root)
            .with_context(|| format!("Failed to scan project '{}'", project_root.display()))?;

        let mut files = BTreeSet::new();
        for entry in manifest.into_iter().filter(|entry| !entry.is_dir) {
            if let Some(path) = normalize_relative_path(&entry.path) {
                files.insert(path);
            }
        }

        let context = ScanContext::read(project_root);
        let mut records: Vec<TraitRecord> = Vec::new();
        let mut taken_ids = BTreeSet::new();
        for relative in files.iter().filter(|path| is_trait_asset(path)) {
            let path = project_root.join(relative);
            let content = virtual_fs::read_file(&path)
                .with_context(|| format!("Failed to read trait asset '{}'", path.display()))?;
            let Ok(mut value) = serde_json::from_slice::<Value>(&content) else {
                tracing::warn!(path = %relative, "Skipping malformed trait declaration");
                continue;
            };
            if !valid_trait_declaration(&value) {
                tracing::warn!(path = %relative, "Skipping malformed trait declaration");
                continue;
            }
            let id = settle_identity(project_root, relative, &mut value, &taken_ids, &context);
            mirror_to_file_index(project_root, relative, &value);
            taken_ids.insert(id.clone());
            if let Some(mut record) = parse_trait_record(&value, relative, id) {
                record.type_index_id = context.type_index_id(&record.name);
                records.push(record);
            }
        }
        add_missing_builtins(&mut records);

        let seed = TraitDb::build(records.clone(), Vec::new());
        let mut blueprints = Vec::new();
        for relative in files.iter().filter(|path| is_blueprint_asset(path)) {
            let path = project_root.join(relative);
            let content = virtual_fs::read_file(&path)
                .with_context(|| format!("Failed to read Blueprint asset '{}'", path.display()))?;
            let Some(declared) = parse_declared(&content) else {
                tracing::warn!(path = %relative, "Skipping malformed or unsupported Blueprint asset");
                continue;
            };
            blueprints.push(entry_for(&seed, relative, &declared));
        }

        let index = Self::assemble(records, blueprints);
        index.persist(project_root)?;
        index.publish(project_root);
        Ok(index)
    }

    /// Load a usable index by rebuilding from source assets.
    ///
    /// Rebuilding every time is intentional: filesystem providers do not
    /// expose a consistent content timestamp contract, so trusting a cache
    /// could silently return stale trait selections. After a single known
    /// change (a save), [`update_path`](Self::update_path) is cheaper.
    pub fn load_or_rebuild(project_root: &Path) -> Result<Self> {
        Self::rebuild(project_root)
    }

    /// Bring the persisted index up to date after `path` (a Blueprint or
    /// trait asset, absolute or project-relative) was saved, moved or
    /// deleted, re-reading only that file. Falls back to a full
    /// [`rebuild`](Self::rebuild) when there is no usable index to update.
    ///
    /// This trusts the persisted index to describe every *other* file. Use
    /// it from a save hook; use `rebuild` to open a project, since changes
    /// made outside the editor (version control, a file manager) are only
    /// seen by a full scan.
    pub fn update_path(project_root: &Path, path: &Path) -> Result<Self> {
        let mut index = match Self::load(project_root) {
            Ok(index) => index,
            Err(error) => {
                tracing::debug!("Rebuilding Blueprint trait index instead of updating: {error:#}");
                return Self::rebuild(project_root);
            }
        };
        if index.apply_path(project_root, path)? {
            index.persist(project_root)?;
            index.publish(project_root);
        }
        Ok(index)
    }

    /// Apply a change to one file in memory. Returns whether the index
    /// changed. Paths that are neither Blueprints nor traits are ignored.
    pub fn apply_path(&mut self, project_root: &Path, path: &Path) -> Result<bool> {
        let Some(relative) = relative_to(project_root, path) else {
            return Ok(false);
        };
        if is_trait_asset(&relative) {
            self.apply_trait(project_root, &relative)
        } else if is_blueprint_asset(&relative) {
            self.apply_blueprint(project_root, &relative)
        } else {
            Ok(false)
        }
    }

    fn apply_trait(&mut self, project_root: &Path, relative: &str) -> Result<bool> {
        let path = project_root.join(relative);
        let before = self.traits.clone();
        let mut traits: Vec<TraitRecord> = self
            .traits
            .iter()
            .filter(|record| record.path != relative)
            .cloned()
            .collect();

        let exists = virtual_fs::exists(&path).unwrap_or(false);
        if exists {
            if let Some(builtin) = crate::builtin_traits::by_path(relative) {
                if let Err(error) = crate::builtin_traits::sync_one(project_root, builtin) {
                    tracing::warn!("Could not register built-in trait '{relative}': {error:#}");
                }
            }
            let bytes = virtual_fs::read_file(&path)
                .with_context(|| format!("Failed to read trait asset '{}'", path.display()))?;
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(mut value) if valid_trait_declaration(&value) => {
                    // The same id elsewhere is a move if the old file is gone
                    // (the trait keeps its identity) and a copy if not.
                    if let Some(id) = embedded_id(&value) {
                        let moved_from: Vec<String> = traits
                            .iter()
                            .filter(|record| record.id == id)
                            .filter(|record| {
                                !virtual_fs::exists(&project_root.join(&record.path))
                                    .unwrap_or(true)
                            })
                            .map(|record| record.path.clone())
                            .collect();
                        traits.retain(|record| !moved_from.contains(&record.path));
                    }
                    let taken: BTreeSet<String> =
                        traits.iter().map(|record| record.id.clone()).collect();
                    let context = ScanContext::read(project_root);
                    let id = settle_identity(project_root, relative, &mut value, &taken, &context);
                    if let Some(mut record) = parse_trait_record(&value, relative, id) {
                        record.type_index_id = context.type_index_id(&record.name);
                        traits.push(record);
                    }
                }
                _ => tracing::warn!(path = %relative, "Skipping malformed trait declaration"),
            }
        }
        add_missing_builtins(&mut traits);
        traits.sort_by(|a, b| a.path.cmp(&b.path));
        if traits == before {
            return Ok(false);
        }

        // Blueprints reference ids; only their derived paths, and any
        // reference that now (un)resolves, move. No Blueprint file is read.
        let seed = TraitDb::build(traits.clone(), Vec::new());
        let blueprints = self
            .blueprints
            .iter()
            .map(|entry| reresolve(&seed, entry))
            .collect();
        *self = Self::assemble(traits, blueprints);
        Ok(true)
    }

    fn apply_blueprint(&mut self, project_root: &Path, relative: &str) -> Result<bool> {
        let path = project_root.join(relative);
        let entry = if virtual_fs::exists(&path).unwrap_or(false) {
            let bytes = virtual_fs::read_file(&path)
                .with_context(|| format!("Failed to read Blueprint asset '{}'", path.display()))?;
            match parse_declared(&bytes) {
                Some(declared) => Some(entry_for(&self.db, relative, &declared)),
                None => {
                    tracing::warn!(path = %relative, "Skipping malformed or unsupported Blueprint asset");
                    None
                }
            }
        } else {
            None
        };
        let mut blueprints = self.blueprints.clone();
        match blueprints.binary_search_by(|e| e.blueprint_path.as_str().cmp(relative)) {
            Ok(at) => match entry {
                Some(entry) if entry == blueprints[at] => return Ok(false),
                Some(entry) => blueprints[at] = entry,
                None => {
                    blueprints.remove(at);
                }
            },
            Err(at) => match entry {
                Some(entry) => blueprints.insert(at, entry),
                None => return Ok(false),
            },
        }
        *self = Self::assemble(self.traits.clone(), blueprints);
        Ok(true)
    }

    fn assemble(mut traits: Vec<TraitRecord>, mut blueprints: Vec<BlueprintTraitEntry>) -> Self {
        traits.sort_by(|a, b| a.path.cmp(&b.path));
        blueprints.sort_by(|a, b| a.blueprint_path.cmp(&b.blueprint_path));
        let blueprint_pos = blueprints
            .iter()
            .enumerate()
            .map(|(at, entry)| (entry.blueprint_path.clone(), at))
            .collect();
        let db = TraitDb::build(
            traits.clone(),
            blueprints
                .iter()
                .map(|entry| (entry.blueprint_path.clone(), entry.implemented_trait_ids.clone())),
        );
        Self {
            traits,
            blueprints,
            blueprint_pos,
            db,
        }
    }

    /// The resolved trait database.
    pub fn db(&self) -> &TraitDb {
        &self.db
    }

    /// Every trait of the project, in path order.
    pub fn traits(&self) -> &[TraitRecord] {
        &self.traits
    }

    /// The trait `reference` names: an id, a path, or a unique name.
    pub fn resolve_trait(&self, reference: &str) -> Option<&TraitRecord> {
        self.db.resolve(reference)
    }

    /// The indexed Blueprint at `blueprint_path`.
    pub fn entry(&self, blueprint_path: &str) -> Option<&BlueprintTraitEntry> {
        let path = normalize_relative_path(blueprint_path)?;
        self.blueprint_pos.get(&path).map(|&at| &self.blueprints[at])
    }

    /// Return all indexed Blueprint entries that implement the trait
    /// `reference` names, declared or inherited through a supertrait, in
    /// path order. `reference` is an id or a normalized project-relative
    /// asset path (a unique trait name also works).
    pub fn blueprints_for_trait(&self, reference: &str) -> Vec<&BlueprintTraitEntry> {
        self.db
            .implementers(reference)
            .into_iter()
            .filter_map(|implementation| self.entry(implementation.asset))
            .collect()
    }

    /// Return the paths of traits the Blueprint at `blueprint_path` declares.
    ///
    /// The lookup path is project-relative and accepts either slash style.
    pub fn traits_for_blueprint(&self, blueprint_path: &Path) -> Option<&[String]> {
        self.entry(&blueprint_path.to_string_lossy())
            .map(|entry| entry.implemented_traits.as_slice())
    }

    /// Iterate through indexed Blueprints in deterministic path order.
    pub fn blueprints(&self) -> &[BlueprintTraitEntry] {
        &self.blueprints
    }

    /// Publish the database to `pulsar_reflection`'s trait registry so
    /// plugins can query it. A no-op without the `editor` feature.
    #[allow(unused_variables)]
    pub fn publish(&self, project_root: &Path) {
        #[cfg(feature = "editor")]
        self.db.publish(project_root);
    }

    fn persist(&self, project_root: &Path) -> Result<()> {
        let path = project_root.join(INDEX_RELATIVE_PATH);
        let parent = path
            .parent()
            .context("Blueprint trait index path has no parent directory")?;
        virtual_fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create '{}'", parent.display()))?;
        let persisted = Persisted {
            schema_version: SCHEMA_VERSION,
            traits: self.traits.clone(),
            blueprints: self.blueprints.clone(),
        };
        let mut bytes = serde_json::to_vec_pretty(&persisted)
            .context("Failed to serialize Blueprint trait index")?;
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

    fn validate(traits: &[TraitRecord], blueprints: &[BlueprintTraitEntry]) -> Result<()> {
        let mut previous_trait: Option<&str> = None;
        let mut ids = BTreeSet::new();
        for record in traits {
            if record.path.starts_with(BUILTIN_PATH_PREFIX) {
                previous_trait = Some(&record.path);
                if record.id.trim().is_empty() || !ids.insert(record.id.as_str()) {
                    anyhow::bail!("Blueprint trait index trait ids are empty or duplicated");
                }
                continue;
            }
            let normalized = normalize_relative_path(&record.path)
                .context("Blueprint trait index contains an unsafe trait path")?;
            if normalized != record.path || !is_trait_asset(&record.path) {
                anyhow::bail!("Blueprint trait index contains a non-canonical trait path");
            }
            if previous_trait.is_some_and(|previous| previous >= record.path.as_str()) {
                anyhow::bail!("Blueprint trait index trait paths are duplicated or not sorted");
            }
            previous_trait = Some(&record.path);
            if record.id.trim().is_empty() || !ids.insert(record.id.as_str()) {
                anyhow::bail!("Blueprint trait index trait ids are empty or duplicated");
            }
        }

        let mut previous_blueprint: Option<&str> = None;
        for blueprint in blueprints {
            let normalized = normalize_relative_path(&blueprint.blueprint_path)
                .context("Blueprint trait index contains an unsafe Blueprint path")?;
            if normalized != blueprint.blueprint_path || !is_blueprint_asset(&blueprint.blueprint_path)
            {
                anyhow::bail!("Blueprint trait index contains a non-canonical Blueprint path");
            }
            if previous_blueprint
                .is_some_and(|previous| previous >= blueprint.blueprint_path.as_str())
            {
                anyhow::bail!("Blueprint trait index paths are duplicated or not sorted");
            }
            previous_blueprint = Some(&blueprint.blueprint_path);

            ensure_sorted_unique(&blueprint.implemented_trait_ids, "trait ids")?;
            ensure_sorted_unique(&blueprint.implemented_traits, "trait paths")?;
            for trait_path in &blueprint.implemented_traits {
                let normalized = normalize_relative_path(trait_path)
                    .context("Blueprint trait index contains an unsafe trait path")?;
                if normalized != *trait_path || !is_trait_asset(trait_path) {
                    anyhow::bail!("Blueprint trait index contains a non-canonical trait path");
                }
            }
        }
        Ok(())
    }
}

fn ensure_sorted_unique(values: &[String], what: &str) -> Result<()> {
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        anyhow::bail!("Blueprint trait index {what} are duplicated or not sorted");
    }
    Ok(())
}

impl ScanContext {
    fn read(project_root: &Path) -> Self {
        let mut context = Self::default();
        let manifest_path = project_root.join(ASSET_ID_MANIFEST);
        if let Some(manifest) = read_json(&manifest_path) {
            if let Some(assets) = manifest.get("assets").and_then(Value::as_object) {
                for (path, kinds) in assets {
                    if let Some(id) = kinds.get(TRAIT_ASSET_KIND).and_then(Value::as_str) {
                        context.manifest_ids.insert(path.clone(), id.to_owned());
                    }
                }
            }
        }
        if let Some(index) = read_json(&project_root.join(TYPE_INDEX_RELATIVE_PATH)) {
            if let Some(traits) = index.pointer("/types/traits").and_then(Value::as_object) {
                context.type_index_traits = traits.keys().cloned().collect();
            }
        }
        context
    }

    fn type_index_id(&self, name: &str) -> Option<String> {
        self.type_index_traits
            .contains(name)
            .then(|| format!("trait:{name}"))
    }
}

fn read_json(path: &Path) -> Option<Value> {
    if !virtual_fs::exists(path).unwrap_or(false) {
        return None;
    }
    serde_json::from_slice(&virtual_fs::read_file(path).ok()?).ok()
}

/// Decide the id of the trait asset `value` read from `relative`, rewriting
/// the asset to carry it when it did not (see the module docs). `taken` are
/// the ids of the other traits.
fn settle_identity(
    project_root: &Path,
    relative: &str,
    value: &mut Value,
    taken: &BTreeSet<String>,
    context: &ScanContext,
) -> String {
    if let Some(builtin) = crate::builtin_traits::by_path(relative) {
        // The engine defines this identity; `ensure` stamped the file, and if
        // that failed the database serves the engine's definition anyway.
        return builtin.id.to_owned();
    }
    let reserved = |id: &str| crate::builtin_traits::by_id(id).is_some();
    let embedded = embedded_id(value);
    if let Some(id) = &embedded {
        if !taken.contains(id) && !reserved(id) {
            return id.clone();
        }
        tracing::warn!(
            path = relative,
            "Trait id '{id}' is already used by another trait; this copy gets a new identity"
        );
    } else if let Some(id) = context.manifest_ids.get(relative) {
        if !taken.contains(id) && !reserved(id) {
            return id.clone();
        }
    }

    let id = uuid::Uuid::new_v4().to_string();
    if let Some(object) = value.as_object_mut() {
        object.insert("id".into(), json!(id));
        // A copy of an engine-owned trait is the project's own trait.
        if let Some(meta) = object.get_mut("meta").and_then(Value::as_object_mut) {
            meta.remove("builtin");
            meta.remove("contractVersion");
        }
    }
    match write_trait(project_root, relative, value) {
        Ok(()) => id,
        Err(error) => {
            tracing::warn!(
                path = relative,
                "Could not store an id in the trait asset ({error:#}); trying the asset-id manifest"
            );
            manifest_id(project_root, relative).unwrap_or_else(|| {
                tracing::warn!(
                    path = relative,
                    "Trait has no stored id; it is identified by its path until it can be written"
                );
                format!("path:{relative}")
            })
        }
    }
}

fn write_trait(project_root: &Path, relative: &str, value: &Value) -> Result<()> {
    let path = project_root.join(relative);
    let mut bytes = serde_json::to_vec_pretty(value).context("Failed to serialize trait")?;
    bytes.push(b'\n');
    virtual_fs::write_file(&path, &bytes)
        .with_context(|| format!("Failed to write trait asset '{}'", path.display()))?;
    mirror_to_file_index(project_root, relative, value);
    Ok(())
}

/// Record the id in the project file index so asset pickers can look the
/// trait up by kind. Best effort: the index is a disposable local cache.
#[cfg(feature = "editor")]
fn mirror_to_file_index(project_root: &Path, relative: &str, value: &Value) {
    let Some(id) = embedded_id(value) else {
        return;
    };
    let path = project_root.join(relative);
    if let Some(index) = crate::file_index::project_file_index_for_path(&path) {
        if let Err(error) = index.record_typed_asset_id(TRAIT_ASSET_KIND, &id, &path) {
            tracing::debug!(path = relative, "Could not mirror trait id: {error:#}");
        }
    }
}

#[cfg(not(feature = "editor"))]
fn mirror_to_file_index(_project_root: &Path, _relative: &str, _value: &Value) {}

/// An id from the durable asset-id manifest, minted by the project file
/// index when one is open (which also keeps it across moves).
#[cfg(feature = "editor")]
fn manifest_id(project_root: &Path, relative: &str) -> Option<String> {
    let path = project_root.join(relative);
    let index = crate::file_index::project_file_index_for_path(&path)?;
    index.ensure_typed_asset_id(TRAIT_ASSET_KIND, &path).ok()
}

#[cfg(not(feature = "editor"))]
fn manifest_id(_project_root: &Path, _relative: &str) -> Option<String> {
    None
}

/// The engine's built-ins that no file in the project provides (an
/// unwritable project): serve the engine's definition.
fn add_missing_builtins(records: &mut Vec<TraitRecord>) {
    for builtin in crate::builtin_traits::all() {
        if !records.iter().any(|record| record.id == builtin.id) {
            // Another trait is at the built-in's path: it keeps the path and
            // the built-in is served with no file.
            let mut record = builtin.to_record();
            if records.iter().any(|other| other.path == record.path) {
                record.path = format!("{BUILTIN_PATH_PREFIX}{}", builtin.name);
            }
            records.push(record);
        }
    }
    records.sort_by(|a, b| a.path.cmp(&b.path));
}

fn relative_to(project_root: &Path, path: &Path) -> Option<String> {
    let text = path.to_string_lossy().replace('\\', "/");
    let root = project_root.to_string_lossy().replace('\\', "/");
    let root = root.trim_end_matches('/');
    let stripped = if path.is_absolute() {
        let lowered = text.to_ascii_lowercase();
        if lowered.starts_with(&root.to_ascii_lowercase())
            && text[root.len()..].starts_with('/')
        {
            text[root.len() + 1..].to_owned()
        } else {
            return None;
        }
    } else {
        text
    };
    normalize_relative_path(&stripped)
}

/// What a Blueprint says about traits, or `None` for a malformed or
/// unsupported asset.
fn parse_declared(bytes: &[u8]) -> Option<Declared> {
    let value = crate::trait_records::parse_blueprint_json(bytes)?;
    declared_from_value(&value)
}

pub(crate) fn declared_from_value(value: &Value) -> Option<Declared> {
    if !BLUEPRINT_FORMAT_VERSIONS.contains(&value.get("format_version")?.as_u64()?) {
        return None;
    }
    if !value.get("main_graph")?.is_object() || !value.get("variables")?.is_array() {
        return None;
    }
    let metadata = value.get("blueprint_metadata").and_then(Value::as_object);
    let list = |key: &str| -> Option<Vec<String>> {
        let Some(raw) = metadata.and_then(|metadata| metadata.get(key)) else {
            return Some(Vec::new());
        };
        Some(
            raw.as_array()?
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
        )
    };
    Some(Declared {
        ids: list("implemented_trait_ids")?,
        paths: list("implemented_traits")?,
    })
}

/// The traits a Blueprint asset's bytes declare, resolved against `db` the
/// way the index does (ids first, then paths). `None` for a malformed or
/// unsupported asset. For a runtime that needs one class's declarations
/// without scanning the project.
pub fn declared_traits(db: &TraitDb, blueprint: &[u8]) -> Option<BlueprintTraitEntry> {
    let declared = parse_declared(blueprint)?;
    Some(entry_for(db, "", &declared))
}

/// Resolve what a Blueprint declares against the traits that exist.
pub(crate) fn entry_for(db: &TraitDb, relative: &str, declared: &Declared) -> BlueprintTraitEntry {
    let mut ids = BTreeSet::new();
    let mut unresolved = BTreeSet::new();
    for id in declared.ids.iter().map(|id| id.trim()).filter(|id| !id.is_empty()) {
        if db.get(id).is_some() {
            ids.insert(id.to_owned());
        } else {
            tracing::warn!(blueprint = relative, "Ignoring reference to missing trait id '{id}'");
            unresolved.insert(id.to_owned());
        }
    }
    // A stale path beside ids that all resolved is only the hint a rename
    // left behind; it is not a missing trait.
    let ids_cover_paths = !declared.ids.is_empty() && unresolved.is_empty();
    for raw in &declared.paths {
        let Some(path) = normalize_relative_path(raw).filter(|p| is_trait_asset(p)) else {
            tracing::warn!(blueprint = relative, "Ignoring invalid Blueprint trait reference");
            continue;
        };
        match db.id_for_path(&path) {
            Some(id) => {
                ids.insert(id.to_owned());
            }
            None if ids_cover_paths => {}
            None => {
                tracing::warn!(
                    blueprint = relative,
                    trait_path = %path,
                    "Ignoring missing or malformed Blueprint trait reference"
                );
                unresolved.insert(path);
            }
        }
    }
    finish_entry(
        db,
        relative,
        ids,
        unresolved,
        declared.ids.is_empty() && !declared.paths.is_empty(),
    )
}

/// An entry after the set of traits changed: ids that vanished become
/// unresolved, references that now name a trait resolve.
fn reresolve(db: &TraitDb, entry: &BlueprintTraitEntry) -> BlueprintTraitEntry {
    let mut ids = BTreeSet::new();
    let mut unresolved = BTreeSet::new();
    for reference in entry.implemented_trait_ids.iter().chain(&entry.unresolved) {
        if db.get(reference).is_some() {
            ids.insert(reference.clone());
        } else if let Some(id) = db.id_for_path(reference) {
            ids.insert(id.to_owned());
        } else {
            unresolved.insert(reference.clone());
        }
    }
    finish_entry(db, &entry.blueprint_path, ids, unresolved, entry.path_only)
}

fn finish_entry(
    db: &TraitDb,
    relative: &str,
    ids: BTreeSet<String>,
    unresolved: BTreeSet<String>,
    path_only: bool,
) -> BlueprintTraitEntry {
    let paths: BTreeSet<String> = ids
        .iter()
        .filter_map(|id| db.get(id))
        .map(|record| record.path.clone())
        .filter(|path| is_trait_asset(path))
        .collect();
    BlueprintTraitEntry {
        blueprint_path: relative.to_owned(),
        implemented_traits: paths.into_iter().collect(),
        implemented_trait_ids: ids.into_iter().collect(),
        unresolved: unresolved.into_iter().collect(),
        path_only,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_traits::{GAME_MODE_TRAIT_ID, GAME_MODE_TRAIT_PATH};

    fn trait_json(name: &str, id: Option<&str>) -> String {
        let mut value = json!({
            "schemaVersion": 1, "typeKind": "trait", "name": name, "displayName": name,
            "methods": [], "meta": {}
        });
        if let Some(id) = id {
            value["id"] = json!(id);
        }
        value.to_string()
    }

    fn blueprint_json(ids: &[&str], paths: &[&str]) -> String {
        json!({
            "format_version": 1,
            "main_graph": {},
            "variables": [],
            "blueprint_metadata": {
                "implemented_trait_ids": ids,
                "implemented_traits": paths,
            }
        })
        .to_string()
    }

    fn put(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn id_in_file(root: &Path, relative: &str) -> String {
        let value: Value =
            serde_json::from_slice(&std::fs::read(root.join(relative)).unwrap()).unwrap();
        value["id"].as_str().unwrap().to_owned()
    }

    #[test]
    fn legacy_path_references_resolve_and_traits_get_ids() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "types/traits/movable.trait.json", &trait_json("Movable", None));
        put(
            dir.path(),
            "classes/Player/graph_save.json",
            &blueprint_json(&[], &["types\\traits\\movable.trait.json"]),
        );
        let index = BlueprintTraitIndex::rebuild(dir.path()).unwrap();

        let id = id_in_file(dir.path(), "types/traits/movable.trait.json");
        let entry = index.entry("classes/Player/graph_save.json").unwrap();
        assert_eq!(entry.implemented_trait_ids, [id.as_str()]);
        assert_eq!(entry.implemented_traits, ["types/traits/movable.trait.json"]);
        assert!(entry.path_only, "path-only references are flagged for migration");
        assert!(entry.unresolved.is_empty());
        assert_eq!(
            index.blueprints_for_trait("types/traits/movable.trait.json").len(),
            1
        );
        assert_eq!(index.blueprints_for_trait(&id).len(), 1);
    }

    #[test]
    fn a_renamed_trait_keeps_its_implementers_and_a_second_rebuild_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "types/traits/movable.trait.json", &trait_json("Movable", Some("T-1")));
        put(
            dir.path(),
            "classes/Player/graph_save.json",
            &blueprint_json(&["T-1"], &["types/traits/movable.trait.json"]),
        );
        let first = BlueprintTraitIndex::rebuild(dir.path()).unwrap();
        assert!(!first.entry("classes/Player/graph_save.json").unwrap().path_only);

        std::fs::create_dir_all(dir.path().join("gameplay")).unwrap();
        std::fs::rename(
            dir.path().join("types/traits/movable.trait.json"),
            dir.path().join("gameplay/mobile.trait.json"),
        )
        .unwrap();
        let second = BlueprintTraitIndex::rebuild(dir.path()).unwrap();
        let entry = second.entry("classes/Player/graph_save.json").unwrap();
        assert_eq!(entry.implemented_trait_ids, ["T-1"]);
        assert_eq!(entry.implemented_traits, ["gameplay/mobile.trait.json"]);
        assert!(entry.unresolved.is_empty(), "the stale path hint is not a missing trait");
        assert_eq!(second.blueprints_for_trait("T-1").len(), 1);

        let third = BlueprintTraitIndex::rebuild(dir.path()).unwrap();
        assert_eq!(second, third);
        let a = std::fs::read(dir.path().join(BlueprintTraitIndex::RELATIVE_PATH)).unwrap();
        BlueprintTraitIndex::rebuild(dir.path()).unwrap();
        let b = std::fs::read(dir.path().join(BlueprintTraitIndex::RELATIVE_PATH)).unwrap();
        assert_eq!(a, b, "persisted output is deterministic");
    }

    #[test]
    fn copied_trait_files_get_distinct_identities() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "a.trait.json", &trait_json("Same", Some("dup")));
        put(dir.path(), "b.trait.json", &trait_json("Same", Some("dup")));
        let index = BlueprintTraitIndex::rebuild(dir.path()).unwrap();
        let a = index.traits().iter().find(|t| t.path == "a.trait.json").unwrap();
        let b = index.traits().iter().find(|t| t.path == "b.trait.json").unwrap();
        assert_eq!(a.id, "dup");
        assert_ne!(b.id, "dup");
        assert_eq!(id_in_file(dir.path(), "b.trait.json"), b.id);
    }

    #[test]
    fn missing_references_are_reported_not_resolved() {
        let dir = tempfile::tempdir().unwrap();
        put(
            dir.path(),
            "classes/A/graph_save.json",
            &blueprint_json(&["gone-id"], &["types/traits/gone.trait.json", "../escape.trait.json"]),
        );
        let index = BlueprintTraitIndex::rebuild(dir.path()).unwrap();
        let entry = index.entry("classes/A/graph_save.json").unwrap();
        assert!(entry.implemented_trait_ids.is_empty());
        assert_eq!(entry.unresolved, ["gone-id", "types/traits/gone.trait.json"]);
    }

    #[test]
    fn built_ins_are_present_with_fixed_ids_and_implementers_by_path_or_id() {
        let dir = tempfile::tempdir().unwrap();
        put(
            dir.path(),
            "classes/Mode/graph_save.json",
            &blueprint_json(&[], &[GAME_MODE_TRAIT_PATH]),
        );
        let index = BlueprintTraitIndex::rebuild(dir.path()).unwrap();
        assert_eq!(index.resolve_trait("GameMode").unwrap().id, GAME_MODE_TRAIT_ID);
        assert_eq!(index.blueprints_for_trait(GAME_MODE_TRAIT_PATH).len(), 1);
        assert_eq!(index.blueprints_for_trait(GAME_MODE_TRAIT_ID).len(), 1);
        assert_eq!(index.db().traits().filter(|t| t.builtin).count(), crate::builtin_traits::all().len());
    }

    #[test]
    fn supertraits_make_subtrait_implementers_implementers_of_the_base() {
        let dir = tempfile::tempdir().unwrap();
        let mut sub: Value = serde_json::from_str(&trait_json("MyMode", Some("sub"))).unwrap();
        sub["supertraits"] = json!([GAME_MODE_TRAIT_ID]);
        put(dir.path(), "types/traits/my_mode.trait.json", &sub.to_string());
        put(dir.path(), "classes/M/graph_save.json", &blueprint_json(&["sub"], &[]));
        let index = BlueprintTraitIndex::rebuild(dir.path()).unwrap();
        let modes = index.blueprints_for_trait(GAME_MODE_TRAIT_ID);
        assert_eq!(modes.len(), 1);
        assert_eq!(modes[0].blueprint_path, "classes/M/graph_save.json");
        assert!(index.db().implements("classes/M/graph_save.json", "GameMode"));
        // The Blueprint declared only the sub-trait.
        assert_eq!(modes[0].implemented_trait_ids, ["sub"]);
    }

    #[test]
    fn update_path_matches_a_full_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "types/traits/a.trait.json", &trait_json("A", Some("ta")));
        put(dir.path(), "classes/One/graph_save.json", &blueprint_json(&["ta"], &[]));
        BlueprintTraitIndex::rebuild(dir.path()).unwrap();

        // A new Blueprint, a changed Blueprint, a new trait it can now resolve.
        put(dir.path(), "classes/Two/graph_save.json", &blueprint_json(&["tb"], &[]));
        let updated =
            BlueprintTraitIndex::update_path(dir.path(), &dir.path().join("classes/Two/graph_save.json"))
                .unwrap();
        assert_eq!(updated.entry("classes/Two/graph_save.json").unwrap().unresolved, ["tb"]);

        put(dir.path(), "types/traits/b.trait.json", &trait_json("B", Some("tb")));
        let updated =
            BlueprintTraitIndex::update_path(dir.path(), Path::new("types/traits/b.trait.json")).unwrap();
        assert!(updated.entry("classes/Two/graph_save.json").unwrap().unresolved.is_empty());
        assert_eq!(updated, BlueprintTraitIndex::rebuild(dir.path()).unwrap());

        // Delete a Blueprint, then a trait.
        std::fs::remove_file(dir.path().join("classes/One/graph_save.json")).unwrap();
        let updated =
            BlueprintTraitIndex::update_path(dir.path(), Path::new("classes/One/graph_save.json")).unwrap();
        assert!(updated.entry("classes/One/graph_save.json").is_none());
        std::fs::remove_file(dir.path().join("types/traits/b.trait.json")).unwrap();
        let updated =
            BlueprintTraitIndex::update_path(dir.path(), Path::new("types/traits/b.trait.json")).unwrap();
        assert_eq!(updated.entry("classes/Two/graph_save.json").unwrap().unresolved, ["tb"]);
        assert_eq!(updated, BlueprintTraitIndex::rebuild(dir.path()).unwrap());
    }

    #[test]
    fn update_path_follows_a_moved_trait_without_losing_identity() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "types/traits/a.trait.json", &trait_json("A", Some("ta")));
        put(dir.path(), "classes/One/graph_save.json", &blueprint_json(&["ta"], &[]));
        BlueprintTraitIndex::rebuild(dir.path()).unwrap();

        std::fs::create_dir_all(dir.path().join("moved")).unwrap();
        std::fs::rename(
            dir.path().join("types/traits/a.trait.json"),
            dir.path().join("moved/a.trait.json"),
        )
        .unwrap();
        let updated =
            BlueprintTraitIndex::update_path(dir.path(), Path::new("moved/a.trait.json")).unwrap();
        assert_eq!(updated.resolve_trait("ta").unwrap().path, "moved/a.trait.json");
        assert_eq!(
            updated.entry("classes/One/graph_save.json").unwrap().implemented_traits,
            ["moved/a.trait.json"]
        );
        assert_eq!(updated, BlueprintTraitIndex::rebuild(dir.path()).unwrap());
    }

    #[test]
    fn lookups_accept_either_slash_style_and_reject_unsafe_paths() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "classes/One/graph_save.json", &blueprint_json(&[], &[]));
        let index = BlueprintTraitIndex::rebuild(dir.path()).unwrap();
        assert!(index.traits_for_blueprint(Path::new("classes\\One\\graph_save.json")).is_some());
        assert!(index.traits_for_blueprint(Path::new("../classes/One/graph_save.json")).is_none());
        assert!(index.blueprints_for_trait("../x.trait.json").is_empty());
    }

    #[test]
    fn load_rejects_an_older_schema() {
        let dir = tempfile::tempdir().unwrap();
        put(
            dir.path(),
            BlueprintTraitIndex::RELATIVE_PATH,
            r#"{"schema_version":1,"blueprints":[]}"#,
        );
        assert!(BlueprintTraitIndex::load(dir.path()).is_err());
    }

    #[test]
    fn the_type_index_is_cross_referenced() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "types/traits/a.trait.json", &trait_json("Named", Some("ta")));
        put(
            dir.path(),
            "type-index/index.json",
            r#"{"types":{"traits":{"Named":{}}}}"#,
        );
        let index = BlueprintTraitIndex::rebuild(dir.path()).unwrap();
        assert_eq!(index.resolve_trait("ta").unwrap().type_index_id.as_deref(), Some("trait:Named"));
    }

    #[test]
    fn blueprints_as_the_editor_saves_them_are_indexed() {
        // Format 2, preceded by the editor's `//` header comments.
        let dir = tempfile::tempdir().unwrap();
        let body = blueprint_json(&[], &[GAME_MODE_TRAIT_PATH]).replace("\"format_version\":1", "\"format_version\":2");
        put(
            dir.path(),
            "classes/Saved/graph_save.json",
            &format!("// Auto Generated by the Pulsar Blueprint Editor\n// DO NOT EDIT\n\n{body}"),
        );
        let index = BlueprintTraitIndex::rebuild(dir.path()).unwrap();
        assert_eq!(index.blueprints_for_trait(GAME_MODE_TRAIT_ID).len(), 1);
    }

    #[test]
    fn one_blueprints_declarations_resolve_without_an_index() {
        let db = TraitDb::builtin();
        let bytes = blueprint_json(&[GAME_MODE_TRAIT_ID, "missing"], &[]);
        let entry = declared_traits(&db, bytes.as_bytes()).unwrap();
        assert_eq!(entry.implemented_trait_ids, [GAME_MODE_TRAIT_ID]);
        assert_eq!(entry.unresolved, ["missing"]);
        assert!(declared_traits(&db, b"not json").is_none());
        let effective = db.with_ancestors(&entry.implemented_trait_ids);
        assert!(effective.contains(GAME_MODE_TRAIT_ID));
        assert_eq!(effective.len(), 1, "a built-in extends nothing");
    }
}
