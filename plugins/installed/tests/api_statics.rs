//! An api crate holds contracts, not state (Pulsar-Native#1608 phase 4,
//! the architecture's invariant 5): every plugin's `api` crate
//! (`plugins/vendor/*/api`) and Helio's `helio-feature-api` have no
//! `static`, `thread_local!` or `inventory` collection or submission outside
//! tests. A plugin library links its own copy of an api crate, so state there
//! would be private to each copy; registrations belong to the plugin's root
//! build. The exceptions are listed, with their reason, in
//! `api_statics_allowed.txt`, which only shrinks: an entry that no longer
//! matches fails too.

use std::path::{Path, PathBuf};

const PATTERNS: &[&str] = &[
    "static ",
    "thread_local!",
    "inventory::collect!",
    "inventory::submit!",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Every api crate's `src` directory.
fn api_sources() -> Vec<PathBuf> {
    let root = root();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root.join("plugins/vendor"))
        .unwrap()
        .map(|entry| entry.unwrap().path().join("api/src"))
        .filter(|dir| dir.is_dir())
        .collect();
    dirs.push(root.join("crates/renderer/helio/crates/helio-feature-api/src"));
    dirs.sort();
    dirs
}

/// `path: line` for every statement that keeps state, outside tests (a
/// file's `#[cfg(test)]` module, at its end, and `//` comments).
fn findings() -> Vec<String> {
    let root = root();
    let mut found = Vec::new();
    for dir in api_sources() {
        let mut files = Vec::new();
        rust_files(&dir, &mut files);
        files.sort();
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            let relative = file
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            for line in text.lines() {
                let code = line.trim();
                if code.starts_with("#[cfg(test)]") {
                    break;
                }
                if code.starts_with("//") {
                    continue;
                }
                let keeps_state = PATTERNS.iter().any(|pattern| {
                    code.starts_with(pattern)
                        || code.starts_with(&format!("pub {pattern}"))
                        || code.starts_with(&format!("pub(crate) {pattern}"))
                        || code.contains(&format!("::{pattern}"))
                });
                if keeps_state {
                    found.push(format!("{relative}: {code}"));
                }
            }
        }
    }
    found
}

fn allowed() -> Vec<String> {
    include_str!("api_statics_allowed.txt")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split(" # ").next().unwrap().trim().to_string())
        .collect()
}

#[test]
fn api_crates_keep_no_state() {
    let found = findings();
    let allowed = allowed();
    let new: Vec<&String> = found.iter().filter(|f| !allowed.contains(f)).collect();
    let stale: Vec<&String> = allowed.iter().filter(|a| !found.contains(a)).collect();
    assert!(
        new.is_empty(),
        "state in an api crate (move it to the plugin's root or a runtime crate):\n{}",
        new.iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        stale.is_empty(),
        "api_statics_allowed.txt lists what is gone; delete these lines:\n{}",
        stale
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}
