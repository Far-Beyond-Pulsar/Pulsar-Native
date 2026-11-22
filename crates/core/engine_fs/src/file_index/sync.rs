//! Bringing the index in line with the VFS: whole-manifest reconcile and
//! single-path refresh.

use anyhow::Result;
use rusqlite::params;
use std::path::Path;

use super::*;

impl ProjectFileIndex {
    /// Reconcile the index against a fresh VFS manifest. Paths whose metadata
    /// is unchanged are not rewritten; entries absent from the manifest are
    /// removed in the same transaction.
    pub fn reconcile(&self, manifest: &[ManifestEntry]) -> Result<()> {
        let mut connection = self.connection.lock();
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

    /// Refresh one local file or directory from the active VFS metadata.
    pub fn refresh_path(&self, path: &Path) -> Result<()> {
        let Some(relative) = self.relative_path(path) else {
            return Ok(());
        };
        if relative.is_empty() || is_cache_entry(&relative) {
            return Ok(());
        }
        let metadata = match crate::virtual_fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => return self.remove_path(path),
        };
        let entry = ManifestEntry {
            path: relative,
            is_dir: metadata.is_dir,
            size: metadata.size,
            modified: metadata.modified,
        };
        self.reconcile_one(&entry)?;
        if metadata.is_dir {
            let relative_root = self.relative_path(path).unwrap_or_default();
            let mut children = Vec::new();
            for mut child in crate::virtual_fs::manifest(path)? {
                child.path = if relative_root.is_empty() {
                    normalize_relative_path(&child.path)
                } else {
                    format!("{relative_root}/{}", normalize_relative_path(&child.path))
                };
                if !is_cache_entry(&child.path) {
                    children.push(child);
                }
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
        let connection = self.connection.lock();
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
}
