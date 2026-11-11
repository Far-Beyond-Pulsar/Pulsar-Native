//! The one trait index of a project: every trait and every implementer, of
//! every source.
//!
//! Traits are `*.trait.json` files: the project's own, and those the engine
//! (`plugins/pulsar/`) and the enabled plugins (`plugins/<id>/`) provide into
//! their own folders (`engine_fs::virtual_fs::mounts`). Implementers come
//! from the registered [`TraitImplementerSource`](crate::TraitImplementerSource)s:
//! files a source claims (Blueprints) and implementers a source lists (native
//! components).
//!
//! What is stored, `.pulsar/trait_index.json` (schema 3), is only what the
//! project's own files say. Everything the engine and plugins bring (mounted
//! traits and implementers, listed implementers) is read again whenever the
//! index loads, so a plugin disabled since the last rebuild has no traits,
//! and its implementers' references show as "Missing trait".
//!
//! A trait's identity is its id, not its path:
//!
//! - the id is embedded in the trait asset (`"id"`). A project trait without
//!   one gets one when the index is rebuilt (a copy that duplicates another's
//!   id gets a fresh one), falling back to the project's durable asset-id
//!   manifest (`.pulsar/asset_ids.json`, kind `trait`) when the file cannot be
//!   rewritten;
//! - implementers reference ids. A path reference still resolves (an asset
//!   from before ids is "path only" until it is next saved), and a renamed or
//!   moved trait keeps its implementers;
//! - **namespaces**: a mounted trait's id is its owner's, `pulsar.<Trait>` in
//!   the engine's folder and `<plugin id>.<Trait>` in a plugin's, with a
//!   `meta.contractVersion`; a mounted trait outside its namespace, and a
//!   project trait claiming a reserved one (an old project's copy of a
//!   built-in), are rejected as [`problems`](TraitIndex::problems), not
//!   indexed.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use engine_fs::virtual_fs;

use crate::db::{Declaration, TraitDb};
use crate::records::{
    embedded_id, is_trait_asset, normalize_relative_path, parse_trait_record,
    valid_trait_declaration, TraitRecord,
};
use crate::sources::{self, Binding, Declared, Scanned};

const INDEX_RELATIVE_PATH: &str = ".pulsar/trait_index.json";
const ASSET_ID_MANIFEST: &str = ".pulsar/asset_ids.json";
const TYPE_INDEX_RELATIVE_PATH: &str = "type-index/index.json";
const SCHEMA_VERSION: u32 = 3;
/// Kind under which trait ids are recorded in the asset-id systems.
pub const TRAIT_ASSET_KIND: &str = "trait";
/// The engine's trait namespace (`pulsar.GameMode`).
pub const ENGINE_NAMESPACE: &str = "pulsar";

/// One implementer and the traits it declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImplementerEntry {
    /// A project-relative asset path, or `component:<Class>`.
    pub id: String,
    /// Its source's kind.
    pub kind: String,
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
    /// it with its editor writes ids and clears this.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub path_only: bool,
    /// How the runtime reaches it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<Binding>,
}

#[derive(Serialize, Deserialize)]
struct Persisted {
    schema_version: u32,
    traits: Vec<TraitRecord>,
    implementers: Vec<ImplementerEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    problems: Vec<String>,
}

/// The project's part of the index: what its own files say.
#[derive(Debug, Clone, Default, PartialEq)]
struct ProjectPart {
    traits: Vec<TraitRecord>,
    implementers: Vec<ImplementerEntry>,
    problems: Vec<String>,
}

/// A project's traits and implementers (see the module docs).
#[derive(Debug, Clone)]
pub struct TraitIndex {
    project: ProjectPart,
    traits: Vec<TraitRecord>,
    implementers: Vec<ImplementerEntry>,
    implementer_pos: HashMap<String, usize>,
    problems: Vec<String>,
    db: TraitDb,
}

impl PartialEq for TraitIndex {
    fn eq(&self, other: &Self) -> bool {
        self.traits == other.traits && self.implementers == other.implementers
    }
}

