//! SQLite schema creation and migration for the file index.

use anyhow::Result;
use rusqlite::Connection;

use super::*;

impl ProjectFileIndex {
    pub(super) fn initialize_schema(connection: &Connection) -> Result<()> {
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
        if version < 4 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS asset_deps (
                    source_path TEXT NOT NULL REFERENCES files(path) ON DELETE CASCADE,
                    source_kind TEXT NOT NULL,
                    source_id TEXT NOT NULL,
                    target_kind TEXT NOT NULL,
                    target_id TEXT NOT NULL,
                    role TEXT NOT NULL,
                    via TEXT NOT NULL,
                    expect TEXT NOT NULL DEFAULT '',
                    PRIMARY KEY (source_path, source_kind, source_id, target_kind, target_id, via)
                 );
                 CREATE INDEX IF NOT EXISTS asset_deps_target_idx ON asset_deps(target_kind, target_id);
                 CREATE INDEX IF NOT EXISTS asset_deps_source_idx ON asset_deps(source_kind, source_id);
                 PRAGMA user_version = 4;
                 COMMIT;",
            )?;
        }
        Ok(())
    }
}
