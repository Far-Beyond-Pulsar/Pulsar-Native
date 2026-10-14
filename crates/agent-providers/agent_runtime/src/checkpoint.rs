//! Per-run file checkpoints.
//!
//! Before a write tool runs, the files it names are copied (through the VFS)
//! into `.pulsar/agent_checkpoints/<run>/`. The first snapshot of a path in a
//! run wins, so reverting restores the state from before the run began.
//! Commands can touch files we cannot predict; only files named in a call's
//! arguments are covered.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use engine_fs::virtual_fs;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::taint::path_args;

pub const CHECKPOINT_DIR: &str = ".pulsar/agent_checkpoints";
const MANIFEST: &str = "manifest.json";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct Manifest {
    run_id: String,
    entries: Vec<ManifestItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ManifestItem {
    /// Absolute path of the file the agent may change.
    path: String,
    /// The file existed before the run; if not, revert deletes it.
    existed: bool,
    /// Snapshot file under the checkpoint dir, when `existed`.
    snapshot: Option<String>,
}

/// What a revert did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RevertReport {
    pub restored: Vec<PathBuf>,
    /// Files the run created, now deleted.
    pub removed: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
}

impl RevertReport {
    pub fn is_clean(&self) -> bool {
        self.failed.is_empty()
    }

    pub fn summary(&self) -> String {
        let mut text = format!(
            "Restored {} file(s), removed {} created file(s)",
            self.restored.len(),
            self.removed.len()
        );
        if !self.failed.is_empty() {
            text.push_str(&format!(", {} failed", self.failed.len()));
        }
        text.push('.');
        text
    }
}

fn run_dir(workspace_root: &Path, run_id: &str) -> Result<PathBuf, String> {
    let valid = !run_id.is_empty()
        && run_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && run_id != "."
        && run_id != "..";
    if !valid {
        return Err(format!("Invalid run id '{run_id}'."));
    }
    Ok(workspace_root.join(CHECKPOINT_DIR).join(run_id))
}

/// Paths a call may change, resolved against the workspace root.
pub fn affected_paths(args: &Value, workspace_root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for raw in path_args(args) {
        let path = Path::new(&raw);
        let resolved = if path.is_absolute() || raw.starts_with("cloud+pulsar://") {
            path.to_path_buf()
        } else {
            workspace_root.join(path)
        };
        if !out.contains(&resolved) {
            out.push(resolved);
        }
    }
    out
}

/// Snapshots for one run. Clones share state, so tool threads can all use it.
#[derive(Clone)]
pub struct CheckpointStore {
    workspace_root: PathBuf,
    dir: PathBuf,
    manifest: Arc<Mutex<Manifest>>,
}

impl CheckpointStore {
    pub fn new(workspace_root: &Path, run_id: &str) -> Result<Self, String> {
        let dir = run_dir(workspace_root, run_id)?;
        // Continue an earlier session of the same run if there is one.
        let manifest = read_manifest(&dir).unwrap_or_else(|_| Manifest {
            run_id: run_id.to_string(),
            entries: Vec::new(),
        });
        Ok(Self {
            workspace_root: workspace_root.to_path_buf(),
            dir,
            manifest: Arc::new(Mutex::new(manifest)),
        })
    }

    pub fn run_dir(&self) -> &Path {
        &self.dir
    }

    /// Snapshot every file `args` names. Returns the paths newly captured.
    pub fn snapshot_for_call(&self, args: &Value) -> Result<Vec<PathBuf>, String> {
        self.snapshot(&affected_paths(args, &self.workspace_root))
    }

    /// Snapshot `paths` that are not yet in this run's checkpoint. Directories
    /// are skipped. Fails (without partial writes of the manifest) if a file
    /// cannot be saved, so the caller can refuse the edit.
    pub fn snapshot(&self, paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
        let mut manifest = self.manifest.lock().unwrap_or_else(|e| e.into_inner());
        let mut captured = Vec::new();
        let mut added = Vec::new();

        for path in paths {
            let key = path.to_string_lossy().to_string();
            if manifest.entries.iter().any(|item| item.path == key)
                || added.iter().any(|item: &ManifestItem| item.path == key)
            {
                continue;
            }
            let exists = virtual_fs::exists(path).unwrap_or(false);
            if exists && virtual_fs::metadata(path).map(|m| m.is_dir).unwrap_or(false) {
                continue;
            }
            let snapshot = if exists {
                let bytes = virtual_fs::read_file(path)
                    .map_err(|e| format!("Checkpoint could not read {}: {e}", path.display()))?;
                let name = format!("files/{:04}.bin", manifest.entries.len() + added.len());
                let target = self.dir.join(&name);
                if let Some(parent) = target.parent() {
                    virtual_fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                virtual_fs::write_file(&target, &bytes)
                    .map_err(|e| format!("Checkpoint could not save {}: {e}", path.display()))?;
                Some(name)
            } else {
                None
            };
            added.push(ManifestItem {
                path: key,
                existed: exists,
                snapshot,
            });
            captured.push(path.clone());
        }

        if !added.is_empty() {
            let mut next = manifest.clone();
            next.entries.extend(added);
            write_manifest(&self.dir, &next)?;
            *manifest = next;
        }
        Ok(captured)
    }

    pub fn file_count(&self) -> usize {
        self.manifest
            .lock()
            .map(|m| m.entries.len())
            .unwrap_or_default()
    }
}

fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let bytes = virtual_fs::read_file(&dir.join(MANIFEST)).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn write_manifest(dir: &Path, manifest: &Manifest) -> Result<(), String> {
    virtual_fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_vec_pretty(manifest).map_err(|e| e.to_string())?;
    virtual_fs::write_file(&dir.join(MANIFEST), &json).map_err(|e| e.to_string())
}

/// Does this run have a checkpoint to revert?
pub fn has_checkpoint(workspace_root: &Path, run_id: &str) -> bool {
    run_dir(workspace_root, run_id)
        .and_then(|dir| read_manifest(&dir))
        .map(|m| !m.entries.is_empty())
        .unwrap_or(false)
}

/// Put every file the run changed back as it was before the run's first edit.
/// Safe to call twice.
pub fn revert_checkpoint(workspace_root: &Path, run_id: &str) -> Result<RevertReport, String> {
    let dir = run_dir(workspace_root, run_id)?;
    let manifest =
        read_manifest(&dir).map_err(|e| format!("No checkpoint for run '{run_id}': {e}"))?;
    let mut report = RevertReport::default();

    for item in &manifest.entries {
        let path = PathBuf::from(&item.path);
        if item.existed {
            let restored = item
                .snapshot
                .as_ref()
                .ok_or_else(|| "snapshot missing".to_string())
                .and_then(|name| virtual_fs::read_file(&dir.join(name)).map_err(|e| e.to_string()))
                .and_then(|bytes| virtual_fs::write_file(&path, &bytes).map_err(|e| e.to_string()));
            match restored {
                Ok(()) => report.restored.push(path),
                Err(e) => report.failed.push((path, e)),
            }
        } else if virtual_fs::exists(&path).unwrap_or(false) {
            match virtual_fs::delete_path(&path) {
                Ok(()) => report.removed.push(path),
                Err(e) => report.failed.push((path, e.to_string())),
            }
        }
    }
    Ok(report)
}
