//! The import database: which sources are linked to which native assets.

use std::path::Path;

use anyhow::{Context, Result};
use engine_fs::virtual_fs;
use serde::{Deserialize, Serialize};

const DIR: &str = ".pulsar";
const FILE: &str = "import_db.json";
const VERSION: u32 = 1;

/// One linked source and the native asset built from it. Paths are relative
/// to the project root, forward-slashed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportRecord {
    pub source: String,
    /// xxh3-128 of the source bytes at the last import, lower-case hex.
    pub source_hash: String,
    pub source_size: u64,
    pub asset: String,
    /// [`crate::Importer::id`] of the importer that produced `asset`.
    pub importer: String,
}

/// Whether a linked source still matches what was imported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkStatus {
    Current,
    OutOfDate,
    SourceMissing,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportDb {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub records: Vec<ImportRecord>,
    /// Sources the user chose not to import; the scan does not offer them again.
    #[serde(default)]
    pub ignored: Vec<String>,
}

fn path(project_root: &Path) -> std::path::PathBuf {
    project_root.join(DIR).join(FILE)
}

/// Project-relative, forward-slashed key for `file`.
pub fn relative_key(project_root: &Path, file: &Path) -> String {
    engine_fs::import_options::asset_key(project_root, file)
}

/// Content hash and size of `file`.
pub fn hash_file(file: &Path) -> Result<(String, u64)> {
    let bytes = virtual_fs::read_file(file).with_context(|| format!("read {}", file.display()))?;
    let hash = twox_hash::XxHash3_128::oneshot(&bytes);
    Ok((format!("{hash:032x}"), bytes.len() as u64))
}

impl ImportDb {
    /// The project's database; empty when missing or unreadable (a damaged
    /// database must never block opening a project).
    pub fn load(project_root: &Path) -> Self {
        let file = path(project_root);
        match virtual_fs::exists(&file) {
            Ok(true) => virtual_fs::read_file(&file)
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default(),
            _ => Self::default(),
        }
    }

    pub fn save(&self, project_root: &Path) -> Result<()> {
        virtual_fs::create_dir_all(&project_root.join(DIR)).context("create .pulsar dir")?;
        let mut out = self.clone();
        out.version = VERSION;
        let bytes = serde_json::to_vec_pretty(&out).context("serialize import db")?;
        virtual_fs::write_file_atomically(&path(project_root), &bytes).context("write import db")
    }

    pub fn find_source(&self, source: &str) -> Option<&ImportRecord> {
        self.records.iter().find(|r| r.source == source)
    }

    pub fn find_asset(&self, asset: &str) -> Option<&ImportRecord> {
        self.records.iter().find(|r| r.asset == asset)
    }

    pub fn is_ignored(&self, source: &str) -> bool {
        self.ignored.iter().any(|s| s == source)
    }

    /// Insert or replace the record for `record.source`; linking clears any
    /// earlier ignore.
    pub fn upsert(&mut self, record: ImportRecord) {
        self.ignored.retain(|s| *s != record.source);
        match self.records.iter_mut().find(|r| r.source == record.source) {
            Some(existing) => *existing = record,
            None => self.records.push(record),
        }
    }

    pub fn remove_source(&mut self, source: &str) {
        self.records.retain(|r| r.source != source);
    }

    pub fn ignore(&mut self, source: &str) {
        if !self.is_ignored(source) {
            self.ignored.push(source.to_owned());
        }
    }

    /// Compare a record against its source on disk.
    pub fn status(&self, project_root: &Path, record: &ImportRecord) -> LinkStatus {
        let file = project_root.join(&record.source);
        match virtual_fs::exists(&file) {
            Ok(true) => {}
            _ => return LinkStatus::SourceMissing,
        }
        match hash_file(&file) {
            Ok((hash, size)) if hash == record.source_hash && size == record.source_size => {
                LinkStatus::Current
            }
            Ok(_) => LinkStatus::OutOfDate,
            Err(_) => LinkStatus::SourceMissing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(source: &str, hash: &str) -> ImportRecord {
        ImportRecord {
            source: source.into(),
            source_hash: hash.into(),
            source_size: 3,
            asset: "m/a.mesh".into(),
            importer: "mesh".into(),
        }
    }

    #[test]
    fn roundtrip_upsert_and_ignore() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ImportDb::load(dir.path());
        assert!(db.records.is_empty());
        db.ignore("m/a.fbx");
        db.upsert(record("m/a.fbx", "1"));
        assert!(!db.is_ignored("m/a.fbx"), "linking clears the ignore");
        db.upsert(record("m/a.fbx", "2"));
        assert_eq!(db.records.len(), 1);
        db.save(dir.path()).unwrap();
        let loaded = ImportDb::load(dir.path());
        assert_eq!(loaded.find_source("m/a.fbx").unwrap().source_hash, "2");
        assert!(loaded.find_asset("m/a.mesh").is_some());
    }

    #[test]
    fn status_follows_the_source_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.fbx");
        std::fs::write(&file, b"abc").unwrap();
        let (hash, size) = hash_file(&file).unwrap();
        let rec = ImportRecord {
            source_hash: hash,
            source_size: size,
            ..record("a.fbx", "")
        };
        let db = ImportDb::default();
        assert_eq!(db.status(dir.path(), &rec), LinkStatus::Current);
        std::fs::write(&file, b"abd").unwrap();
        assert_eq!(db.status(dir.path(), &rec), LinkStatus::OutOfDate);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(db.status(dir.path(), &rec), LinkStatus::SourceMissing);
    }
}
