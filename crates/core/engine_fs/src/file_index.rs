//! Persistent, disposable index of project files.
//!
//! This index stores provider-independent project-relative paths and basic
//! metadata. The SQLite file is local-only cache data under `.pulsar/cache/`;
//! remote providers can continue to use their own manifest/query mechanisms.

use anyhow::{Context, Result};
use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::events::{FsChangeKind, FsEventSource};
use crate::providers::ManifestEntry;

const SCHEMA_VERSION: i64 = 3;
const ASSET_ID_MANIFEST: &str = ".pulsar/asset_ids.json";
static ASSET_ID_MANIFEST_LOCK: OnceLock<parking_lot::Mutex<()>> = OnceLock::new();

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct DurableAssetIdManifest {
    version: u32,
    /// Project-relative path -> asset kind -> stable ID.
    assets: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFile {
    /// Forward-slash-separated path relative to the project root.
    pub path: String,
    pub name: String,
    pub extension: Option<String>,
    pub file_type_id: Option<String>,
    pub is_dir: bool,
    pub size: u64,
    pub created: Option<i64>,
    pub modified: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileIndexSort {
    Path,
    Name,
    Extension,
    FileType,
    Size,
    Created,
    Modified,
}

#[derive(Debug, Clone, Default)]
pub struct FileIndexQuery {
    pub file_type_id: Option<String>,
    pub extension: Option<String>,
    /// Inclusive lower bound, Unix seconds.
    pub modified_after: Option<i64>,
    /// Inclusive upper bound, Unix seconds.
    pub modified_before: Option<i64>,
    pub name_contains: Option<String>,
    /// Match this folder and all descendants. Empty means the project root.
    pub folder: Option<String>,
    /// Restrict results to direct children of this folder (for file listings).
    pub direct_children_of: Option<String>,
    /// Exact file name match; useful for marker files.
    pub name_equals: Option<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub include_directories: bool,
    pub sort: FileIndexSort,
    pub descending: bool,
    pub limit: Option<usize>,
}

impl Default for FileIndexSort {
    fn default() -> Self {
        Self::Path
    }
}

/// SQLite-backed index for one local project.
///
/// A single mutex-protected connection serializes writes and reads. This keeps
/// the cache simple and safe for current engine callers; a dedicated writer
/// thread/read-only pool can be introduced if measured contention warrants it.
pub struct ProjectFileIndex {
    project_root: PathBuf,
    connection: Mutex<Connection>,
}

impl std::fmt::Debug for ProjectFileIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectFileIndex")
            .field("project_root", &self.project_root)
            .finish_non_exhaustive()
    }
}

impl ProjectFileIndex {
    /// Open or create `.pulsar/cache/files.db` for a local project.
    pub fn open(project_root: impl Into<PathBuf>) -> Result<Self> {
        let project_root = normalize_absolute_path(project_root.into());
        let cache_dir = project_root.join(".pulsar").join("cache");
        crate::virtual_fs::create_dir_all(&cache_dir)
            .with_context(|| format!("create file-index cache at {}", cache_dir.display()))?;
        let db_path = cache_dir.join("files.db");
        let connection = Connection::open(&db_path)
            .with_context(|| format!("open project file index at {}", db_path.display()))?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        Self::initialize_schema(&connection)?;
        Ok(Self {
            project_root,
            connection: Mutex::new(connection),
        })
    }

