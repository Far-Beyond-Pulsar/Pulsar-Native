//! How individual project files relate to the import database, for UI that
//! shows a file's import actions and an out-of-date badge (the content
//! drawer).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::db::{relative_key, ImportDb, LinkStatus};
use crate::importer::importer_for;

/// Where a file stands with the import database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileImportState {
    /// An importable source with no record. A scan offers it.
    Unlinked,
    /// An importable source the user chose not to import ("Don't import").
    Ignored,
    /// A linked source.
    LinkedSource { status: LinkStatus },
    /// A native asset built from a linked source (record key `source`).
    LinkedAsset { source: String, status: LinkStatus },
}

impl FileImportState {
    /// The record key of the linked source, for a linked source or asset.
    pub fn linked_source(&self, project_root: &Path, file: &Path) -> Option<String> {
        match self {
            Self::LinkedSource { .. } => Some(relative_key(project_root, file)),
            Self::LinkedAsset { source, .. } => Some(source.clone()),
            Self::Unlinked | Self::Ignored => None,
        }
    }

    /// Whether the linked source changed since it was imported.
    pub fn is_out_of_date(&self) -> bool {
        matches!(
            self,
            Self::LinkedSource {
                status: LinkStatus::OutOfDate
            } | Self::LinkedAsset {
                status: LinkStatus::OutOfDate,
                ..
            }
        )
    }
}

/// The import state of each of `files` that has one. Files that are neither
/// importable nor a linked asset are left out.
///
/// Reads the database once and hashes each linked source at most once (a
/// source and its asset share the result), so it does blocking I/O: call it
/// off the UI thread.
pub fn import_states(project_root: &Path, files: &[PathBuf]) -> HashMap<PathBuf, FileImportState> {
    let db = ImportDb::load(project_root);
    let mut statuses: HashMap<String, LinkStatus> = HashMap::new();
    let mut status_of = |source: &str| {
        *statuses.entry(source.to_owned()).or_insert_with(|| {
            let record = db.find_source(source).expect("status of a recorded source");
            db.status(project_root, record)
        })
    };

    let mut states = HashMap::new();
    for file in files {
        let key = relative_key(project_root, file);
        let state = if db.find_source(&key).is_some() {
            FileImportState::LinkedSource {
                status: status_of(&key),
            }
        } else if let Some(record) = db.find_asset(&key) {
            let source = record.source.clone();
            FileImportState::LinkedAsset {
                status: status_of(&source),
                source,
            }
        } else if is_importable(file) {
            if db.is_ignored(&key) {
                FileImportState::Ignored
            } else {
                FileImportState::Unlinked
            }
        } else {
            continue;
        };
        states.insert(file.clone(), state);
    }
    states
}

/// Whether `file` is a format some importer handles.
pub fn is_importable(file: &Path) -> bool {
    file.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| importer_for(&ext.to_ascii_lowercase()).is_some())
}

static REVISION: AtomicU64 = AtomicU64::new(0);

/// Bumped whenever this process writes the import database, so views that
/// show [`import_states`] know to recompute them. Changes to source files
/// themselves are not counted: watch the files for those.
pub fn db_revision() -> u64 {
    REVISION.load(Ordering::Acquire)
}

pub(crate) fn bump_db_revision() {
    REVISION.fetch_add(1, Ordering::AcqRel);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{hash_file, ImportRecord};

    #[test]
    fn each_file_gets_its_relation_to_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in [
            "linked.fbx",
            "linked.mesh",
            "new.obj",
            "skipped.fbx",
            "notes.txt",
        ] {
            std::fs::write(root.join(name), name).unwrap();
        }
        let (source_hash, source_size) = hash_file(&root.join("linked.fbx")).unwrap();
        let mut db = ImportDb::default();
        db.upsert(ImportRecord {
            source: "linked.fbx".into(),
            source_hash,
            source_size,
            asset: "linked.mesh".into(),
            importer: "mesh".into(),
            options: None,
        });
        db.ignore("skipped.fbx");
        db.save(root).unwrap();

        let files: Vec<PathBuf> = [
            "linked.fbx",
            "linked.mesh",
            "new.obj",
            "skipped.fbx",
            "notes.txt",
        ]
        .iter()
        .map(|name| root.join(name))
        .collect();
        let states = import_states(root, &files);
        let current = LinkStatus::Current;
        assert_eq!(
            states[&root.join("linked.fbx")],
            FileImportState::LinkedSource { status: current }
        );
        assert_eq!(
            states[&root.join("linked.mesh")],
            FileImportState::LinkedAsset {
                source: "linked.fbx".into(),
                status: current
            }
        );
        assert_eq!(states[&root.join("new.obj")], FileImportState::Unlinked);
        assert_eq!(states[&root.join("skipped.fbx")], FileImportState::Ignored);
        assert!(!states.contains_key(&root.join("notes.txt")));

        std::fs::write(root.join("linked.fbx"), "edited").unwrap();
        let states = import_states(root, &files);
        assert!(states[&root.join("linked.fbx")].is_out_of_date());
        assert!(
            states[&root.join("linked.mesh")].is_out_of_date(),
            "the asset shows its source's staleness"
        );
        assert_eq!(
            states[&root.join("linked.mesh")]
                .linked_source(root, &root.join("linked.mesh"))
                .as_deref(),
            Some("linked.fbx")
        );
    }
}
