//! The raw-thread ratchet.
//!
//! Engine, editor and plugin code does not create threads, tokio runtimes or
//! rayon pools by hand: it uses `editor_task_queue` (`spawn_thread`,
//! `spawn_service`, `submit`/`submit_async`, and the engine pools in
//! `pools`), so every thread is listed in the Tasks window with its role, its
//! state and, for services, a log and a heartbeat.
//!
//! This test finds the places that still do it by hand. Each file that does is
//! listed, with its reason, in `architecture/raw-threads-allowed.txt`. The list
//! may only shrink: a file that starts creating threads fails here until it
//! uses the task system, and a listed file that no longer does must be removed
//! from the list. See `.agents/EDITOR_TASKS.md`.

// Reads the source tree itself, outside any project, like engine_fs's own
// allow-list test.
#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Calls that create a thread, runtime or pool outside the task system.
const PATTERNS: &[&str] = &[
    "thread::spawn(",
    "thread::Builder::new(",
    "Runtime::new(",
    "Builder::new_multi_thread(",
    "Builder::new_current_thread(",
    "ThreadPoolBuilder::new(",
];

/// Directories scanned. Vendored UI, Helio and third-party crates are
/// foundation code that does not know the editor's task system.
const ROOTS: &[&str] = &[
    "crates/core",
    "crates/editor",
    "crates/subsystems",
    "crates/profiling",
    "plugins/vendor",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("workspace root")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if matches!(
                name.as_str(),
                "target" | "tests" | "examples" | "benches" | ".git" | "node_modules"
            ) {
                continue;
            }
            walk(&path, out);
        } else if name.ends_with(".rs") && name != "build.rs" {
            out.push(path);
        }
    }
}

/// `text` without its `#[cfg(test)]` items: each attribute and the item it
/// gates (a `mod tests;` line, or everything to the brace closing the block
/// it opens). Test code may spawn what it likes; code after a test module is
/// still checked.
fn without_test_items(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut skipping = false;
    let mut depth = 0i32;
    let mut opened = false;
    for line in text.lines() {
        let mut line = line;
        if !skipping {
            let trimmed = line.trim_start();
            let Some(rest) = trimmed.strip_prefix("#[cfg(test)]") else {
                kept.push_str(line);
                kept.push('\n');
                continue;
            };
            skipping = true;
            depth = 0;
            opened = false;
            line = rest;
        }
        let code = line.split("//").next().unwrap_or("");
        for ch in code.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if (opened && depth <= 0) || (!opened && code.trim_end().ends_with(';')) {
            skipping = false;
        }
    }
    kept
}

/// Whether `line` calls `pattern` as written, not as the tail of a longer
/// name (`ScriptRuntime::new(` is not `Runtime::new(`).
fn calls(line: &str, pattern: &str) -> bool {
    line.match_indices(pattern).any(|(at, _)| {
        !line[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

/// Files with a raw call in non-test code, mapped to what they call.
fn offenders(root: &Path) -> BTreeMap<String, Vec<&'static str>> {
    let mut files = Vec::new();
    for dir in ROOTS {
        walk(&root.join(dir), &mut files);
    }
    let mut found = BTreeMap::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let code = without_test_items(&text);
        let mut hits: Vec<&'static str> = Vec::new();
        for line in code.lines() {
            let line = line.trim_start();
            if line.starts_with("//") {
                continue;
            }
            for pattern in PATTERNS {
                if calls(line, pattern) && !hits.contains(pattern) {
                    hits.push(pattern);
                }
            }
        }
        if !hits.is_empty() {
            let relative = file
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            found.insert(relative, hits);
        }
    }
    found
}

fn allowed(root: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(root.join("architecture/raw-threads-allowed.txt"))
        .expect("architecture/raw-threads-allowed.txt");
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (path, reason) = line
                .split_once('|')
                .unwrap_or_else(|| panic!("`path | reason` expected: {line}"));
            (path.trim().to_owned(), reason.trim().to_owned())
        })
        .collect()
}

#[test]
fn raw_thread_creation_only_shrinks() {
    let root = workspace_root();
    let found = offenders(&root);
    let allowed = allowed(&root);

    let new: Vec<String> = found
        .iter()
        .filter(|(path, _)| !allowed.contains_key(*path))
        .map(|(path, hits)| format!("{path}: {}", hits.join(", ")))
        .collect();
    let stale: Vec<&String> = allowed
        .keys()
        .filter(|path| !found.contains_key(*path))
        .collect();

    assert!(
        new.is_empty(),
        "these files create threads, runtimes or pools by hand. Use \
         editor_task_queue (spawn_thread, spawn_service, submit_async, or the \
         engine pools) instead; do not add them to the allow list:\n{new:#?}"
    );
    assert!(
        stale.is_empty(),
        "these files no longer create threads by hand; remove them from \
         architecture/raw-threads-allowed.txt (it only shrinks):\n{stale:#?}"
    );
}

#[test]
fn test_items_are_skipped_and_code_after_them_is_not() {
    let text = "fn a() {}
#[cfg(test)]
mod tests {
    fn t() { std::thread::spawn(|| {}); }
}
fn b() { std::thread::spawn(|| {}); }
";
    let code = without_test_items(text);
    assert!(!code.contains("fn t()"));
    assert!(code.contains("fn b()"));
    assert!(calls("std::thread::spawn(f)", "thread::spawn("));
    assert!(!calls("ScriptRuntime::new(dir)", "Runtime::new("));
    assert!(calls("tokio::runtime::Runtime::new()", "Runtime::new("));
}