/// Source facts read once per full scan.
#[derive(Default)]
struct ScanContext {
    /// Path -> id from the durable asset-id manifest, kind `trait`.
    manifest_ids: HashMap<String, String>,
    /// Trait names listed by the type system's `type-index/index.json`.
    type_index_traits: BTreeSet<String>,
}

/// The owner whose folder a project-relative path is in (`pulsar` for the
/// engine, a plugin's id), or `None` for the project's own files.
pub fn mount_owner(relative: &str) -> Option<String> {
    let (first, rest) = relative.split_once('/')?;
    if first == virtual_fs::PLUGINS_FOLDER {
        return rest.split_once('/').map(|(id, _)| id.to_owned());
    }
    None
}

/// The namespace a project trait may not use that `id` is in: the engine's
/// or an installed plugin's.
fn reserved_namespace(id: &str) -> Option<String> {
    if id
        .strip_prefix(ENGINE_NAMESPACE)
        .is_some_and(|rest| rest.starts_with('.'))
    {
        return Some(ENGINE_NAMESPACE.to_owned());
    }
    pulsar_plugins::installed()
        .iter()
        .find(|plugin| {
            id.strip_prefix(plugin.id.as_str())
                .is_some_and(|rest| rest.starts_with('.'))
        })
        .map(|plugin| plugin.id.clone())
}

impl TraitIndex {
    /// Path of the stored index relative to a project root.
    pub const RELATIVE_PATH: &'static str = INDEX_RELATIVE_PATH;

    /// Load the stored index and add what the engine and the enabled plugins
    /// bring now. Rejects an unsupported or malformed schema.
    pub fn load(project_root: &Path) -> Result<Self> {
        virtual_fs::mount_project(project_root);
        let path = project_root.join(INDEX_RELATIVE_PATH);
        let bytes = virtual_fs::read_file(&path)
            .with_context(|| format!("Failed to read the trait index at '{}'", path.display()))?;
        let persisted: Persisted = serde_json::from_slice(&bytes)
            .with_context(|| format!("Malformed trait index at '{}'", path.display()))?;
        if persisted.schema_version != SCHEMA_VERSION {
            anyhow::bail!(
                "Unsupported trait index schema version {} (expected {})",
                persisted.schema_version,
                SCHEMA_VERSION
            );
        }
        let project = ProjectPart {
            traits: persisted.traits,
            implementers: persisted.implementers,
            problems: persisted.problems,
        };
        validate(&project)?;
        let index = Self::assemble(project_root, project);
        index.publish(project_root);
        Ok(index)
    }

    /// What the engine and the enabled plugins bring alone, without the
    /// project's files: the index of a game whose content ships none.
    pub fn live(project_root: &Path) -> Self {
        virtual_fs::mount_project(project_root);
        Self::assemble(project_root, ProjectPart::default())
    }

