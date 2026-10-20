//! Loading task definitions, status types, and inter-thread event types.
//!
//! Every task is a plain `fn` that does real work and returns how long it
//! took plus an optional human-readable detail string.  The loading-screen
//! background thread runs them sequentially, measures each one, and sends a
//! `LoadingEvent::TaskDone` so the UI advances its progress indicator in
//! real-time.  There are no artificial `sleep` calls — the loading screen
//! completes as fast as the actual work does.

use std::path::{Path, PathBuf};
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
    ("Preparing editor plugins", task_prepare_plugins),
    ("Verifying project structure", task_verify_project),
    ("Indexing project files", task_index_project_files),
    ("Reading project configuration", task_read_config),
    ("Scanning workspace packages", task_scan_packages),
    ("Indexing source files", task_index_files),
    ("Scanning asset pipeline", task_scan_assets),
    ("Building file tree", task_scan_folder_tree),
    ("Warming scene cache", task_warm_scene),
    ("Preloading open tabs", task_warm_open_tabs),
    ("Preloading plugin assets", task_preload_plugin_assets),
    ("Loading engine settings", task_load_settings),
    ("Checking language server", task_check_lsp),
    ("Synchronizing Git hooks", task_sync_git_hooks),
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

fn task_verify_project(project: &Path) -> TaskResult {
    let t = Instant::now();
    let exists = project.exists();
    let has_cargo = project.join("Cargo.toml").exists();
    let has_src = project.join("src").exists();
    let detail = format!(
        "{}{}{}",
        if exists { "found" } else { "missing" },
        if has_cargo {
            " • Cargo.toml ✓"
        } else {
            " • Cargo.toml missing"
        },
        if has_src { " • src/ ✓" } else { "" },
    );
    TaskResult {
        elapsed: t.elapsed(),
        detail: Some(detail),
    }
}

fn task_index_project_files(project: &Path) -> TaskResult {
    let t = Instant::now();
    let detail = match engine_fs::open_project_file_index(project) {
        Ok(Some(index)) => match index.query(&engine_fs::FileIndexQuery::default()) {
            Ok(files) => format!("{} files", files.len()),
            Err(error) => {
                tracing::warn!(%error, project = %project.display(), "Failed to query project file index during startup");
                "index opened; query failed".to_owned()
            }
        },
        Ok(None) => "remote provider; using provider listings".to_owned(),
        Err(error) => {
            tracing::warn!(%error, project = %project.display(), "Failed to initialize project file index during startup");
            "index unavailable; using filesystem listings".to_owned()
        }
    };
    TaskResult {
        elapsed: t.elapsed(),
        detail: Some(detail),
    }
}

fn task_read_config(project: &Path) -> TaskResult {
    let t = Instant::now();
    let path = project.join("Cargo.toml");
    let detail = std::fs::read_to_string(&path)
        .map(|s| format!("{} bytes", s.len()))
        .unwrap_or_else(|_| "not found".to_string());
    TaskResult {
        elapsed: t.elapsed(),
        detail: Some(detail),
    }
}

fn task_scan_packages(project: &Path) -> TaskResult {
    let t = Instant::now();
    let content = std::fs::read_to_string(project.join("Cargo.toml")).unwrap_or_default();
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
            .map(|fi| PreloadedFileEntry {
                path: fi.path,
                name: fi.name,
            })
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
    TaskResult {
        elapsed: t.elapsed(),
        detail: Some(detail),
    }
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
    TaskResult {
        elapsed: t.elapsed(),
        detail: Some(detail),
    }
}

fn task_load_settings(_project: &Path) -> TaskResult {
    let t = Instant::now();
    // Best-effort: check whether a settings file exists at the standard path.
    let detail = directories::ProjectDirs::from("dev", "Pulsar", "Pulsar Engine")
        .map(|dirs| dirs.config_dir().join("settings.json"))
        .and_then(|p| {
            std::fs::metadata(&p)
                .ok()
                .map(|m| format!("{} bytes", m.len()))
        })
        .unwrap_or_else(|| "using defaults".to_string());
    TaskResult {
        elapsed: t.elapsed(),
        detail: Some(detail),
    }
}

fn task_check_lsp(_project: &Path) -> TaskResult {
    let t = Instant::now();
    let sep = if cfg!(windows) { ';' } else { ':' };
    let found = std::env::var("PATH")
        .unwrap_or_default()
        .split(sep)
        .any(|dir| {
            let base = Path::new(dir);
            base.join("rust-analyzer").exists() || base.join("rust-analyzer.exe").exists()
        });
    TaskResult {
        elapsed: t.elapsed(),
        detail: Some(if found {
            "found".to_string()
        } else {
            "not in PATH".to_string()
        }),
    }
}

