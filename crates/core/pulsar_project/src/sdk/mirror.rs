//! The SDK mirror: the index's packages copied from the SDK into a
//! destination (a project's `.pulsar/sdk/`, or an install's exported `sdk/`)
//! under their SDK-relative directories, manifests normalized.
//!
//! Incremental. [`MirrorRecord`] (`sdk.json` at the destination's root,
//! written last) remembers, per file, the source's size and modification
//! time and the size of what was written. A file is rewritten when its
//! source changed, when its output is missing or a different size, or (for
//! manifests) when the index changed; files the SDK no longer has are
//! deleted. An interrupted run leaves the old record, so the next run
//! compares content and finishes the work.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::fs;
use super::index::SdkIndex;
use super::normalize;
use crate::paths;

/// Bumped when the mirror's layout or normalization changes: a record of
/// another format is ignored (every file is compared again).
pub const MIRROR_FORMAT: u32 = 1;

/// The record's file name at the root of a mirror.
pub const RECORD_FILE: &str = "sdk.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MirrorRecord {
    pub format: u32,
    /// The index's fingerprint when the mirror was made.
    pub fingerprint: String,
    /// The key of the checkout state the index was built from
    /// (`EngineSdk::index`); `None` for an exported SDK.
    pub index_key: Option<String>,
    pub index: SdkIndex,
    /// Destination-relative path → stamps.
    pub files: BTreeMap<String, FileStamp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    pub source_size: u64,
    pub source_modified_ns: u128,
    pub output_size: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MirrorReport {
    pub written: usize,
    pub unchanged: usize,
    pub deleted: usize,
}

impl MirrorRecord {
    /// The record at `dest`, if it is readable and of this format.
    pub fn read(dest: &Path) -> Option<Self> {
        let bytes = fs::read_if_exists(&dest.join(RECORD_FILE)).ok()??;
        let record: Self = serde_json::from_slice(&bytes).ok()?;
        (record.format == MIRROR_FORMAT).then_some(record)
    }
}

/// Mirror `index`'s packages from `source` (the SDK root) into `dest`.
/// `force` compares every file's content instead of trusting the record, and
/// removes every file the mirror does not have.
pub fn mirror(
    source: &Path,
    index: &SdkIndex,
    index_key: Option<String>,
    dest: &Path,
    force: bool,
) -> Result<MirrorReport, String> {
    let previous = MirrorRecord::read(dest).filter(|_| !force);
    let fingerprint = index.fingerprint();
    let index_changed = previous
        .as_ref()
        .is_none_or(|record| record.fingerprint != fingerprint);

    let mut report = MirrorReport::default();
    let mut files: BTreeMap<String, FileStamp> = BTreeMap::new();
    for package in &index.packages {
        let package_dir = paths::join(source, &package.dir);
        let manifest_rel = format!("{}/Cargo.toml", package.dir);
        for found in fs::walk_package(&package_dir)? {
            let rel = format!("{}/{}", package.dir, found.rel);
            if rel == RECORD_FILE {
                return Err(format!("the SDK has a file named {RECORD_FILE} at its root"));
            }
            let is_manifest = rel == manifest_rel;
            let (source_size, source_modified_ns) = fs::stamp(&found.path)?;
            let out_path = paths::join(dest, &rel);
            let kept = previous
                .as_ref()
                .and_then(|record| record.files.get(&rel))
                .filter(|stamp| {
                    stamp.source_size == source_size
                        && stamp.source_modified_ns == source_modified_ns
                        && (!is_manifest || !index_changed)
                        && fs::size(&out_path) == Some(stamp.output_size)
                });
            if let Some(stamp) = kept {
                files.insert(rel, *stamp);
                report.unchanged += 1;
                continue;
            }
            let bytes = fs::read(&found.path)?;
            let bytes = if is_manifest {
                let text = String::from_utf8(bytes)
                    .map_err(|_| format!("{}: not UTF-8", found.path.display()))?;
                normalize::normalize(&text, &package.dir, index.workspace_of(package))
                    .map_err(|e| format!("{}: {e}", found.path.display()))?
                    .into_bytes()
            } else {
                bytes
            };
            if fs::read_if_exists(&out_path)?.as_deref() != Some(bytes.as_slice()) {
                fs::write(&out_path, &bytes)?;
                report.written += 1;
            } else {
                report.unchanged += 1;
            }
            files.insert(
                rel,
                FileStamp {
                    source_size,
                    source_modified_ns,
                    output_size: bytes.len() as u64,
                },
            );
        }
        if !files.contains_key(&manifest_rel) {
            return Err(format!(
                "{}: the SDK's package `{}` has no Cargo.toml",
                package_dir.display(),
                package.name
            ));
        }
    }

    // What the mirror no longer has: the previous record's files, or (when
    // forced, or without a record) everything on disk.
    let stale: Vec<String> = match &previous {
        Some(record) => record
            .files
            .keys()
            .filter(|rel| !files.contains_key(*rel))
            .cloned()
            .collect(),
        None => fs::all_files(dest)?
            .into_iter()
            .filter(|rel| rel != RECORD_FILE && !files.contains_key(rel))
            .collect(),
    };
    for rel in stale {
        fs::remove_file_and_empty_parents(&paths::join(dest, &rel), dest)?;
        report.deleted += 1;
    }

    let record = MirrorRecord {
        format: MIRROR_FORMAT,
        fingerprint,
        index_key,
        index: index.clone(),
        files,
    };
    let json = serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?;
    fs::write(&dest.join(RECORD_FILE), &json)?;
    Ok(report)
}