    /// Rebuild the index from every file of the project and store it.
    ///
    /// Malformed assets and references to missing or malformed traits are
    /// skipped with a warning. A failed manifest or asset read aborts before
    /// replacing the stored index. Project trait assets without an id are
    /// given one (see the module docs).
    pub fn rebuild(project_root: &Path) -> Result<Self> {
        virtual_fs::mount_project(project_root);
        let manifest = virtual_fs::manifest(project_root)
            .with_context(|| format!("Failed to scan project '{}'", project_root.display()))?;
        let files: BTreeSet<String> = manifest
            .into_iter()
            .filter(|entry| !entry.is_dir)
            .filter_map(|entry| normalize_relative_path(&entry.path))
            .filter(|path| mount_owner(path).is_none())
            .collect();

        let context = ScanContext::read(project_root);
        let mut part = ProjectPart::default();
        let mut taken_ids: BTreeSet<String> = live_traits(project_root)
            .0
            .into_iter()
            .map(|record| record.id)
            .collect();
        for relative in files.iter().filter(|path| is_trait_asset(path)) {
            let path = project_root.join(relative);
            let content = virtual_fs::read_file(&path)
                .with_context(|| format!("Failed to read trait asset '{}'", path.display()))?;
            match project_trait(project_root, relative, &content, &taken_ids, &context) {
                Ok(Some(record)) => {
                    taken_ids.insert(record.id.clone());
                    part.traits.push(record);
                }
                Ok(None) => {}
                Err(problem) => part.problems.push(problem),
            }
        }

        let seed = Self::assemble(
            project_root,
            ProjectPart {
                traits: part.traits.clone(),
                ..ProjectPart::default()
            },
        );
        for relative in &files {
            let Some(source) = sources::source_for(relative) else {
                continue;
            };
            let path = project_root.join(relative);
            let content = virtual_fs::read_file(&path)
                .with_context(|| format!("Failed to read '{}'", path.display()))?;
            let Some(scanned) = source.scan.and_then(|scan| scan(relative, &content)) else {
                tracing::warn!(path = %relative, kind = source.kind, "Skipping malformed or unsupported implementer");
                continue;
            };
            part.implementers
                .push(entry_for(&seed.db, source.kind, relative, scanned));
        }

        let index = Self::assemble(project_root, part);
        index.persist(project_root)?;
        index.publish(project_root);
        Ok(index)
    }

    /// Load a usable index by rebuilding from source assets.
    ///
    /// Rebuilding every time is intentional: filesystem providers do not
    /// expose a consistent content timestamp contract, so trusting a cache
    /// could silently return stale selections. After a single known change
    /// (a save), [`update_path`](Self::update_path) is cheaper.
    pub fn load_or_rebuild(project_root: &Path) -> Result<Self> {
        Self::rebuild(project_root)
    }

    /// Bring the stored index up to date after `path` (an implementer or a
    /// trait asset, absolute or project-relative) was saved, moved or
    /// deleted, re-reading only that file. Falls back to a full
    /// [`rebuild`](Self::rebuild) when there is no usable index to update.
    ///
    /// This trusts the stored index to describe every *other* file. Use it
    /// from a save hook; use `rebuild` to open a project, since changes made
    /// outside the editor (version control, a file manager) are only seen by
    /// a full scan.
    pub fn update_path(project_root: &Path, path: &Path) -> Result<Self> {
        let mut index = match Self::load(project_root) {
            Ok(index) => index,
            Err(error) => {
                tracing::debug!("Rebuilding the trait index instead of updating: {error:#}");
                return Self::rebuild(project_root);
            }
        };
        if index.apply_path(project_root, path)? {
            index.persist(project_root)?;
            index.publish(project_root);
        }
        Ok(index)
    }

    /// Apply a change to one project file in memory. Returns whether the
    /// index changed. Paths that are neither traits nor claimed by a source,
    /// and mounted paths (they never change), are ignored.
    pub fn apply_path(&mut self, project_root: &Path, path: &Path) -> Result<bool> {
        let Some(relative) = relative_to(project_root, path) else {
            return Ok(false);
        };
        if mount_owner(&relative).is_some() {
            return Ok(false);
        }
        if is_trait_asset(&relative) {
            self.apply_trait(project_root, &relative)
        } else if let Some(source) = sources::source_for(&relative) {
            self.apply_implementer(project_root, source, &relative)
        } else {
            Ok(false)
        }
    }

