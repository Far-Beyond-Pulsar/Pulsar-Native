//! Loading task definitions, status types, and inter-thread event types.
//!
//! Every task is a plain `fn` that does real work and returns how long it
//! took plus an optional human-readable detail string.  The loading-screen
//! background thread runs them sequentially, measures each one, and sends a
//! `LoadingEvent::TaskDone` so the UI advances its progress indicator in
//! real-time.  There are no artificial `sleep` calls — the loading screen
//! completes as fast as the actual work does.

use std::path::Path;
use std::time::{Duration, Instant};

// ── Task result ────────────────────────────────────────────────────────────

pub(crate) struct TaskResult {
    pub elapsed: Duration,
    /// Short detail shown in the loading screen's status line and logged to
    /// the terminal, e.g. `"847 files indexed"`.
    pub detail: Option<String>,
}

// ── Task function type ─────────────────────────────────────────────────────

pub(crate) type TaskFn = fn(&Path) -> TaskResult;

// ── Task list ──────────────────────────────────────────────────────────────

/// Sequential loading tasks.  Each entry is `(display_label, task_fn)`.
/// The background thread executes them in order and reports real elapsed times.
pub(crate) const TASKS: &[(&str, TaskFn)] = &[
    ("Pre-warming compiler cache", task_cargo_check),
    ("Verifying project structure", task_verify_project),
    ("Reading project configuration", task_read_config),
    ("Scanning workspace packages", task_scan_packages),
    ("Indexing source files", task_index_files),
    ("Scanning asset pipeline", task_scan_assets),
    ("Building file tree", task_scan_folder_tree),
    ("Warming scene cache", task_warm_scene),
    ("Loading engine settings", task_load_settings),
    ("Checking language server", task_check_lsp),
    ("Finalizing workspace", task_finalize),
];

// ── Status / event types ───────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum TaskStatus {
    Pending,
    Running,
    /// Task finished; stores the real wall-clock duration for display.
    Done(Duration),
}

#[derive(Debug)]
pub(crate) enum LoadingEvent {
    /// Emitted when a task finishes.
    TaskDone {
        idx: usize,
        elapsed: Duration,
        detail: Option<String>,
    },
}

// ── Task implementations ───────────────────────────────────────────────────

fn task_cargo_check(project: &Path) -> TaskResult {
    let t = Instant::now();
    let output = std::process::Command::new("cargo")
        .args(["check", "--quiet"])
        .current_dir(project)
        .output();
    match output {
        Ok(out) => {
            let status = if out.status.success() { "ok" } else { "errors" };
            let detail = format!(
                "{status} ({} stderr)",
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .count()
                    .saturating_sub(1)
            );
            TaskResult { elapsed: t.elapsed(), detail: Some(detail) }
        }
        Err(e) => TaskResult { elapsed: t.elapsed(), detail: Some(format!("cargo not found: {e}")) },
    }
}

fn task_verify_project(project: &Path) -> TaskResult {
    let t = Instant::now();
    let exists = project.exists();
    let has_cargo = project.join("Cargo.toml").exists();
    let has_src = project.join("src").exists();
    let detail = format!(
        "{}{}{}",
        if exists { "found" } else { "missing" },
        if has_cargo { " • Cargo.toml ✓" } else { " • Cargo.toml missing" },
        if has_src { " • src/ ✓" } else { "" },
    );
    TaskResult { elapsed: t.elapsed(), detail: Some(detail) }
}

fn task_read_config(project: &Path) -> TaskResult {
    let t = Instant::now();
    let path = project.join("Cargo.toml");
    let detail = std::fs::read_to_string(&path)
        .map(|s| format!("{} bytes", s.len()))
        .unwrap_or_else(|_| "not found".to_string());
    TaskResult { elapsed: t.elapsed(), detail: Some(detail) }
}

fn task_scan_packages(project: &Path) -> TaskResult {
    let t = Instant::now();
    let content = std::fs::read_to_string(project.join("Cargo.toml"))
        .unwrap_or_default();
    // Count quoted workspace member entries inside the [workspace] section.
    let mut in_workspace = false;
    let mut count = 0usize;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("[workspace") {
            in_workspace = true;
            continue;
        }
        if in_workspace && trimmed.starts_with('[') {
            in_workspace = false;
        }
        if in_workspace && trimmed.starts_with('"') {
            count += 1;
        }
    }
    TaskResult {
        elapsed: t.elapsed(),
        detail: Some(format!("{count} crates")),
    }
}