fn task_sync_git_hooks(project: &Path) -> TaskResult {
    let t = Instant::now();
    let detail = match ui_git_manager::sync_configured_project_hooks(project) {
        Ok(report) => {
            let ui_git_manager::GitHookSyncReport {
                status,
                created,
                preserved,
            } = report;
            match status {
                ui_git_manager::GitHookSyncStatus::Synced => {
                    if created.is_empty() && preserved.is_empty() {
                        "no hooks configured".to_string()
                    } else {
                        format!("{} installed, {} preserved", created.len(), preserved.len())
                    }
                }
                ui_git_manager::GitHookSyncStatus::Skipped(reason) => match reason {
                    ui_git_manager::GitHookSyncSkipReason::RemoteProvider { label } => {
                        format!("skipped: {label} provider")
                    }
                    ui_git_manager::GitHookSyncSkipReason::MissingGitDirectory => {
                        "skipped: no Git repository".to_string()
                    }
                    ui_git_manager::GitHookSyncSkipReason::CustomHooksPath { path } => {
                        format!("skipped: custom hooks path {}", path.display())
                    }
                },
            }
        }
        Err(error) => {
            tracing::warn!(
                project = %project.display(),
                error = %error,
                "Failed to synchronize configured Git hooks; continuing startup"
            );
            "skipped: hook sync failed".to_string()
        }
    };

    TaskResult {
        elapsed: t.elapsed(),
        detail: Some(detail),
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
    TaskResult {
        elapsed: Duration::ZERO,
        detail: Some("ready".to_string()),
    }
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

// ── Editor startup work ────────────────────────────────────────────────────

/// Load and verify every editor plugin library (OS loader, SHA-256, integrity
/// manifest) on worker threads, so the editor window only has to install them.
fn task_prepare_plugins(_project: &Path) -> TaskResult {
    let t = Instant::now();
    let count = plugin_manager::prewarm_plugins("plugins/editor");
    TaskResult {
        elapsed: t.elapsed(),
        detail: Some(format!("{count} plugin librar(ies)")),
    }
}

/// Read the files of the tabs the last session left open, so the editors that
/// restore them in the first frames find them in the OS file cache. An asset
/// that is a folder (a Blueprint class, a material) is read file by file. The
/// paths are also kept for the plugins' own preloads.
fn task_warm_open_tabs(project: &Path) -> TaskResult {
    let t = Instant::now();
    let mut files = Vec::new();
    if let Ok(text) = std::fs::read_to_string(project.join(".pulsar").join("layout.json")) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            collect_tab_files(&value, &mut files);
        }
    }
    files.sort();
    files.dedup();
    let absolute: Vec<PathBuf> = files
        .iter()
        .map(|file| {
            let p = Path::new(file);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                project.join(p)
            }
        })
        .collect();
    if let Ok(mut slot) = OPEN_TAB_FILES.lock() {
        *slot = absolute.clone();
    }


    let bytes: usize = std::thread::scope(|scope| {
        let workers: Vec<_> = absolute
            .iter()
            .map(|path| scope.spawn(move || warm_path(path, 0)))
            .collect();
        workers.into_iter().filter_map(|w| w.join().ok()).sum()
    });
    TaskResult {
        elapsed: t.elapsed(),
        detail: Some(format!("{} tabs, {} KB", files.len(), bytes / 1024)),
    }
}

/// Every string stored under a `"file"` key, anywhere in a saved layout.
fn collect_tab_files(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                match (key.as_str(), child) {
                    ("file", serde_json::Value::String(file)) => out.push(file.clone()),
                    _ => collect_tab_files(child, out),
                }
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| collect_tab_files(v, out)),
        _ => {}
    }
}

/// Read a file, or a folder's files (a few levels deep), returning the bytes.
fn warm_path(path: &Path, depth: usize) -> usize {
    if path.is_dir() {
        if depth >= 3 {
            return 0;
        }
        return std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| warm_path(&entry.path(), depth + 1))
            .sum();
    }
    std::fs::read(path).map(|b| b.len()).unwrap_or(0)
}

/// The files the last session's open tabs refer to, absolute. Filled by
/// [`task_warm_open_tabs`], read by [`task_preload_plugin_assets`].
static OPEN_TAB_FILES: std::sync::Mutex<Vec<std::path::PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Let every plugin preload what its editors will need for the open tabs
/// (`EditorPlugin::preload`). Waits for the UI thread to install the plugins.
fn task_preload_plugin_assets(project: &Path) -> TaskResult {
    let t = Instant::now();
    let files = OPEN_TAB_FILES.lock().map(|f| f.clone()).unwrap_or_default();
    let ctx = plugin_editor_api::PreloadContext {
        project_root: project,
        open_files: &files,
    };
    let results = plugin_manager::run_plugin_preloads(&ctx, Duration::from_secs(30));
    let notes: Vec<String> = results
        .into_iter()
        .filter_map(|(id, note)| note.map(|n| format!("{id}: {n}")))
        .collect();
    TaskResult {
        elapsed: t.elapsed(),
        detail: Some(if notes.is_empty() {
            "nothing requested".to_string()
        } else {
            notes.join("; ")
        }),
    }
}