    fn apply_trait(&mut self, project_root: &Path, relative: &str) -> Result<bool> {
        let path = project_root.join(relative);
        let before = self.project.clone();
        let mut part = self.project.clone();
        part.traits.retain(|record| record.path != relative);
        let prefix = format!("{relative}: ");
        part.problems
            .retain(|problem| !problem.starts_with(&prefix));

        if virtual_fs::exists(&path).unwrap_or(false) {
            let bytes = virtual_fs::read_file(&path)
                .with_context(|| format!("Failed to read trait asset '{}'", path.display()))?;
            // The same id elsewhere is a move if the old file is gone (the
            // trait keeps its identity) and a copy if not.
            if let Some(id) = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .as_ref()
                .and_then(embedded_id)
            {
                part.traits.retain(|record| {
                    record.id != id
                        || virtual_fs::exists(&project_root.join(&record.path)).unwrap_or(true)
                });
            }
            let taken: BTreeSet<String> = part
                .traits
                .iter()
                .map(|record| record.id.clone())
                .chain(live_traits(project_root).0.into_iter().map(|r| r.id))
                .collect();
            let context = ScanContext::read(project_root);
            match project_trait(project_root, relative, &bytes, &taken, &context) {
                Ok(Some(record)) => part.traits.push(record),
                Ok(None) => {}
                Err(problem) => part.problems.push(problem),
            }
        }
        part.traits.sort_by(|a, b| a.path.cmp(&b.path));
        if part == before {
            return Ok(false);
        }

        // Implementers reference ids; only their derived paths, and any
        // reference that now (un)resolves, move. No implementer file is read.
        let seed = Self::assemble(
            project_root,
            ProjectPart {
                traits: part.traits.clone(),
                ..ProjectPart::default()
            },
        );
        part.implementers = part
            .implementers
            .iter()
            .map(|entry| reresolve(&seed.db, entry))
            .collect();
        *self = Self::assemble(project_root, part);
        Ok(true)
    }

    fn apply_implementer(
        &mut self,
        project_root: &Path,
        source: &sources::TraitImplementerSource,
        relative: &str,
    ) -> Result<bool> {
        let path = project_root.join(relative);
        let entry = if virtual_fs::exists(&path).unwrap_or(false) {
            let bytes = virtual_fs::read_file(&path)
                .with_context(|| format!("Failed to read '{}'", path.display()))?;
            match source.scan.and_then(|scan| scan(relative, &bytes)) {
                Some(scanned) => Some(entry_for(&self.db, source.kind, relative, scanned)),
                None => {
                    tracing::warn!(path = %relative, kind = source.kind, "Skipping malformed or unsupported implementer");
                    None
                }
            }
        } else {
            None
        };
        let mut part = self.project.clone();
        match part
            .implementers
            .binary_search_by(|e| e.id.as_str().cmp(relative))
        {
            Ok(at) => match entry {
                Some(entry) if entry == part.implementers[at] => return Ok(false),
                Some(entry) => part.implementers[at] = entry,
                None => {
                    part.implementers.remove(at);
                }
            },
            Err(at) => match entry {
                Some(entry) => part.implementers.insert(at, entry),
                None => return Ok(false),
            },
        }
        *self = Self::assemble(project_root, part);
        Ok(true)
    }

    /// The project's part plus what the engine and plugins bring now.
    fn assemble(project_root: &Path, mut project: ProjectPart) -> Self {
        project.traits.sort_by(|a, b| a.path.cmp(&b.path));
        project.implementers.sort_by(|a, b| a.id.cmp(&b.id));
        let (live, mut problems) = live_traits(project_root);
        let mut traits = project.traits.clone();
        traits.extend(live);
        traits.sort_by(|a, b| a.path.cmp(&b.path));

        let seed = TraitDb::build(traits.clone(), Vec::new());
        let mut implementers = project.implementers.clone();
        implementers.extend(live_implementers(project_root, &seed));
        implementers.sort_by(|a, b| a.id.cmp(&b.id));
        implementers.dedup_by(|a, b| a.id == b.id);
        let implementer_pos = implementers
            .iter()
            .enumerate()
            .map(|(at, entry)| (entry.id.clone(), at))
            .collect();
        let db = TraitDb::build(
            traits.clone(),
            implementers.iter().map(|entry| Declaration {
                implementer: entry.id.clone(),
                kind: entry.kind.clone(),
                trait_ids: entry.implemented_trait_ids.clone(),
            }),
        );
        problems.extend(project.problems.iter().cloned());
        for (implementer, issue) in sources::native_conformance(&db) {
            problems.push(format!("{implementer}: {}", issue.message));
        }
        Self {
            project,
            traits,
            implementers,
            implementer_pos,
            problems,
            db,
        }
    }

