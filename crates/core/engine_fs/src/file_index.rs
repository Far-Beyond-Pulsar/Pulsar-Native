//! Persistent, disposable index of project files.
//!
//! This index stores provider-independent project-relative paths and basic
//! metadata. The SQLite file is local-only cache data under `.pulsar/cache/`;
//! remote providers can continue to use their own manifest/query mechanisms.

mod asset_deps;
mod asset_ids;
mod query;
mod registry;
mod schema;
mod sync;
mod types;

#[cfg(test)]
mod asset_dep_tests;
#[cfg(test)]
mod tests;

pub use registry::{
    move_open_project_file_indexes, open_project_file_index, project_file_index_for_path,
    update_open_project_file_indexes,
};
pub use types::{AssetDep, AssetDependent, FileIndexQuery, FileIndexSort, IndexedFile};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

use crate::providers::ManifestEntry;
use asset_ids::ASSET_ID_MANIFEST;

const SCHEMA_VERSION: i64 = 4;

/// SQLite-backed index for one local project.
///
/// A single mutex-protected connection serializes writes and reads. This keeps
/// the cache simple and safe for current engine callers; a dedicated writer
/// thread/read-only pool can be introduced if measured contention warrants it.
pub struct ProjectFileIndex {
    project_root: PathBuf,
    /// `parking_lot` mutex: it cannot poison. A panic mid-operation drops any open
    /// `rusqlite::Transaction`, which rolls back, so the connection stays consistent.
    connection: parking_lot::Mutex<Connection>,
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
    #[track_caller]
    pub fn open(project_root: impl Into<PathBuf>) -> Result<Self> {
        editor_task_queue::blocking_call("file_index::open");
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
            connection: parking_lot::Mutex::new(connection),
        })
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

    /// Apply a filesystem notification for a local path. The watcher should
    /// call this on create/modify; deletions remove a path and its descendants.
    #[track_caller]
    pub fn remove_path(&self, path: &Path) -> Result<()> {
        editor_task_queue::blocking_call("file_index::remove_path");
        let Some(relative) = self.relative_path(path) else {
            return Ok(());
        };
        if relative.is_empty() {
            return Ok(());
        }
        let connection = self.connection.lock();
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
    #[track_caller]
    pub fn move_path(&self, from: &Path, to: &Path) -> Result<()> {
        editor_task_queue::blocking_call("file_index::move_path");
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

        let mut connection = self.connection.lock();
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

        let mut asset_deps = Vec::new();
        {
            let mut statement = tx.prepare(
                "SELECT source_kind, source_id, target_kind, target_id, role, via, expect
                 FROM asset_deps WHERE source_path = ?1",
            )?;
            for entry in &moved {
                let rows = statement.query_map([&entry.path], |row| {
                    Ok(AssetDep {
                        source_kind: row.get(0)?,
                        source_id: row.get(1)?,
                        target_kind: row.get(2)?,
                        target_id: row.get(3)?,
                        role: row.get(4)?,
                        via: row.get(5)?,
                        expect: row.get(6)?,
                    })
                })?;
                for dep in rows {
                    asset_deps.push((dep?, entry.path.clone()));
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
        for (dep, old_path) in asset_deps {
            let suffix = old_path.strip_prefix(&from).unwrap_or_default();
            let new_path = if suffix.is_empty() {
                to.clone()
            } else {
                format!("{to}{suffix}")
            };
            tx.execute(
                "INSERT OR IGNORE INTO asset_deps(source_path, source_kind, source_id, target_kind, target_id, role, via, expect)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    new_path,
                    dep.source_kind,
                    dep.source_id,
                    dep.target_kind,
                    dep.target_id,
                    dep.role,
                    dep.via,
                    dep.expect
                ],
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

    fn relative_path(&self, path: &Path) -> Option<String> {
        if !path.is_absolute() {
            return None;
        }
        let path = crate::virtual_fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
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
        let connection = self.connection.lock();
        let count: i64 =
            connection.query_row("SELECT count(*) FROM files", [], |row| row.get(0))?;
        Ok(usize::try_from(count).unwrap_or_default())
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

/// Whether `path`, below `project_root`, is inside an
/// [`UNINDEXED_DIRS`](crate::providers::UNINDEXED_DIRS) directory (`target/`,
/// `.git/`, ...), which the full manifest skips. The project watcher and the
/// file manager ignore changes there: a build writes tens of thousands of
/// files under `target/`. (A file there can still be indexed on request, when
/// a picker gives it a durable id.)
pub fn is_unindexed_path(project_root: &Path, path: &Path) -> bool {
    path.strip_prefix(project_root).is_ok_and(|relative| {
        relative
            .components()
            .any(|part| crate::providers::is_unindexed_dir(&part.as_os_str().to_string_lossy()))
    })
}

/// Names a path's registered file type for the index's `file_type` column.
/// The editor's file-type registry submits one (`plugin_manager`); without it
/// (a game, a tool) files are indexed with no type. Must not block: an editor
/// may refresh the index while holding the registry's lock.
pub struct FileTypeClassifier {
    pub origin: &'static str,
    pub id: &'static str,
    pub classify: fn(&Path) -> Option<String>,
}

impl pulsar_plugins::Contribution for FileTypeClassifier {
    const CATEGORY: &'static str = "fs.file_types";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.id.to_owned()
    }
}

pulsar_plugins::registration!(FileTypeClassifier);

fn registered_file_type(path: &Path) -> Option<String> {
    pulsar_plugins::registered::<FileTypeClassifier>()
        .find_map(|classifier| (classifier.classify)(path))
}
