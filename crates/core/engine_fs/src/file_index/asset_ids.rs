//! Stable asset ids: the indexed id table, the durable `.pulsar/asset_ids.json`
//! manifest behind it, and lookups by id.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use rusqlite::params;
use std::path::{Path, PathBuf};

use super::*;

pub(super) const ASSET_ID_MANIFEST: &str = ".pulsar/asset_ids.json";
static ASSET_ID_MANIFEST_LOCK: OnceLock<parking_lot::Mutex<()>> = OnceLock::new();

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct DurableAssetIdManifest {
    version: u32,
    /// Project-relative path -> asset kind -> stable ID.
    assets: BTreeMap<String, BTreeMap<String, String>>,
}

impl ProjectFileIndex {
    /// Associate a plugin-defined stable id with an indexed project file.
    /// Recording another id for the same path replaces the previous value.
    #[track_caller]
    pub fn record_asset_id(&self, id: &str, path: &Path) -> Result<()> {
        editor_task_queue::blocking_call("file_index::record_asset_id");
        let Some(relative) = self.relative_path(path) else {
            anyhow::bail!("asset id path is outside the indexed project root");
        };
        let connection = self.connection.lock();
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
    #[track_caller]
    pub fn record_typed_asset_id(&self, kind: &str, id: &str, path: &Path) -> Result<()> {
        editor_task_queue::blocking_call("file_index::record_typed_asset_id");
        let Some(relative) = self.relative_path(path) else {
            anyhow::bail!("asset id path is outside the indexed project root");
        };
        let kind = kind.trim();
        if kind.is_empty() {
            anyhow::bail!("typed asset kind cannot be empty");
        }
        let connection = self.connection.lock();
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
    #[track_caller]
    pub fn ensure_typed_asset_id(&self, kind: &str, path: &Path) -> Result<String> {
        editor_task_queue::blocking_call("file_index::ensure_typed_asset_id");
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

    pub(super) fn remove_durable_asset_ids(&self, relative: &str) -> Result<()> {
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

    pub(super) fn move_durable_asset_ids(&self, from: &str, to: &str) -> Result<()> {
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

    pub(super) fn restore_durable_asset_ids(&self) -> Result<()> {
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
            // An id can be given to a file the index does not list (a
            // thumbnail under `.pulsar/`, a file under `target/`): live, the
            // file is refreshed in first, but reconcile drops it again, and
            // `asset_ids.path` references `files.path`. Such an entry has no
            // row to hang from; skip it rather than fail the whole open.
            if !self.lists(&path)? {
                tracing::debug!(path = %path.display(), "durable asset id names a file the index does not list; not restored");
                continue;
            }
            for (kind, id) in kinds {
                self.record_typed_asset_id(&kind, &id, &path)?;
            }
        }
        Ok(())
    }

    /// Whether the index has a `files` row for `path`.
    fn lists(&self, path: &Path) -> Result<bool> {
        let Some(relative) = self.relative_path(path) else {
            return Ok(false);
        };
        let connection = self.connection.lock();
        let mut statement = connection.prepare_cached("SELECT 1 FROM files WHERE path = ?1")?;
        let listed = statement.exists([&relative])?;
        Ok(listed)
    }

    /// Remove all stable-ID associations for a project file. Call this when a
    /// parsed asset no longer has a valid ID (or can no longer be read).
    #[track_caller]
    pub fn clear_asset_ids(&self, path: &Path) -> Result<()> {
        editor_task_queue::blocking_call("file_index::clear_asset_ids");
        let Some(relative) = self.relative_path(path) else {
            return Ok(());
        };
        let connection = self.connection.lock();
        connection.execute("DELETE FROM asset_ids WHERE path = ?1", [&relative])?;
        Ok(())
    }

    #[track_caller]
    pub fn clear_asset_id(&self, path: &Path) -> Result<()> {
        editor_task_queue::blocking_call("file_index::clear_asset_id");
        self.clear_asset_ids(path)
    }

    /// Return every path claiming `id`. More than one result indicates a
    /// copied asset whose stable id must be repaired by its owning plugin.
    #[track_caller]
    pub fn paths_for_asset_id(&self, id: &str) -> Result<Vec<PathBuf>> {
        editor_task_queue::blocking_call("file_index::paths_for_asset_id");
        let connection = self.connection.lock();
        let mut statement = connection
            .prepare_cached("SELECT path FROM asset_ids WHERE asset_id = ?1 ORDER BY path")?;
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
    #[track_caller]
    pub fn path_for_asset_id_kind(&self, kind: &str, id: &str) -> Result<Option<PathBuf>> {
        editor_task_queue::blocking_call("file_index::path_for_asset_id_kind");
        let connection = self.connection.lock();
        let mut statement = connection.prepare_cached(
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

    /// [`path_for_asset_id_kind`](Self::path_for_asset_id_kind) for many ids
    /// at once: one lock and one prepared statement for the whole batch (a
    /// cold start resolving a large prefab's references, #2018). Each id maps
    /// to its path when exactly one file carries it for `kind`; ids that are
    /// missing or claimed twice are absent from the map.
    #[track_caller]
    pub fn paths_for_asset_ids_kind<'a>(
        &self,
        kind: &str,
        ids: impl IntoIterator<Item = &'a str>,
    ) -> Result<BTreeMap<String, PathBuf>> {
        editor_task_queue::blocking_call("file_index::paths_for_asset_ids_kind");
        let connection = self.connection.lock();
        let mut statement = connection.prepare_cached(
            "SELECT path FROM asset_ids WHERE kind = ?1 AND asset_id = ?2 ORDER BY path LIMIT 2",
        )?;
        let mut found = BTreeMap::new();
        for id in ids {
            if found.contains_key(id) {
                continue;
            }
            let mut rows = statement.query(params![kind, id])?;
            let Some(first) = rows.next()? else {
                continue;
            };
            let relative: String = first.get(0)?;
            if rows.next()?.is_some() {
                continue;
            }
            found.insert(
                id.to_string(),
                self.project_root
                    .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR)),
            );
        }
        Ok(found)
    }

    /// Return every project path carrying `id` for `kind`.
    #[track_caller]
    pub fn paths_for_asset_id_kind(&self, kind: &str, id: &str) -> Result<Vec<PathBuf>> {
        editor_task_queue::blocking_call("file_index::paths_for_asset_id_kind");
        let connection = self.connection.lock();
        let mut statement = connection.prepare_cached(
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

    /// Every indexed `(asset id, path)` of `kind`: the candidate list an asset
    /// picker filters. Ambiguous ids appear once per claiming path.
    #[track_caller]
    pub fn assets_of_kind(&self, kind: &str) -> Result<Vec<(String, PathBuf)>> {
        editor_task_queue::blocking_call("file_index::assets_of_kind");
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT asset_id, path FROM asset_ids WHERE kind = ?1 ORDER BY path, asset_id",
        )?;
        let rows = statement.query_map([kind], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            row.map(|(id, relative)| {
                (
                    id,
                    self.project_root
                        .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR)),
                )
            })
        })
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("query assets of kind")
    }

    /// Return stable asset IDs associated with one indexed project file.
    #[track_caller]
    pub fn asset_ids_for_path(&self, path: &Path) -> Result<Vec<String>> {
        editor_task_queue::blocking_call("file_index::asset_ids_for_path");
        let Some(relative) = self.relative_path(path) else {
            anyhow::bail!("asset id path is outside the indexed project root");
        };
        let connection = self.connection.lock();
        let mut statement = connection
            .prepare("SELECT asset_id FROM asset_ids WHERE path = ?1 ORDER BY asset_id")?;
        let rows = statement.query_map([relative], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("query asset ids for file")
    }

    /// Return stable asset IDs of `kind` associated with one indexed file.
    #[track_caller]
    pub fn asset_ids_for_path_kind(&self, path: &Path, kind: &str) -> Result<Vec<String>> {
        editor_task_queue::blocking_call("file_index::asset_ids_for_path_kind");
        let Some(relative) = self.relative_path(path) else {
            anyhow::bail!("asset id path is outside the indexed project root");
        };
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT asset_id FROM asset_ids WHERE path = ?1 AND kind = ?2 ORDER BY asset_id",
        )?;
        let rows = statement.query_map(params![relative, kind], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("query typed asset ids for file")
    }
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