    /// The resolved trait database.
    pub fn db(&self) -> &TraitDb {
        &self.db
    }

    /// Every trait, in path order.
    pub fn traits(&self) -> &[TraitRecord] {
        &self.traits
    }

    /// The trait `reference` names: an id, a path, or a unique name.
    pub fn resolve_trait(&self, reference: &str) -> Option<&TraitRecord> {
        self.db.resolve(reference)
    }

    /// The implementer with this id (a project-relative path or
    /// `component:<Class>`).
    pub fn entry(&self, id: &str) -> Option<&ImplementerEntry> {
        let id = normalize_relative_path(id)?;
        self.implementer_pos
            .get(&id)
            .map(|&at| &self.implementers[at])
    }

    /// The implementer the runtime reaches through `binding`.
    pub fn entry_for_binding(&self, binding: &Binding) -> Option<&ImplementerEntry> {
        self.implementers
            .iter()
            .find(|entry| entry.binding.as_ref() == Some(binding))
    }

    /// Every implementer of the trait `reference` names, declared or
    /// inherited through a supertrait, in id order. `reference` is an id or a
    /// project-relative trait path (a unique trait name also works).
    pub fn implementers_of(&self, reference: &str) -> Vec<&ImplementerEntry> {
        self.db
            .implementers(reference)
            .into_iter()
            .filter_map(|implementation| self.entry(implementation.asset))
            .collect()
    }

    /// The paths of the traits the implementer `id` declares.
    pub fn traits_for(&self, id: &str) -> Option<&[String]> {
        self.entry(id)
            .map(|entry| entry.implemented_traits.as_slice())
    }

    /// Every implementer, in id order.
    pub fn implementers(&self) -> &[ImplementerEntry] {
        &self.implementers
    }

    /// What the index refused, one message each: mounted traits outside
    /// their namespace or without a contract version, project traits in a
    /// reserved namespace, native implementers that do not conform.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// Publish the database to `pulsar_reflection`'s trait registry so
    /// plugins can query it.
    pub fn publish(&self, project_root: &Path) {
        self.db.publish(project_root);
    }

    fn persist(&self, project_root: &Path) -> Result<()> {
        let path = project_root.join(INDEX_RELATIVE_PATH);
        let parent = path
            .parent()
            .context("Trait index path has no parent directory")?;
        virtual_fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create '{}'", parent.display()))?;
        let persisted = Persisted {
            schema_version: SCHEMA_VERSION,
            traits: self.project.traits.clone(),
            implementers: self.project.implementers.clone(),
            problems: self.project.problems.clone(),
        };
        let mut bytes =
            serde_json::to_vec_pretty(&persisted).context("Failed to serialize the trait index")?;
        bytes.push(b'\n');
        replace_file(&path, &bytes)
            .with_context(|| format!("Failed to write derived index '{}'", path.display()))
    }
}

/// Replace `path` with `bytes` atomically, so an interrupted write cannot
/// leave it truncated. Remote and P2P providers have no atomic publish
/// operation yet and write directly.
fn replace_file(path: &Path, bytes: &[u8]) -> Result<()> {
    match virtual_fs::write_file_atomically(path, bytes) {
        Ok(()) => Ok(()),
        Err(error) if virtual_fs::is_remote() => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "Provider has no atomic write; writing directly"
            );
            virtual_fs::write_file(path, bytes)
        }
        Err(error) => Err(error),
    }
}

