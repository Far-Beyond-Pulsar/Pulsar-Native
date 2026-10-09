//! Finding importable sources and out-of-date links in a project.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::db::{relative_key, ImportDb, ImportRecord, LinkStatus};
use crate::importer::importer_for;

/// What a project scan found.
#[derive(Debug, Default)]
pub struct ScanReport {
    /// Importable sources with no record and not ignored, grouped by
    /// lower-case extension so each format is configured once.
    pub unlinked: BTreeMap<String, Vec<PathBuf>>,
    /// Linked sources whose file changed since they were imported.
    pub out_of_date: Vec<ImportRecord>,
    /// Records whose source file no longer exists.
    pub missing: Vec<ImportRecord>,
}

impl ScanReport {
    pub fn is_empty(&self) -> bool {
        self.unlinked.is_empty() && self.out_of_date.is_empty() && self.missing.is_empty()
    }
}

fn skipped_dir(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "target" | "node_modules")
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => {
                if !skipped_dir(&name) {
                    collect(&path, out);
                }
            }
            Ok(kind) if kind.is_file() => out.push(path),
            _ => {}
        }
    }
}

/// Scan `project_root`. Does blocking disk I/O (and hashes linked sources);
/// call it off the UI thread.
pub fn scan_project(project_root: &Path) -> ScanReport {
    let db = ImportDb::load(project_root);
    let mut report = ScanReport::default();

    let mut files = Vec::new();
    collect(project_root, &mut files);
    files.sort();
    for file in files {
        let Some(ext) = file.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let ext = ext.to_ascii_lowercase();
        if importer_for(&ext).is_none() {
            continue;
        }
        let key = relative_key(project_root, &file);
        if db.find_source(&key).is_some() || db.is_ignored(&key) {
            continue;
        }
        report.unlinked.entry(ext).or_default().push(file);
    }

    for record in &db.records {
        match db.status(project_root, record) {
            LinkStatus::Current => {}
            LinkStatus::OutOfDate => report.out_of_date.push(record.clone()),
            LinkStatus::SourceMissing => report.missing.push(record.clone()),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_unlinked_sources_by_format_and_skips_known_ones() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("models")).unwrap();
        std::fs::create_dir_all(root.join(".pulsar")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        for name in [
            "models/a.fbx",
            "models/b.fbx",
            "models/c.obj",
            "models/t.png",
            ".pulsar/x.fbx",
            "target/y.fbx",
            "models/linked.fbx",
            "models/no.fbx",
        ] {
            std::fs::write(root.join(name), name.as_bytes()).unwrap();
        }
        let mut db = ImportDb::default();
        db.upsert(ImportRecord {
            source: "models/linked.fbx".into(),
            source_hash: "stale".into(),
            source_size: 1,
            asset: "models/linked.mesh".into(),
            importer: "mesh".into(),
        });
        db.ignore("models/no.fbx");
        db.save(root).unwrap();

        let report = scan_project(root);
        let fbx: Vec<_> = report.unlinked["fbx"]
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(fbx, ["a.fbx", "b.fbx"]);
        assert_eq!(report.unlinked["obj"].len(), 1);
        assert!(!report.unlinked.contains_key("png"));
        assert_eq!(report.out_of_date.len(), 1);
        assert!(report.missing.is_empty());
    }
}