fn task_index_files(project: &Path) -> TaskResult {
    use crate::preload::{store_preloaded_files, PreloadedFileEntry};
    use ui_common::file_utils::find_openable_files;

    let t = Instant::now();
    let files = find_openable_files(project, Some(1000));
    let count = files.len();
    store_preloaded_files(
        files
            .into_iter()
            .map(|fi| PreloadedFileEntry { path: fi.path, name: fi.name })
            .collect(),
    );
    TaskResult {
        elapsed: t.elapsed(),
        detail: Some(format!("{count} files")),
    }
}

fn task_scan_assets(project: &Path) -> TaskResult {
    let t = Instant::now();
    let assets_dir = project.join("assets");
    let count = count_files_recursive(&assets_dir, 4);
    let detail = if assets_dir.is_dir() {
        format!("{count} assets")
    } else {
        "no assets/ dir".to_string()
    };
    TaskResult { elapsed: t.elapsed(), detail: Some(detail) }
}

fn task_warm_scene(project: &Path) -> TaskResult {
    let t = Instant::now();
    let scene_dir = project.join("scene");
    let _ = std::fs::create_dir_all(&scene_dir);
    // Read the file into memory — the OS page cache remains warm so
    // ensure_default_level_file's subsequent deserialization skips disk.
    let size = std::fs::read(scene_dir.join("default.level"))
        .map(|b| b.len())
        .unwrap_or(0);
    let detail = if size > 0 {
        format!("{} KB cached", (size + 511) / 1024)
    } else {
        "scene dir ready".to_string()
    };
    TaskResult { elapsed: t.elapsed(), detail: Some(detail) }
}

fn task_load_settings(_project: &Path) -> TaskResult {
    let t = Instant::now();
    // Best-effort: check whether a settings file exists at the standard path.
    let detail = directories::ProjectDirs::from("dev", "Pulsar", "Pulsar Engine")
        .map(|dirs| dirs.config_dir().join("settings.json"))
        .and_then(|p| std::fs::metadata(&p).ok().map(|m| format!("{} bytes", m.len())))
        .unwrap_or_else(|| "using defaults".to_string());
    TaskResult { elapsed: t.elapsed(), detail: Some(detail) }
}

fn task_check_lsp(_project: &Path) -> TaskResult {
    let t = Instant::now();
    let sep = if cfg!(windows) { ';' } else { ':' };
    let found = std::env::var("PATH").unwrap_or_default().split(sep).any(|dir| {
        let base = Path::new(dir);
        base.join("rust-analyzer").exists() || base.join("rust-analyzer.exe").exists()
    });
    TaskResult {
        elapsed: t.elapsed(),
        detail: Some(if found { "found".to_string() } else { "not in PATH".to_string() }),
    }
}

fn task_scan_folder_tree(project: &Path) -> TaskResult {
    use ui_file_manager::{store_preloaded_tree, FolderNode};
    let t = Instant::now();
    let tree = FolderNode::from_path(project);
    let count = tree.as_ref().map(count_tree_nodes).unwrap_or(0);
    store_preloaded_tree(tree);
    TaskResult {
        elapsed: t.elapsed(),
        detail: Some(format!("{count} folders")),
    }
}

fn task_finalize(_project: &Path) -> TaskResult {
    // Logical fence: all earlier tasks have completed, pre-loaded data is ready.
    TaskResult { elapsed: Duration::ZERO, detail: Some("ready".to_string()) }
}

// ── Internal helpers ───────────────────────────────────────────────────────

fn count_tree_nodes(node: &ui_file_manager::FolderNode) -> usize {
    1 + node.children.iter().map(count_tree_nodes).sum::<usize>()
}

fn count_files_recursive(dir: &Path, max_depth: usize) -> usize {
    if max_depth == 0 || !dir.is_dir() {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            n += 1;
        } else if path.is_dir() {
            n += count_files_recursive(&path, max_depth - 1);
        }
    }
    n
}