/// A stored project part must describe only project files, canonically.
fn validate(part: &ProjectPart) -> Result<()> {
    let mut previous: Option<&str> = None;
    let mut ids = BTreeSet::new();
    for record in &part.traits {
        let normalized = normalize_relative_path(&record.path)
            .context("The trait index contains an unsafe trait path")?;
        if normalized != record.path || !is_trait_asset(&record.path) {
            anyhow::bail!("The trait index contains a non-canonical trait path");
        }
        if previous.is_some_and(|previous| previous >= record.path.as_str()) {
            anyhow::bail!("The trait index's trait paths are duplicated or not sorted");
        }
        previous = Some(&record.path);
        if record.id.trim().is_empty() || !ids.insert(record.id.as_str()) {
            anyhow::bail!("The trait index's trait ids are empty or duplicated");
        }
    }
    let mut previous: Option<&str> = None;
    for entry in &part.implementers {
        let normalized = normalize_relative_path(&entry.id)
            .context("The trait index contains an unsafe implementer path")?;
        if normalized != entry.id {
            anyhow::bail!("The trait index contains a non-canonical implementer path");
        }
        if previous.is_some_and(|previous| previous >= entry.id.as_str()) {
            anyhow::bail!("The trait index's implementers are duplicated or not sorted");
        }
        previous = Some(&entry.id);
        ensure_sorted_unique(&entry.implemented_trait_ids, "trait ids")?;
        ensure_sorted_unique(&entry.implemented_traits, "trait paths")?;
    }
    Ok(())
}

fn ensure_sorted_unique(values: &[String], what: &str) -> Result<()> {
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        anyhow::bail!("The trait index's {what} are duplicated or not sorted");
    }
    Ok(())
}

/// The mounted traits of a project (the engine's and the enabled plugins'),
/// and the problems with them. Read through the VFS, which serves them from
/// memory.
fn live_traits(project_root: &Path) -> (Vec<TraitRecord>, Vec<String>) {
    let mut records = Vec::new();
    let mut problems = Vec::new();
    for mount in [virtual_fs::PLUGINS_FOLDER] {
        let Ok(entries) = virtual_fs::manifest(&project_root.join(mount)) else {
            continue;
        };
        for entry in entries.into_iter().filter(|entry| !entry.is_dir) {
            let relative = format!("{mount}/{}", entry.path);
            if !is_trait_asset(&relative) {
                continue;
            }
            match mounted_trait(project_root, &relative) {
                Ok(record) => records.push(record),
                Err(problem) => problems.push(problem),
            }
        }
    }
    (records, problems)
}

fn mounted_trait(project_root: &Path, relative: &str) -> std::result::Result<TraitRecord, String> {
    let owner = mount_owner(relative).unwrap_or_default();
    let bytes = virtual_fs::read_file(&project_root.join(relative))
        .map_err(|error| format!("{relative}: unreadable: {error:#}"))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{relative}: malformed trait declaration: {error}"))?;
    let id = embedded_id(&value)
        .ok_or_else(|| format!("{relative}: a mounted trait must carry its id"))?;
    let name_part = id
        .strip_prefix(owner.as_str())
        .and_then(|rest| rest.strip_prefix('.'));
    if !name_part.is_some_and(|name| !name.is_empty() && !name.contains('.')) {
        return Err(format!(
            "{relative}: trait id '{id}' is outside its owner's namespace '{owner}.<Trait>'; it is not indexed"
        ));
    }
    let mut record = parse_trait_record(&value, relative, id.clone())
        .ok_or_else(|| format!("{relative}: malformed trait declaration"))?;
    if record.contract_version == 0 {
        return Err(format!(
            "{relative}: trait '{id}' has no `meta.contractVersion`; a mounted trait is a contract and must version it"
        ));
    }
    // Served read-only: the method contract is its owner's.
    record.builtin = true;
    Ok(record)
}

/// Implementers the engine and plugins bring: mounted files a source claims,
/// and listed ones.
fn live_implementers(project_root: &Path, db: &TraitDb) -> Vec<ImplementerEntry> {
    let mut out = Vec::new();
    for mount in [virtual_fs::PLUGINS_FOLDER] {
        let Ok(entries) = virtual_fs::manifest(&project_root.join(mount)) else {
            continue;
        };
        for entry in entries.into_iter().filter(|entry| !entry.is_dir) {
            let relative = format!("{mount}/{}", entry.path);
            let Some(source) = sources::source_for(&relative) else {
                continue;
            };
            let Ok(bytes) = virtual_fs::read_file(&project_root.join(&relative)) else {
                continue;
            };
            if let Some(scanned) = source.scan.and_then(|scan| scan(&relative, &bytes)) {
                out.push(entry_for(db, source.kind, &relative, scanned));
            }
        }
    }
    for source in sources::sources() {
        let Some(list) = source.list else {
            continue;
        };
        for scanned in list() {
            let id = scanned.id.clone();
            out.push(entry_for(db, source.kind, &id, scanned));
        }
    }
    out
}