    fn initialize_schema(connection: &Connection) -> Result<()> {
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            anyhow::bail!(
                "file index schema version {version} is newer than supported version {SCHEMA_VERSION}"
            );
        }
        if version < 1 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS files (
                    path TEXT PRIMARY KEY NOT NULL,
                    name TEXT NOT NULL,
                    extension TEXT,
                    file_type_id TEXT,
                    is_dir INTEGER NOT NULL CHECK (is_dir IN (0, 1)),
                    size INTEGER NOT NULL CHECK (size >= 0),
                    created INTEGER,
                    modified INTEGER
                 );
                 CREATE INDEX IF NOT EXISTS files_extension_idx ON files(extension);
                 CREATE INDEX IF NOT EXISTS files_type_idx ON files(file_type_id);
                 CREATE INDEX IF NOT EXISTS files_modified_idx ON files(modified);
                 CREATE INDEX IF NOT EXISTS files_name_idx ON files(name COLLATE NOCASE);
                 COMMIT;",
            )?;
        }
        if version < 2 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS asset_ids (
                    asset_id TEXT NOT NULL,
                    path TEXT NOT NULL REFERENCES files(path) ON DELETE CASCADE,
                    PRIMARY KEY (asset_id, path)
                 );
                 CREATE INDEX IF NOT EXISTS asset_ids_path_idx ON asset_ids(path);
                 CREATE INDEX IF NOT EXISTS asset_ids_id_idx ON asset_ids(asset_id);
                 PRAGMA user_version = 2;
                 COMMIT;",
            )?;
        }
        if version < 3 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 DROP INDEX IF EXISTS asset_ids_path_idx;
                 DROP INDEX IF EXISTS asset_ids_id_idx;
                 ALTER TABLE asset_ids RENAME TO asset_ids_legacy;
                 CREATE TABLE asset_ids (
                    asset_id TEXT NOT NULL,
                    kind TEXT NOT NULL DEFAULT '',
                    path TEXT NOT NULL REFERENCES files(path) ON DELETE CASCADE,
                    PRIMARY KEY (asset_id, path, kind)
                 );
                 INSERT INTO asset_ids(asset_id, kind, path)
                    SELECT asset_id, '', path FROM asset_ids_legacy;
                 DROP TABLE asset_ids_legacy;
                 CREATE INDEX asset_ids_path_idx ON asset_ids(path);
                 CREATE INDEX asset_ids_id_idx ON asset_ids(asset_id);
                 CREATE INDEX asset_ids_kind_idx ON asset_ids(kind);
                 PRAGMA user_version = 3;
                 COMMIT;",
            )?;
        }
        Ok(())
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub fn database_path(&self) -> PathBuf {
        self.project_root
            .join(".pulsar")
            .join("cache")
            .join("files.db")
    }

    /// Reconcile the index against a fresh VFS manifest. Paths whose metadata
    /// is unchanged are not rewritten; entries absent from the manifest are
    /// removed in the same transaction.
    pub fn reconcile(&self, manifest: &[ManifestEntry]) -> Result<()> {
        let mut connection = self.connection.lock().expect("file-index mutex poisoned");
        let tx = connection.transaction()?;
        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS incoming_files (
                path TEXT PRIMARY KEY NOT NULL
             );
             DELETE FROM incoming_files;",
        )?;
        {
            let mut insert_seen =
                tx.prepare("INSERT OR IGNORE INTO incoming_files(path) VALUES (?1)")?;
            let mut upsert = tx.prepare(
                "INSERT INTO files(path, name, extension, file_type_id, is_dir, size, created, modified)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)
                 ON CONFLICT(path) DO UPDATE SET
                    name=excluded.name, extension=excluded.extension,
                    file_type_id=COALESCE(excluded.file_type_id, files.file_type_id),
                    is_dir=excluded.is_dir, size=excluded.size, modified=excluded.modified
                 WHERE files.name != excluded.name
                    OR files.extension IS NOT excluded.extension
                    OR (excluded.file_type_id IS NOT NULL AND files.file_type_id IS NOT excluded.file_type_id)
                    OR files.is_dir != excluded.is_dir
                    OR files.size != excluded.size
                    OR files.modified IS NOT excluded.modified",
            )?;
            for entry in manifest {
                let path = normalize_relative_path(&entry.path);
                if path.is_empty() || is_cache_entry(&path) {
                    continue;
                }
                let full_path = self
                    .project_root
                    .join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
                let name = Path::new(&path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&path)
                    .to_owned();
                let extension = if entry.is_dir {
                    None
                } else {
                    Path::new(&path)
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .map(str::to_ascii_lowercase)
                };
                let file_type_id = if entry.is_dir {
                    None
                } else {
                    registered_file_type(&full_path)
                };
                let size = i64::try_from(entry.size).unwrap_or(i64::MAX);
                let modified = entry
                    .modified
                    .and_then(|seconds| i64::try_from(seconds).ok());
                insert_seen.execute([&path])?;
                upsert.execute(params![
                    path,
                    name,
                    extension,
                    file_type_id,
                    entry.is_dir,
                    size,
                    modified,
                ])?;
            }
        }
        tx.execute(
            "DELETE FROM files WHERE path NOT IN (SELECT path FROM incoming_files)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Apply a filesystem notification for a local path. The watcher should
    /// call this on create/modify; deletions remove a path and its descendants.
    pub fn remove_path(&self, path: &Path) -> Result<()> {
        let Some(relative) = self.relative_path(path) else {
            return Ok(());
        };
        if relative.is_empty() {
            return Ok(());
        }
        let connection = self.connection.lock().expect("file-index mutex poisoned");
        connection.execute(
            "DELETE FROM files WHERE path = ?1 OR substr(path, 1, length(?1) + 1) = ?1 || '/'",
            [&relative],
        )?;
        drop(connection);
        self.remove_durable_asset_ids(&relative)?;
        Ok(())
    }

    /// Move a file or directory subtree while retaining its stable asset IDs.
    /// The rows and ID associations are rewritten in one SQLite transaction.
    pub fn move_path(&self, from: &Path, to: &Path) -> Result<()> {
        let (Some(from), Some(to)) = (self.relative_path(from), self.relative_path(to)) else {
            anyhow::bail!("file-index move paths must be inside the project root");
        };
        if from.is_empty() || to.is_empty() || from == to {
            return Ok(());
        }
        if is_cache_entry(&from) || is_cache_entry(&to) {
            return Ok(());
        }
        if to.starts_with(&format!("{from}/")) {
            anyhow::bail!("cannot move a directory into its own subtree");
        }

        let mut connection = self.connection.lock().expect("file-index mutex poisoned");
        let tx = connection.transaction()?;
        let moved = {
            let mut statement = tx.prepare(
                "SELECT path, name, extension, file_type_id, is_dir, size, created, modified
                 FROM files WHERE path = ?1 OR substr(path, 1, length(?1) + 1) = ?1 || '/'
                 ORDER BY path",
            )?;
            let rows = statement.query_map([&from], |row| {
                Ok(IndexedFile {
                    path: row.get(0)?,
                    name: row.get(1)?,
                    extension: row.get(2)?,
                    file_type_id: row.get(3)?,
                    is_dir: row.get(4)?,
                    size: u64::try_from(row.get::<_, i64>(5)?).unwrap_or_default(),
                    created: row.get(6)?,
                    modified: row.get(7)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if moved.is_empty() {
            tx.commit()?;
            return Ok(());
        }

        let mut asset_ids = Vec::new();
        {
            let mut statement =
                tx.prepare("SELECT asset_id, kind FROM asset_ids WHERE path = ?1")?;
            for entry in &moved {
                let rows = statement.query_map([&entry.path], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                for id in rows {
                    let (id, kind) = id?;
                    asset_ids.push((id, kind, entry.path.clone()));
                }
            }
        }

        // Replacing an indexed destination is consistent with filesystem
        // rename semantics: the old destination and its IDs are discarded.
        tx.execute(
            "DELETE FROM files WHERE path = ?1 OR substr(path, 1, length(?1) + 1) = ?1 || '/'",
            [&to],
        )?;
        tx.execute(
            "DELETE FROM files WHERE path = ?1 OR substr(path, 1, length(?1) + 1) = ?1 || '/'",
            [&from],
        )?;

        for entry in &moved {
            let suffix = entry.path.strip_prefix(&from).unwrap_or_default();
            let new_path = if suffix.is_empty() {
                to.clone()
            } else {
                format!("{to}{suffix}")
            };
            let name = Path::new(&new_path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&new_path)
                .to_owned();
            let extension = if entry.is_dir {
                None
            } else {
                Path::new(&new_path)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .map(str::to_ascii_lowercase)
            };
            let file_type_id = if extension != entry.extension {
                registered_file_type(
                    &self
                        .project_root
                        .join(new_path.replace('/', std::path::MAIN_SEPARATOR_STR)),
                )
            } else {
                entry.file_type_id.clone()
            };
            tx.execute(
                "INSERT INTO files(path, name, extension, file_type_id, is_dir, size, created, modified)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    new_path,
                    name,
                    extension,
                    file_type_id,
                    entry.is_dir,
                    i64::try_from(entry.size).unwrap_or(i64::MAX),
                    entry.created,
                    entry.modified,
                ],
            )?;
        }
        for (id, kind, old_path) in asset_ids {
            let suffix = old_path.strip_prefix(&from).unwrap_or_default();
            let new_path = if suffix.is_empty() {
                to.clone()
            } else {
                format!("{to}{suffix}")
            };
            tx.execute(
                "INSERT OR IGNORE INTO asset_ids(asset_id, kind, path) VALUES (?1, ?2, ?3)",
                params![id, kind, new_path],
            )?;
        }
        tx.commit()?;
        // Lock order is manifest, then connection (`ensure_typed_asset_id`,
        // `restore_durable_asset_ids`). Holding the connection while taking the
        // manifest lock here is the reverse, and deadlocks against them.
        drop(connection);
        self.move_durable_asset_ids(&from, &to)?;
        Ok(())
    }

    /// Associate a plugin-defined stable id with an indexed project file.
    /// Recording another id for the same path replaces the previous value.
    pub fn record_asset_id(&self, id: &str, path: &Path) -> Result<()> {
        let Some(relative) = self.relative_path(path) else {
            anyhow::bail!("asset id path is outside the indexed project root");
        };
        let connection = self.connection.lock().expect("file-index mutex poisoned");
        connection.execute(
            "DELETE FROM asset_ids WHERE path = ?1 AND kind = ''",
            [&relative],
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO asset_ids(asset_id, kind, path) VALUES (?1, '', ?2)",
            params![id, relative],
        )?;
        Ok(())
    }

    /// Associate a stable ID and asset kind with an indexed project file.
    /// Recording another ID of the same kind for the path replaces its prior
    /// typed association while preserving IDs recorded for other kinds.
    pub fn record_typed_asset_id(&self, kind: &str, id: &str, path: &Path) -> Result<()> {
        let Some(relative) = self.relative_path(path) else {
            anyhow::bail!("asset id path is outside the indexed project root");
        };
        let kind = kind.trim();
        if kind.is_empty() {
            anyhow::bail!("typed asset kind cannot be empty");
        }
        let connection = self.connection.lock().expect("file-index mutex poisoned");
        connection.execute(
            "DELETE FROM asset_ids WHERE path = ?1 AND kind = ?2",
            params![relative, kind],
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO asset_ids(asset_id, kind, path) VALUES (?1, ?2, ?3)",
            params![id, kind, relative],
        )?;
        Ok(())
    }

    /// Ensure that a file or folder has a persistent typed ID.
    ///
    /// Embedded asset IDs should use [`record_typed_asset_id`](Self::record_typed_asset_id)
    /// instead. This helper is for file and folder assets that cannot store
    /// metadata in their payload (such as PNG images). IDs are saved in the
    /// project-owned `.pulsar/asset_ids.json` manifest and mirrored into the
    /// disposable file index for fast picker lookup.
    pub fn ensure_typed_asset_id(&self, kind: &str, path: &Path) -> Result<String> {
        let Some(relative) = self.relative_path(path) else {
            anyhow::bail!("asset id path is outside the indexed project root");
        };
        let kind = kind.trim();
        if kind.is_empty() {
            anyhow::bail!("typed asset kind cannot be empty");
        }
        self.refresh_path(path)?;
        let lock = ASSET_ID_MANIFEST_LOCK.get_or_init(|| parking_lot::Mutex::new(()));
        let _guard = lock.lock();
        let manifest_path = self.project_root.join(ASSET_ID_MANIFEST);
        let mut manifest = read_durable_asset_id_manifest(&manifest_path)?;
        if let Some(id) = manifest
            .assets
            .get(&relative)
            .and_then(|by_kind| by_kind.get(kind))
            .cloned()
        {
            self.record_typed_asset_id(kind, &id, path)?;
            return Ok(id);
        }
        let id = uuid::Uuid::new_v4().to_string();
        manifest.version = 1;
        manifest
            .assets
            .entry(relative)
            .or_default()
            .insert(kind.to_owned(), id.clone());
        write_durable_asset_id_manifest(&manifest_path, &manifest)?;
        self.record_typed_asset_id(kind, &id, path)?;
        Ok(id)
    }

    fn remove_durable_asset_ids(&self, relative: &str) -> Result<()> {
        let lock = ASSET_ID_MANIFEST_LOCK.get_or_init(|| parking_lot::Mutex::new(()));
        let _guard = lock.lock();
        let manifest_path = self.project_root.join(ASSET_ID_MANIFEST);
        let mut manifest = read_durable_asset_id_manifest(&manifest_path)?;
        let removed: Vec<String> = manifest
            .assets
            .keys()
            .filter(|path| *path == relative || path.starts_with(&format!("{relative}/")))
            .cloned()
            .collect();
        if removed.is_empty() {
            return Ok(());
        }
        for path in removed {
            manifest.assets.remove(&path);
        }
        write_durable_asset_id_manifest(&manifest_path, &manifest)
    }

    fn move_durable_asset_ids(&self, from: &str, to: &str) -> Result<()> {
        let lock = ASSET_ID_MANIFEST_LOCK.get_or_init(|| parking_lot::Mutex::new(()));
        let _guard = lock.lock();
        let manifest_path = self.project_root.join(ASSET_ID_MANIFEST);
        let mut manifest = read_durable_asset_id_manifest(&manifest_path)?;
        let moved: Vec<(String, BTreeMap<String, String>)> = manifest
            .assets
            .iter()
            .filter(|(path, _)| *path == from || path.starts_with(&format!("{from}/")))
            .map(|(path, kinds)| (path.clone(), kinds.clone()))
            .collect();
        if moved.is_empty() {
            return Ok(());
        }
        for (old_path, _) in &moved {
            manifest.assets.remove(old_path);
        }
        for (old_path, kinds) in moved {
            let suffix = old_path.strip_prefix(from).unwrap_or_default();
            let new_path = if suffix.is_empty() {
                to.to_owned()
            } else {
                format!("{to}{suffix}")
            };
            manifest.assets.insert(new_path, kinds);
        }
        write_durable_asset_id_manifest(&manifest_path, &manifest)
    }

    fn restore_durable_asset_ids(&self) -> Result<()> {
        let lock = ASSET_ID_MANIFEST_LOCK.get_or_init(|| parking_lot::Mutex::new(()));
        let _guard = lock.lock();
        let manifest = read_durable_asset_id_manifest(&self.project_root.join(ASSET_ID_MANIFEST))?;
        for (relative, kinds) in manifest.assets {
            let path = self
                .project_root
                .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
            if !crate::virtual_fs::exists(&path).unwrap_or(false) {
                continue;
            }
            for (kind, id) in kinds {
                self.record_typed_asset_id(&kind, &id, &path)?;
            }
        }
        Ok(())
    }

    /// Remove all stable-ID associations for a project file. Call this when a
    /// parsed asset no longer has a valid ID (or can no longer be read).
    pub fn clear_asset_ids(&self, path: &Path) -> Result<()> {
        let Some(relative) = self.relative_path(path) else {
            return Ok(());
        };
        let connection = self.connection.lock().expect("file-index mutex poisoned");
        connection.execute("DELETE FROM asset_ids WHERE path = ?1", [&relative])?;
        Ok(())
    }

    /// Singular alias for [`ProjectFileIndex::clear_asset_ids`].
    pub fn clear_asset_id(&self, path: &Path) -> Result<()> {
        self.clear_asset_ids(path)
    }

    /// Return every path claiming `id`. More than one result indicates a
    /// copied asset whose stable id must be repaired by its owning plugin.
    pub fn paths_for_asset_id(&self, id: &str) -> Result<Vec<PathBuf>> {
        let connection = self.connection.lock().expect("file-index mutex poisoned");
        let mut statement =
            connection.prepare("SELECT path FROM asset_ids WHERE asset_id = ?1 ORDER BY path")?;
        let rows = statement.query_map([id], |row| row.get::<_, String>(0))?;
        rows.map(|row| {
            row.map(|relative| {
                self.project_root
                    .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
            })
        })
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("query asset id index")
    }

    /// Return the first project path carrying `id` for `kind`, if indexed.
    pub fn path_for_asset_id_kind(&self, kind: &str, id: &str) -> Result<Option<PathBuf>> {
        let connection = self.connection.lock().expect("file-index mutex poisoned");
        let mut statement = connection.prepare(
            "SELECT path FROM asset_ids WHERE kind = ?1 AND asset_id = ?2 ORDER BY path LIMIT 2",
        )?;
        let mut rows = statement.query(params![kind, id])?;
        let Some(first) = rows.next()? else {
            return Ok(None);
        };
        let relative: String = first.get(0)?;
        if rows.next()?.is_some() {
            return Ok(None);
        }
        Ok(Some(self.project_root.join(
            relative.replace('/', std::path::MAIN_SEPARATOR_STR),
        )))
    }

    /// Return every project path carrying `id` for `kind`.
    pub fn paths_for_asset_id_kind(&self, kind: &str, id: &str) -> Result<Vec<PathBuf>> {
        let connection = self.connection.lock().expect("file-index mutex poisoned");
        let mut statement = connection.prepare(
            "SELECT path FROM asset_ids WHERE kind = ?1 AND asset_id = ?2 ORDER BY path",
        )?;
        let rows = statement.query_map(params![kind, id], |row| row.get::<_, String>(0))?;
        rows.map(|row| {
            row.map(|relative| {
                self.project_root
                    .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
            })
        })
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("query typed asset id paths")
    }

    /// Return stable asset IDs associated with one indexed project file.
    pub fn asset_ids_for_path(&self, path: &Path) -> Result<Vec<String>> {
        let Some(relative) = self.relative_path(path) else {
            anyhow::bail!("asset id path is outside the indexed project root");
        };
        let connection = self.connection.lock().expect("file-index mutex poisoned");
        let mut statement = connection
            .prepare("SELECT asset_id FROM asset_ids WHERE path = ?1 ORDER BY asset_id")?;
        let rows = statement.query_map([relative], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("query asset ids for file")
    }

    /// Return stable asset IDs of `kind` associated with one indexed file.
    pub fn asset_ids_for_path_kind(&self, path: &Path, kind: &str) -> Result<Vec<String>> {
        let Some(relative) = self.relative_path(path) else {
            anyhow::bail!("asset id path is outside the indexed project root");
        };
        let connection = self.connection.lock().expect("file-index mutex poisoned");
        let mut statement = connection.prepare(
            "SELECT asset_id FROM asset_ids WHERE path = ?1 AND kind = ?2 ORDER BY asset_id",
        )?;
        let rows = statement.query_map(params![relative, kind], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("query typed asset ids for file")
    }

    /// Refresh one local file or directory from the active VFS metadata.
    pub fn refresh_path(&self, path: &Path) -> Result<()> {
        let Some(relative) = self.relative_path(path) else {
            return Ok(());
        };
        if relative.is_empty() || is_cache_entry(&relative) {
            return Ok(());
        }
        // File-index updates run synchronously from local provider callbacks,
        // which can be emitted while the virtual-filesystem provider read lock
        // is held. Read local metadata directly to avoid re-entering that lock.
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => return self.remove_path(path),
        };
        let entry = ManifestEntry {
            path: relative,
            is_dir: metadata.is_dir(),
            size: metadata.len(),
            modified: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs()),
        };
        self.reconcile_one(&entry)?;
        if metadata.is_dir() {
            let relative_root = self.relative_path(path).unwrap_or_default();
            let mut children = Vec::new();
            for child in walkdir::WalkDir::new(path).min_depth(1) {
                let child = child?;
                let metadata = child.metadata()?;
                let child_relative = child
                    .path()
                    .strip_prefix(path)
                    .context("walked file escaped refreshed directory")?;
                let child_relative = normalize_relative_path(&child_relative.to_string_lossy());
                let child_path = if relative_root.is_empty() {
                    child_relative
                } else {
                    format!("{relative_root}/{child_relative}")
                };
                if is_cache_entry(&child_path) {
                    continue;
                }
                children.push(ManifestEntry {
                    path: child_path,
                    is_dir: metadata.is_dir(),
                    size: metadata.len(),
                    modified: metadata
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|duration| duration.as_secs()),
                });
            }

            // Preserve stable-ID rows for unchanged files. Only remove old
            // descendants that disappeared from the refreshed subtree.
            let existing = self.query(&FileIndexQuery {
                folder: Some(relative_root.clone()),
                include_directories: true,
                ..Default::default()
            })?;
            let seen: std::collections::HashSet<_> =
                children.iter().map(|child| child.path.as_str()).collect();
            for stale in existing {
                if stale.path != relative_root && !seen.contains(stale.path.as_str()) {
                    self.remove_path(
                        &self
                            .project_root
                            .join(stale.path.replace('/', std::path::MAIN_SEPARATOR_STR)),
                    )?;
                }
            }
            for child in &children {
                self.reconcile_one(child)?;
            }
        }
        Ok(())
    }

    fn reconcile_one(&self, entry: &ManifestEntry) -> Result<()> {
        let path = normalize_relative_path(&entry.path);
        let name = Path::new(&path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&path)
            .to_owned();
        let extension = if entry.is_dir {
            None
        } else {
            Path::new(&path)
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
        };
        let full_path = self
            .project_root
            .join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let file_type_id = if entry.is_dir {
            None
        } else {
            registered_file_type(&full_path)
        };
        let connection = self.connection.lock().expect("file-index mutex poisoned");
        connection.execute(
            "INSERT INTO files(path, name, extension, file_type_id, is_dir, size, created, modified)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)
             ON CONFLICT(path) DO UPDATE SET name=excluded.name, extension=excluded.extension,
                file_type_id=COALESCE(excluded.file_type_id, files.file_type_id), is_dir=excluded.is_dir,
                size=excluded.size, modified=excluded.modified",
            params![
                path,
                name,
                extension,
                file_type_id,
                entry.is_dir,
                i64::try_from(entry.size).unwrap_or(i64::MAX),
                entry.modified.and_then(|seconds| i64::try_from(seconds).ok()),
            ],
        )?;
        Ok(())
    }

    pub fn query(&self, query: &FileIndexQuery) -> Result<Vec<IndexedFile>> {
        let mut sql = String::from(
            "SELECT path, name, extension, file_type_id, is_dir, size, created, modified FROM files WHERE 1=1",
        );
        let mut values = Vec::<Value>::new();
        if !query.include_directories {
            sql.push_str(" AND is_dir = 0");
        }
        if let Some(file_type) = query.file_type_id.as_deref() {
            sql.push_str(" AND file_type_id = ?");
            values.push(Value::Text(file_type.to_owned()));
        }
        if let Some(extension) = query.extension.as_deref() {
            sql.push_str(" AND extension = ?");
            values.push(Value::Text(
                extension.trim_start_matches('.').to_ascii_lowercase(),
            ));
        }
        if let Some(after) = query.modified_after {
            sql.push_str(" AND modified >= ?");
            values.push(Value::Integer(after));
        }
        if let Some(before) = query.modified_before {
            sql.push_str(" AND modified <= ?");
            values.push(Value::Integer(before));
        }
        if let Some(name) = query
            .name_contains
            .as_deref()
            .filter(|name| !name.is_empty())
        {
            sql.push_str(" AND instr(lower(name), lower(?)) > 0");
            values.push(Value::Text(name.to_owned()));
        }
        if let Some(name) = query.name_equals.as_deref() {
            sql.push_str(" AND name = ? COLLATE NOCASE");
            values.push(Value::Text(name.to_owned()));
        }
        if let Some(folder) = query.folder.as_deref() {
            let folder = normalize_relative_path(folder)
                .trim_end_matches('/')
                .to_owned();
            if folder.is_empty() {
                // All entries are already relative to the project root.
            } else {
                sql.push_str(" AND (path = ? OR substr(path, 1, length(?) + 1) = ? || '/')");
                values.extend([
                    Value::Text(folder.clone()),
                    Value::Text(folder.clone()),
                    Value::Text(folder),
                ]);
            }
        }
        if let Some(folder) = query.direct_children_of.as_deref() {
            let folder = normalize_relative_path(folder)
                .trim_end_matches('/')
                .to_owned();
            if folder.is_empty() {
                sql.push_str(" AND instr(path, '/') = 0");
            } else {
                sql.push_str(
                    " AND substr(path, 1, length(?) + 1) = ? || '/'\
                      AND instr(substr(path, length(?) + 2), '/') = 0",
                );
                values.extend([
                    Value::Text(folder.clone()),
                    Value::Text(folder.clone()),
                    Value::Text(folder),
                ]);
            }
        }
        if let Some(min_size) = query.min_size {
            sql.push_str(" AND size >= ?");
            values.push(Value::Integer(i64::try_from(min_size).unwrap_or(i64::MAX)));
        }
        if let Some(max_size) = query.max_size {
            sql.push_str(" AND size <= ?");
            values.push(Value::Integer(i64::try_from(max_size).unwrap_or(i64::MAX)));
        }
        let order = match query.sort {
            FileIndexSort::Path => "path",
            FileIndexSort::Name => "name COLLATE NOCASE",
            FileIndexSort::Extension => "extension",
            FileIndexSort::FileType => "file_type_id",
            FileIndexSort::Size => "size",
            FileIndexSort::Created => "created",
            FileIndexSort::Modified => "modified",
        };
        sql.push_str(" ORDER BY ");
        sql.push_str(order);
        sql.push_str(if query.descending { " DESC" } else { " ASC" });
        sql.push_str(", path ASC");
        if let Some(limit) = query.limit {
            sql.push_str(" LIMIT ?");
            values.push(Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
        }

        let connection = self.connection.lock().expect("file-index mutex poisoned");
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok(IndexedFile {
                path: row.get(0)?,
                name: row.get(1)?,
                extension: row.get(2)?,
                file_type_id: row.get(3)?,
                is_dir: row.get(4)?,
                size: u64::try_from(row.get::<_, i64>(5)?).unwrap_or_default(),
                created: row.get(6)?,
                modified: row.get(7)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("query project file index")
    }

    fn relative_path(&self, path: &Path) -> Option<String> {
        if !path.is_absolute() {
            return None;
        }
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Ok(relative) = path.strip_prefix(&self.project_root) {
            return Some(normalize_relative_path(&relative.to_string_lossy()));
        }
        #[cfg(windows)]
        {
            let normalize = |path: &Path| {
                let mut path = path.to_string_lossy().replace('\\', "/");
                if let Some(without_prefix) = path.strip_prefix("//?/") {
                    path = without_prefix.to_owned();
                }
                path.trim_end_matches('/').to_owned()
            };
            let path = normalize(&path);
            let root = normalize(&self.project_root);
            if path.eq_ignore_ascii_case(&root) {
                return Some(String::new());
            }
            if path.len() > root.len()
                && path[root.len()..].starts_with('/')
                && path[..root.len()].eq_ignore_ascii_case(&root)
            {
                return Some(normalize_relative_path(&path[root.len() + 1..]));
            }
        }
        None
    }

    #[cfg(test)]
    fn file_count(&self) -> Result<usize> {
        let connection = self.connection.lock().expect("file-index mutex poisoned");
        let count: i64 =
            connection.query_row("SELECT count(*) FROM files", [], |row| row.get(0))?;
        Ok(usize::try_from(count).unwrap_or_default())
    }
}

static SHARED_FILE_INDEXES: OnceLock<parking_lot::Mutex<HashMap<PathBuf, Arc<ProjectFileIndex>>>> =
    OnceLock::new();

/// Get the process-shared index for a local project, creating and populating
/// the cache the first time it is requested. Remote and P2P projects return
/// `Ok(None)` so callers can use their provider's native listing path.
pub fn open_project_file_index(project_root: &Path) -> Result<Option<Arc<ProjectFileIndex>>> {
    if crate::virtual_fs::is_remote() || crate::is_cloud_path(project_root) {
        return Ok(None);
    }
    let key = normalize_absolute_path(
        project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf()),
    );
    let indexes = SHARED_FILE_INDEXES.get_or_init(|| parking_lot::Mutex::new(HashMap::new()));
    let existing = { indexes.lock().get(&key).cloned() };
    if let Some(index) = existing {
        if let Err(error) = crate::watchers::start_file_index_watcher(key, index.clone()) {
            tracing::warn!(%error, root = %index.project_root().display(), "Could not watch project file index for live changes");
        }
        return Ok(Some(index.clone()));
    }

    // Opening creates `.pulsar/cache` through VFS, which emits a local event.
    // Never hold the shared registry lock across provider calls or that event
    // handler would re-enter this registry and deadlock.
    let index = Arc::new(ProjectFileIndex::open(key.clone())?);
    let manifest =
        crate::virtual_fs::manifest(&key).context("read project manifest for file index")?;
    index.reconcile(&manifest)?;
    index.restore_durable_asset_ids()?;
    let index = {
        let mut indexes = indexes.lock();
        if let Some(existing) = indexes.get(&key) {
            existing.clone()
        } else {
            indexes.insert(key.clone(), index.clone());
            index
        }
    };
    if let Err(error) = crate::watchers::start_file_index_watcher(key.clone(), index.clone()) {
        tracing::warn!(%error, root = %key.display(), "Could not watch project file index for live changes");
    }
    Ok(Some(index))
}

/// Return an already-open project index containing an absolute filesystem path.
/// This never opens a database, so callers can use it after the project index
/// loading task has completed without accidentally triggering a scan.
pub fn project_file_index_for_path(path: &Path) -> Option<Arc<ProjectFileIndex>> {
    if !path.is_absolute() {
        return None;
    }
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let indexes = SHARED_FILE_INDEXES.get()?.lock();
    indexes
        .values()
        .filter(|index| index.relative_path(&path).is_some())
        .max_by_key(|index| index.project_root.components().count())
        .cloned()
}

/// Synchronously apply a local VFS operation to every already-open index that
/// owns its path. Local notify watchers remain responsible for external disk
/// changes; remote VFS operations must not update this local-only cache.
pub fn update_open_project_file_indexes(path: &Path, kind: &FsChangeKind, source: &FsEventSource) {
    if !matches!(source, FsEventSource::Local) {
        return;
    }
    let indexes = SHARED_FILE_INDEXES
        .get()
        .map(|indexes| indexes.lock().values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    for index in indexes {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            index.project_root.join(path)
        };
        if index.relative_path(&path).is_none() {
            continue;
        }
        let result = match kind {
            FsChangeKind::Deleted => index.remove_path(&path),
            FsChangeKind::Created | FsChangeKind::Modified => index.refresh_path(&path),
        };
        if let Err(error) = result {
            tracing::warn!(%error, path = %path.display(), "Could not update project file index from local VFS event");
        }
    }
}

/// Move indexed paths in every open local project that contains either side.
/// If a move crosses a project boundary, remove it from the old index and
/// refresh it in the new one.
pub fn move_open_project_file_indexes(from: &Path, to: &Path, source: &FsEventSource) {
    if !matches!(source, FsEventSource::Local) {
        return;
    }
    let indexes = SHARED_FILE_INDEXES
        .get()
        .map(|indexes| indexes.lock().values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    for index in indexes {
        let from_path = if from.is_absolute() {
            from.to_path_buf()
        } else {
            index.project_root.join(from)
        };
        let to_path = if to.is_absolute() {
            to.to_path_buf()
        } else {
            index.project_root.join(to)
        };
        let from_is_in_project = index.relative_path(&from_path).is_some();
        let to_is_in_project = index.relative_path(&to_path).is_some();
        let result = match (from_is_in_project, to_is_in_project) {
            (true, true) => index.move_path(&from_path, &to_path),
            (true, false) => index.remove_path(&from_path),
            (false, true) => index.refresh_path(&to_path),
            (false, false) => continue,
        };
        if let Err(error) = result {
            tracing::warn!(%error, from = %from_path.display(), to = %to_path.display(), "Could not move paths in project file index");
        }
    }
}

fn normalize_relative_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_owned()
}

fn normalize_absolute_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let path = path.to_string_lossy();
        if let Some(unc_path) = path.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc_path}"));
        }
        if let Some(path) = path.strip_prefix(r"\\?\") {
            return PathBuf::from(path);
        }
        PathBuf::from(path.as_ref())
    }
    #[cfg(not(windows))]
    path
}

fn is_cache_entry(path: &str) -> bool {
    path == ".pulsar/cache" || path.starts_with(".pulsar/cache/") || path == ASSET_ID_MANIFEST
}

fn read_durable_asset_id_manifest(path: &Path) -> Result<DurableAssetIdManifest> {
    if !crate::virtual_fs::exists(path)? {
        return Ok(DurableAssetIdManifest::default());
    }
    let bytes = crate::virtual_fs::read_file(path)
        .with_context(|| format!("read durable asset IDs from {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse durable asset IDs from {}", path.display()))
}

fn write_durable_asset_id_manifest(path: &Path, manifest: &DurableAssetIdManifest) -> Result<()> {
    let parent = path
        .parent()
        .context("durable asset ID manifest has no parent directory")?;
    crate::virtual_fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec_pretty(manifest)?;
    crate::virtual_fs::write_file_atomically(path, &bytes)
        .or_else(|_| crate::virtual_fs::write_file(path, &bytes))
        .with_context(|| format!("write durable asset IDs to {}", path.display()))
}

fn registered_file_type(path: &Path) -> Option<String> {
    // Lock-free on purpose. `ui_core` creates editors while holding the plugin
    // manager's write lock, and an editor may refresh this index from that same
    // thread; taking the manager's lock here would wait on itself forever
    // (`parking_lot` locks are not reentrant).
    plugin_manager::file_type_for_path(path).map(|file_type| file_type.as_str().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, is_dir: bool, size: u64, modified: Option<u64>) -> ManifestEntry {
        ManifestEntry {
            path: path.to_owned(),
            is_dir,
            size,
            modified,
        }
    }

    #[test]
    fn persists_manifest_and_queries_filters_after_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let index = ProjectFileIndex::open(root.clone()).unwrap();
        index
            .reconcile(&[
                file("Assets", true, 0, None),
                file("Assets/wood.PNG", false, 4096, Some(100)),
                file("Assets/scene.json", false, 512, Some(200)),
                file(".pulsar/cache/files.db", false, 1, Some(300)),
            ])
            .unwrap();
        assert!(index.database_path().exists());
        let pngs = index
            .query(&FileIndexQuery {
                extension: Some(".PNG".to_owned()),
                folder: Some("Assets".to_owned()),
                modified_after: Some(100),
                min_size: Some(1000),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            pngs.iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["Assets/wood.PNG"]
        );
        drop(index);

        let reopened = ProjectFileIndex::open(root).unwrap();
        assert_eq!(reopened.query(&FileIndexQuery::default()).unwrap().len(), 2);
        assert_eq!(reopened.file_count().unwrap(), 3); // includes the Assets directory
    }

    #[test]
    fn reconcile_removes_stale_paths_and_refresh_can_delete_subtree() {
        let temp = tempfile::tempdir().unwrap();
        let index = ProjectFileIndex::open(temp.path()).unwrap();
        index
            .reconcile(&[
                file("Assets", true, 0, None),
                file("Assets/a.png", false, 10, Some(1)),
                file("Assets/b.png", false, 20, Some(2)),
            ])
            .unwrap();
        index
            .reconcile(&[
                file("Assets", true, 0, None),
                file("Assets/b.png", false, 21, Some(3)),
            ])
            .unwrap();
        assert_eq!(index.query(&FileIndexQuery::default()).unwrap().len(), 1);
        index.remove_path(&temp.path().join("Assets")).unwrap();
        assert!(index
            .query(&FileIndexQuery {
                include_directories: true,
                ..Default::default()
            })
            .unwrap()
            .is_empty());
    }

    #[test]
    fn query_name_search_treats_percent_as_literal_and_sorts() {
        let temp = tempfile::tempdir().unwrap();
        let index = ProjectFileIndex::open(temp.path()).unwrap();
        index
            .reconcile(&[
                file("z/100%.txt", false, 1, Some(1)),
                file("a/100x.txt", false, 2, Some(2)),
            ])
            .unwrap();
        let result = index
            .query(&FileIndexQuery {
                name_contains: Some("100%".to_owned()),
                sort: FileIndexSort::Name,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].path, "z/100%.txt");
    }

    #[test]
    fn direct_children_and_duplicate_asset_ids_are_indexed() {
        let temp = tempfile::tempdir().unwrap();
        let index = ProjectFileIndex::open(temp.path()).unwrap();
        index
            .reconcile(&[
                file("Assets", true, 0, None),
                file("Assets/a.widget", false, 100, Some(1)),
                file("Assets/nested", true, 0, None),
                file("Assets/nested/b.widget", false, 100, Some(1)),
            ])
            .unwrap();
        let entries = index
            .query(&FileIndexQuery {
                direct_children_of: Some("Assets".to_owned()),
                include_directories: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|entry| entry.path == "Assets/nested"));

        index
            .record_asset_id("widget-1", &temp.path().join("Assets/a.widget"))
            .unwrap();
        index
            .record_asset_id("widget-1", &temp.path().join("Assets/nested/b.widget"))
            .unwrap();
        assert_eq!(index.paths_for_asset_id("widget-1").unwrap().len(), 2);
        index
            .remove_path(&temp.path().join("Assets/nested/b.widget"))
            .unwrap();
        assert_eq!(index.paths_for_asset_id("widget-1").unwrap().len(), 1);
    }

    #[test]
    fn local_vfs_events_update_only_already_open_indexes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let index = Arc::new(ProjectFileIndex::open(root.clone()).unwrap());
        SHARED_FILE_INDEXES
            .get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
            .lock()
            .insert(root.clone(), index.clone());
        let file_path = root.join("created.widget");
        std::fs::write(&file_path, "{}").unwrap();

        crate::events::emit_with_source(
            file_path.clone(),
            FsChangeKind::Created,
            FsEventSource::Local,
        );
        assert!(index
            .query(&FileIndexQuery {
                extension: Some("widget".to_owned()),
                ..Default::default()
            })
            .unwrap()
            .iter()
            .any(|entry| entry.path == "created.widget"));
        assert!(project_file_index_for_path(&file_path)
            .is_some_and(|found| Arc::ptr_eq(&found, &index)));

        crate::events::emit_with_source(
            file_path.clone(),
            FsChangeKind::Deleted,
            FsEventSource::Remote,
        );
        assert_eq!(index.query(&FileIndexQuery::default()).unwrap().len(), 1);

        crate::events::emit_with_source(
            file_path.clone(),
            FsChangeKind::Deleted,
            FsEventSource::Local,
        );
        assert!(index.query(&FileIndexQuery::default()).unwrap().is_empty());
        SHARED_FILE_INDEXES.get().unwrap().lock().remove(&root);
    }

    #[test]
    fn local_vfs_rename_moves_rows_and_preserves_stable_ids() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let old_path = root.join("Widgets/Before.widget");
        let new_path = root.join("Widgets/After.widget");
        std::fs::create_dir_all(old_path.parent().unwrap()).unwrap();
        std::fs::write(&old_path, "widget").unwrap();
        let index = Arc::new(ProjectFileIndex::open(root.clone()).unwrap());
        index
            .reconcile(&crate::virtual_fs::manifest(&root).unwrap())
            .unwrap();
        index
            .record_asset_id("widget-stable-id", &old_path)
            .unwrap();
        SHARED_FILE_INDEXES
            .get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
            .lock()
            .insert(root.clone(), index.clone());

        crate::virtual_fs::rename(&old_path, &new_path).unwrap();

        assert_eq!(
            index.paths_for_asset_id("widget-stable-id").unwrap(),
            [normalize_absolute_path(new_path.clone())]
        );
        assert!(index
            .query(&FileIndexQuery::default())
            .unwrap()
            .iter()
            .any(|entry| entry.path == "Widgets/After.widget"));
        SHARED_FILE_INDEXES.get().unwrap().lock().remove(&root);
    }

    #[test]
    fn refreshing_a_directory_preserves_ids_and_removes_deleted_descendants() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let assets = root.join("Assets");
        let existing = assets.join("existing.widget");
        std::fs::create_dir_all(&assets).unwrap();
        std::fs::write(&existing, "{}").unwrap();

        let index = ProjectFileIndex::open(root).unwrap();
        index.refresh_path(&assets).unwrap();
        index.record_asset_id("widget-id", &existing).unwrap();

        std::fs::write(assets.join("new.widget"), "{}").unwrap();
        index.refresh_path(&assets).unwrap();
        assert_eq!(
            index.paths_for_asset_id("widget-id").unwrap(),
            vec![normalize_absolute_path(existing.clone())]
        );
        assert!(index
            .query(&FileIndexQuery {
                extension: Some("widget".to_owned()),
                ..Default::default()
            })
            .unwrap()
            .iter()
            .any(|entry| entry.path == "Assets/new.widget"));

        std::fs::remove_file(&existing).unwrap();
        index.refresh_path(&assets).unwrap();
        assert!(index.paths_for_asset_id("widget-id").unwrap().is_empty());
    }
}
