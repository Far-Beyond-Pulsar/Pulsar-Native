//! Applying a rendered [`FileSet`] to a project, against the generation
//! record (`.pulsar/build/generated.json`): what the generator wrote last
//! time, and the hash of each file.
//!
//! - A file whose content differs is replaced atomically.
//! - A file the record has and the set no longer does is deleted.
//! - A file that differs from the hash the record says was written was
//!   edited by hand: it is replaced and reported as drift.
//! - A file that exists, differs, and is **not** in the record was not made
//!   by this generator (a project from before it, or the user's own): it is
//!   never overwritten; the sync fails and says so.
//!
//! The record is written last by the caller, so an interrupted sync is
//! redone in full by the next one.

use std::collections::BTreeMap;
use std::path::Path;

use engine_fs::virtual_fs;
use serde::{Deserialize, Serialize};

use crate::error::SyncError;
use crate::hash::sha256_hex;
use crate::paths;
use crate::render::FileSet;

/// Bumped when the generated layout changes in a way a record of the old
/// format cannot describe.
pub const RECORD_FORMAT: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationRecord {
    pub format: u32,
    /// The SDK index fingerprint the files were generated from (the
    /// lockfile is seeded again when it changes).
    pub engine: String,
    /// Project-relative path → SHA-256 of the content written.
    pub files: BTreeMap<String, String>,
}

impl GenerationRecord {
    /// The project's record, or `None` without one (or with one of another
    /// format: nothing it lists is trusted).
    pub fn read(project_root: &Path) -> Result<Option<Self>, SyncError> {
        let path = paths::join(project_root, paths::GENERATION_RECORD);
        if !virtual_fs::is_file(&path) {
            return Ok(None);
        }
        let text = virtual_fs::read_to_string(&path).map_err(|e| SyncError::io(&path, e))?;
        Ok(serde_json::from_str::<Self>(&text)
            .ok()
            .filter(|r| r.format == RECORD_FORMAT))
    }

    pub fn write(&self, project_root: &Path) -> Result<(), SyncError> {
        let json = serde_json::to_string_pretty(self).expect("the record serializes");
        write(&paths::join(project_root, paths::GENERATION_RECORD), json.as_bytes())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyReport {
    pub written: Vec<String>,
    pub deleted: Vec<String>,
    /// Files edited by hand since the last sync (replaced or deleted).
    pub drifted: Vec<String>,
}

/// Apply `files` to the project; returns what changed and the new record's
/// file hashes. Checks every file before writing any, so a refused file
/// leaves the project untouched.
pub fn apply(
    project_root: &Path,
    files: &FileSet,
    previous: Option<&GenerationRecord>,
) -> Result<(ApplyReport, BTreeMap<String, String>), SyncError> {
    let recorded = |rel: &str| previous.and_then(|r| r.files.get(rel));
    let mut report = ApplyReport::default();
    let mut pending: Vec<(&String, &Vec<u8>)> = Vec::new();

    for (rel, content) in files {
        let path = paths::join(project_root, rel);
        let current = if virtual_fs::is_file(&path) {
            Some(virtual_fs::read_file(&path).map_err(|e| SyncError::io(&path, e))?)
        } else {
            None
        };
        match (&current, recorded(rel)) {
            (Some(current), _) if current == content => continue,
            (Some(current), Some(hash)) => {
                if sha256_hex(current) != *hash {
                    report.drifted.push(rel.clone());
                }
            }
            (Some(_), None) => {
                return Err(SyncError::Unowned {
                    path: rel.clone(),
                });
            }
            (None, _) => {}
        }
        pending.push((rel, content));
    }

    for (rel, content) in pending {
        write(&paths::join(project_root, rel), content)?;
        report.written.push(rel.clone());
    }

    if let Some(previous) = previous {
        for (rel, hash) in &previous.files {
            if files.contains_key(rel) {
                continue;
            }
            let path = paths::join(project_root, rel);
            if !virtual_fs::is_file(&path) {
                continue;
            }
            let current = virtual_fs::read_file(&path).map_err(|e| SyncError::io(&path, e))?;
            if sha256_hex(&current) != *hash {
                report.drifted.push(rel.clone());
            }
            virtual_fs::delete_path(&path).map_err(|e| SyncError::io(&path, e))?;
            report.deleted.push(rel.clone());
        }
    }

    let hashes = files
        .iter()
        .map(|(rel, content)| (rel.clone(), sha256_hex(content)))
        .collect();
    Ok((report, hashes))
}

/// Replace a project file atomically (Cargo and rust-analyzer read these
/// files while the editor writes them). Remote and peer-to-peer providers
/// have no atomic replace; they get a plain write.
pub(crate) fn write(path: &Path, content: &[u8]) -> Result<(), SyncError> {
    if let Some(parent) = path.parent() {
        virtual_fs::create_dir_all(parent).map_err(|e| SyncError::io(parent, e))?;
    }
    match virtual_fs::write_file_atomically(path, content) {
        Ok(()) => Ok(()),
        Err(_) if virtual_fs::is_remote() => {
            virtual_fs::write_file(path, content).map_err(|e| SyncError::io(path, e))
        }
        Err(error) => Err(SyncError::io(path, error)),
    }
}
