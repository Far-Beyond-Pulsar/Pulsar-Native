//! The real-disk side of the SDK. The SDK is the engine's own checkout or
//! install, and its mirror under a project's `.pulsar/sdk/` is per-machine
//! input for the local Cargo, which reads the real disk: neither is a shared
//! project file, so neither goes through `engine_fs::virtual_fs` (a remote
//! project's provider would carry a machine's SDK to its peers).
//!
//! Every `std::fs` call of this crate is in this module
//! (`architecture/std-fs-allowed.txt`).
#![allow(clippy::disallowed_methods)]

use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn read_string(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The file's bytes, or `None` when it does not exist.
pub fn read_if_exists(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// `path` absolute, with `.`/`..` and links resolved, without the Windows
/// verbatim (`\\?\`) prefix Cargo does not print.
pub fn canonical(path: &Path) -> Result<PathBuf, String> {
    let resolved = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = resolved.to_string_lossy();
    Ok(match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => resolved,
    })
}

pub fn is_file(path: &Path) -> bool {
    path.is_file()
}

/// Size and modification time (nanoseconds since the epoch) of a file.
pub fn stamp(path: &Path) -> Result<(u64, u128), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    Ok((meta.len(), modified))
}

/// The size of a file, or `None` when it does not exist.
pub fn size(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len())
}

/// Write `bytes` to `path` (its directories created) through a temporary
/// file and a rename, so Cargo never reads a half-written manifest.
pub fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{}: no parent directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = parent.join(format!(".{name}.pulsar-tmp"));
    std::fs::write(&temp, bytes).map_err(|e| format!("{}: {e}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("{}: {e}", path.display())
    })
}

/// Remove a file, then each parent left empty, up to (not including) `root`.
pub fn remove_file_and_empty_parents(path: &Path, root: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", path.display())),
    }
    let mut dir = path.parent();
    while let Some(current) = dir {
        if current == root || !current.starts_with(root) {
            break;
        }
        // Fails (and stops) when the directory is not empty.
        if std::fs::remove_dir(current).is_err() {
            break;
        }
        dir = current.parent();
    }
    Ok(())
}

/// One file found by [`walk_package`].
pub struct Found {
    /// Relative to the package directory, forward slashes.
    pub rel: String,
    pub path: PathBuf,
}

/// The files of the package at `dir` a build can need: everything except
/// build output and version control (`target/`, dot-directories and
/// dot-files at any depth), the package's `tests/`, `benches/` and
/// `examples/` (their targets are removed from the mirrored manifest), and
/// nested packages (a directory with its own `Cargo.toml`, mirrored on its
/// own when the game uses it).
pub fn walk_package(dir: &Path) -> Result<Vec<Found>, String> {
    let mut found = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((current, rel)) = stack.pop() {
        let entries = std::fs::read_dir(&current).map_err(|e| format!("{}: {e}", current.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: {e}", current.display()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let child = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            // Follows links: a linked file is mirrored as its content.
            let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if meta.is_dir() {
                let skipped = name == "target"
                    || (rel.is_empty() && matches!(name.as_str(), "tests" | "benches" | "examples"))
                    || path.join("Cargo.toml").is_file();
                if !skipped {
                    stack.push((path, child));
                }
            } else if meta.is_file() {
                found.push(Found { rel: child, path });
            }
        }
    }
    found.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(found)
}

/// Every file under `dir`, relative and forward-slashed (for removing what
/// the mirror no longer has after an interrupted run).
pub fn all_files(dir: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((current, rel)) = stack.pop() {
        let entries = std::fs::read_dir(&current).map_err(|e| format!("{}: {e}", current.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: {e}", current.display()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let kind = entry
                .file_type()
                .map_err(|e| format!("{}: {e}", entry.path().display()))?;
            if kind.is_dir() {
                stack.push((entry.path(), child));
            } else {
                out.push(child);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The running executable's directory and its ancestors.
pub fn executable_ancestors() -> Vec<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .map(|dir| dir.ancestors().map(Path::to_path_buf).collect())
        .unwrap_or_default()
}
