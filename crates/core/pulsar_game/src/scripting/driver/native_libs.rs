//! Native extension libraries of a project (`native/<crate>/target/...`):
//! finding the built cdylibs, loading them at startup and relinking script
//! classes when Cargo replaces one. Entry points: the driver methods
//! `load_project_native_libraries` and `reload_changed_native_libraries`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::ScriptDriver;

/// Dynamic libraries in each native crate's direct Cargo profile directory.
/// `deps/` is deliberately excluded: Cargo places transitive dependencies and
/// proc-macro DLLs there, and they are not Pulsar native extensions.
pub(super) fn project_native_library_artifacts(root: &Path) -> Vec<PathBuf> {
    fn visit_target(dir: &Path, out: &mut Vec<(String, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_file() {
                continue;
            }
            let name = entry.file_name();
            if name == "deps" || name == "incremental" || name == "build" {
                continue;
            }
            if name == "debug" || name == "release" {
                let profile = name.to_string_lossy().into_owned();
                let Ok(files) = std::fs::read_dir(path) else {
                    continue;
                };
                for file in files.flatten() {
                    let artifact = file.path();
                    if file.file_type().is_ok_and(|t| t.is_file())
                        && artifact.extension().is_some_and(|ext| {
                            ext.eq_ignore_ascii_case(std::env::consts::DLL_EXTENSION)
                        })
                    {
                        out.push((profile.clone(), artifact));
                    }
                }
            } else if kind.is_dir() {
                // Cross-compiled artifacts use target/<triple>/<profile>.
                visit_target(&path, out);
            }
        }
    }

    fn visit(dir: &Path, out: &mut Vec<(String, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_dir() {
                continue;
            }
            if path.file_name().is_some_and(|name| name == "target") {
                visit_target(&path, out);
                continue;
            }
            visit(&path, out);
        }
    }
    let mut candidates = Vec::new();
    if root.is_dir() {
        visit(root, &mut candidates);
    }
    // Debug and release outputs register identical native names. Load only
    // the profile matching the running game; use the other when that crate
    // has not yet produced the preferred profile.
    let preferred = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    candidates.sort_by(|(left_profile, left_path), (right_profile, right_path)| {
        let left_preferred = left_profile == preferred;
        let right_preferred = right_profile == preferred;
        right_preferred
            .cmp(&left_preferred)
            .then_with(|| left_path.cmp(right_path))
    });
    let mut selected = HashMap::<PathBuf, PathBuf>::new();
    for (_, path) in candidates {
        selected
            .entry(native_library_key(root, &path))
            .or_insert(path);
    }
    let mut artifacts: Vec<_> = selected.into_values().collect();
    artifacts.sort();
    artifacts
}

/// Stable identity for one native crate across debug/release profile paths.
pub(super) fn native_library_key(native_root: &Path, artifact: &Path) -> PathBuf {
    let crate_root = artifact
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == "target"))
        .and_then(Path::parent)
        .unwrap_or(native_root);
    let mut key = crate_root
        .strip_prefix(native_root)
        .unwrap_or(crate_root)
        .to_owned();
    if let Some(stem) = artifact.file_stem() {
        key.push(stem);
    }
    key
}

pub(super) fn log_relink_report(report: pulsar_script_runtime::RelinkReport) {
    for class in report.relinked {
        tracing::debug!(class, "relinked script class after native library change");
    }
    for (class, error) in report.failed {
        tracing::error!(
            class,
            "script class failed to relink after native library change: {error}"
        );
    }
}

impl ScriptDriver {
    /// Load the project's built cdylibs from `native/<crate>/target/{debug,release}`.
    /// This layout keeps native extension artifacts separate from the engine's
    /// own Cargo target directory and works for a single native crate or many.
    pub(super) fn load_project_native_libraries(&mut self) {
        let native_root = self.project_root.join("native");
        for path in project_native_library_artifacts(&native_root) {
            let modified = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            let key = native_library_key(&native_root, &path);
            match self.runtime.load_library(&path) {
                Ok((id, report)) => {
                    log_relink_report(report);
                    self.native_libraries.insert(key, (id, path, modified));
                }
                Err(error) => {
                    tracing::warn!(path = %path.display(), "could not load project native library: {error}")
                }
            }
        }
    }

    /// Pick up completed Cargo builds while the game or PIE session is running.
    /// A failed reload leaves the old library registered and is retried after
    /// the artifact changes again.
    pub(super) fn reload_changed_native_libraries(&mut self) {
        let native_root = self.project_root.join("native");
        let artifacts = project_native_library_artifacts(&native_root);
        let mut seen = HashSet::new();
        for path in artifacts {
            let modified = match std::fs::metadata(&path).and_then(|meta| meta.modified()) {
                Ok(modified) => modified,
                Err(error) => {
                    tracing::debug!(path = %path.display(), "cannot inspect native library: {error}");
                    continue;
                }
            };
            let key = native_library_key(&native_root, &path);
            seen.insert(key.clone());
            let Some((id, previous_path, previous)) = self.native_libraries.get(&key).cloned()
            else {
                match self.runtime.load_library(&path) {
                    Ok((id, report)) => {
                        log_relink_report(report);
                        self.native_libraries.insert(key, (id, path, modified));
                    }
                    Err(error) => {
                        tracing::warn!(path = %path.display(), "could not load project native library: {error}")
                    }
                }
                continue;
            };
            if previous_path != path {
                // The preferred profile became available after startup (for
                // example, a release build replaced a debug fallback). Swap
                // the library identity without ever registering both copies.
                self.native_libraries.remove(&key);
                match self.runtime.unload_library(id) {
                    Ok(report) => log_relink_report(report),
                    Err(error) => {
                        tracing::warn!(path = %previous_path.display(), "could not unload previous native library profile: {error}")
                    }
                }
                match self.runtime.load_library(&path) {
                    Ok((id, report)) => {
                        log_relink_report(report);
                        self.native_libraries.insert(key, (id, path, modified));
                    }
                    Err(error) => {
                        tracing::warn!(path = %path.display(), "could not load preferred native library profile: {error}")
                    }
                }
                continue;
            }
            if modified <= previous {
                continue;
            }
            match self.runtime.reload_library(id) {
                Ok(report) => {
                    log_relink_report(report);
                    self.native_libraries.insert(key, (id, path, modified));
                }
                Err(error) => {
                    tracing::warn!(path = %path.display(), "could not reload project native library: {error}")
                }
            }
        }
        let removed: Vec<_> = self
            .native_libraries
            .iter()
            .filter(|(key, _)| !seen.contains(*key))
            .map(|(key, (id, path, _))| (key.clone(), *id, path.clone()))
            .collect();
        for (key, id, path) in removed {
            self.native_libraries.remove(&key);
            match self.runtime.unload_library(id) {
                Ok(report) => log_relink_report(report),
                Err(error) => {
                    tracing::warn!(path = %path.display(), "could not unload removed project native library: {error}")
                }
            }
        }
    }
}