/// Read one project trait asset: its record, `Ok(None)` for one that is not
/// a declaration, `Err` for one the index refuses.
fn project_trait(
    project_root: &Path,
    relative: &str,
    content: &[u8],
    taken: &BTreeSet<String>,
    context: &ScanContext,
) -> std::result::Result<Option<TraitRecord>, String> {
    let Ok(mut value) = serde_json::from_slice::<Value>(content) else {
        tracing::warn!(path = %relative, "Skipping malformed trait declaration");
        return Ok(None);
    };
    if !valid_trait_declaration(&value) {
        tracing::warn!(path = %relative, "Skipping malformed trait declaration");
        return Ok(None);
    }
    if let Some(id) = embedded_id(&value) {
        if let Some(namespace) = reserved_namespace(&id) {
            return Err(format!(
                "{relative}: trait id '{id}' is in the reserved namespace '{namespace}' (the engine and plugins provide their traits in their own folders, `plugins/<id>/`); this file is ignored. An old project's copy of a built-in trait can be deleted"
            ));
        }
    }
    let id = settle_identity(project_root, relative, &mut value, taken, context);
    mirror_to_file_index(project_root, relative, &value);
    Ok(parse_trait_record(&value, relative, id).map(|mut record| {
        record.type_index_id = context.type_index_id(&record.name);
        // Only a mount serves engine- or plugin-owned contracts.
        record.builtin = false;
        record
    }))
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

/// Decide the id of the project trait asset `value` read from `relative`,
/// rewriting the asset to carry it when it did not (see the module docs).
/// `taken` are the ids of the other traits.
fn settle_identity(
    project_root: &Path,
    relative: &str,
    value: &mut Value,
    taken: &BTreeSet<String>,
    context: &ScanContext,
) -> String {
    let usable = |id: &str| !taken.contains(id) && reserved_namespace(id).is_none();
    if let Some(id) = embedded_id(value) {
        if usable(&id) {
            return id;
        }
        tracing::warn!(
            path = relative,
            "Trait id '{id}' is already used by another trait; this copy gets a new identity"
        );
    } else if let Some(id) = context.manifest_ids.get(relative) {
        if usable(id) {
            return id.clone();
        }
    }

    let id = uuid::Uuid::new_v4().to_string();
    if let Some(object) = value.as_object_mut() {
        object.insert("id".into(), json!(id));
        // A copy of a mounted trait is the project's own trait.
        if let Some(meta) = object.get_mut("meta").and_then(Value::as_object_mut) {
            meta.remove("builtin");
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
    // The trait asset is the user's data, rewritten during an index rebuild.
    replace_file(&path, &bytes)
        .with_context(|| format!("Failed to write trait asset '{}'", path.display()))?;
    mirror_to_file_index(project_root, relative, value);
    Ok(())
}

/// Record the id in the project file index so asset pickers can look the
/// trait up by kind. Best effort: the index is a disposable local cache.
fn mirror_to_file_index(project_root: &Path, relative: &str, value: &Value) {
    let Some(id) = embedded_id(value) else {
        return;
    };
    let path = project_root.join(relative);
    if let Some(index) = engine_fs::project_file_index_for_path(&path) {
        if let Err(error) = index.record_typed_asset_id(TRAIT_ASSET_KIND, &id, &path) {
            tracing::debug!(path = relative, "Could not mirror trait id: {error:#}");
        }
    }
}

/// An id from the durable asset-id manifest, minted by the project file
/// index when one is open (which also keeps it across moves).
fn manifest_id(project_root: &Path, relative: &str) -> Option<String> {
    let path = project_root.join(relative);
    let index = engine_fs::project_file_index_for_path(&path)?;
    index.ensure_typed_asset_id(TRAIT_ASSET_KIND, &path).ok()
}

fn relative_to(project_root: &Path, path: &Path) -> Option<String> {
    let text = path.to_string_lossy().replace('\\', "/");
    let root = project_root.to_string_lossy().replace('\\', "/");
    let root = root.trim_end_matches('/');
    let stripped = if path.is_absolute() {
        let lowered = text.to_ascii_lowercase();
        if lowered.starts_with(&root.to_ascii_lowercase()) && text[root.len()..].starts_with('/') {
            text[root.len() + 1..].to_owned()
        } else {
            return None;
        }
    } else {
        text
    };
    normalize_relative_path(&stripped)
}

/// What an implementer declares, resolved against `db` the way the index
/// does (ids first, then paths): for a source, an editor or a runtime that
/// needs one implementer's declarations without scanning the project.
pub fn resolve_declared(db: &TraitDb, declared: &Declared) -> ImplementerEntry {
    entry_for(
        db,
        "",
        "",
        Scanned {
            id: String::new(),
            declared: declared.clone(),
            binding: None,
        },
    )
}

/// Resolve what an implementer declares against the traits that exist.
fn entry_for(db: &TraitDb, kind: &str, relative: &str, scanned: Scanned) -> ImplementerEntry {
    let declared = &scanned.declared;
    let mut ids = BTreeSet::new();
    let mut unresolved = BTreeSet::new();
    for id in declared
        .ids
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
    {
        if db.get(id).is_some() {
            ids.insert(id.to_owned());
        } else {
            tracing::warn!(
                implementer = relative,
                "Ignoring reference to missing trait id '{id}'"
            );
            unresolved.insert(id.to_owned());
        }
    }
    // A stale path beside ids that all resolved is only the hint a rename
    // left behind; it is not a missing trait.
    let ids_cover_paths = !declared.ids.is_empty() && unresolved.is_empty();
    for raw in &declared.paths {
        let Some(path) = normalize_relative_path(raw).filter(|p| is_trait_asset(p)) else {
            tracing::warn!(implementer = relative, "Ignoring invalid trait reference");
            continue;
        };
        match db.id_for_path(&path) {
            Some(id) => {
                ids.insert(id.to_owned());
            }
            None if ids_cover_paths => {}
            None => {
                tracing::warn!(implementer = relative, trait_path = %path, "Ignoring missing or malformed trait reference");
                unresolved.insert(path);
            }
        }
    }
    let path_only = declared.ids.is_empty() && !declared.paths.is_empty();
    finish_entry(
        db,
        ImplementerEntry {
            id: if relative.is_empty() {
                scanned.id.clone()
            } else {
                relative.to_owned()
            },
            kind: kind.to_owned(),
            implemented_traits: Vec::new(),
            implemented_trait_ids: Vec::new(),
            unresolved: Vec::new(),
            path_only,
            binding: scanned.binding,
        },
        ids,
        unresolved,
    )
}

/// An entry after the set of traits changed: ids that vanished become
/// unresolved, references that now name a trait resolve.
fn reresolve(db: &TraitDb, entry: &ImplementerEntry) -> ImplementerEntry {
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
    finish_entry(db, entry.clone(), ids, unresolved)
}

fn finish_entry(
    db: &TraitDb,
    mut entry: ImplementerEntry,
    ids: BTreeSet<String>,
    unresolved: BTreeSet<String>,
) -> ImplementerEntry {
    let paths: BTreeSet<String> = ids
        .iter()
        .filter_map(|id| db.get(id))
        .map(|record| record.path.clone())
        .filter(|path| is_trait_asset(path))
        .collect();
    entry.implemented_traits = paths.into_iter().collect();
    entry.implemented_trait_ids = ids.into_iter().collect();
    entry.unresolved = unresolved.into_iter().collect();
    entry
}

#[cfg(test)]
mod tests;
