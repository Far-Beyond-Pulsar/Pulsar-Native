//! "Build Core" toolbar button.
//!
//! On click:
//! 1. Shows an immediate "Building…" toast.
//! 2. Spawns a `std::thread` that walks the project tree for blueprint folders
//!    (any directory containing `graph_save.json`, the same convention used by
//!    the file manager), compiles each graph with `blueprint_compiler`, calls
//!    `generate_project`, and writes the output directly into the project root
//!    (which is already the Rust crate).
//! 3. Sends the `Result<PathBuf, String>` back via a `smol::channel`.
//! 4. `App::spawn` awaits the channel on the main thread and pushes a success
//!    or error toast — the standard pattern used across this codebase.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use ui::button::{Button, ButtonVariants as _};
use ui::notification::Notification;
use ui::{ContextModal as _, Disableable as _, IconName};

use blueprint_compiler::{
    compile_blueprint,
    project::{CompiledBlueprint, ProjectSpec, generate_project},
    GraphDescription,
};

use super::super::state::LevelEditorState;

pub struct BuildCoreButton;

impl BuildCoreButton {
    pub fn render<V>(
        state: &LevelEditorState,
        _state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        _cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let is_playing = state.editor_mode == super::super::state::EditorMode::Play;

        Button::new("build_core")
            .icon(IconName::Hammer)
            .label("Build Core")
            .tooltip("Compile all blueprints and generate a runnable Pulsar game crate")
            .when(is_playing, |b: Button| b.disabled(true))
            .on_click(move |_, window, cx| {
                let Some(project_root) = engine_state::get_project_path().map(PathBuf::from)
                else {
                    window.push_notification(
                        Notification::warning("Build Core")
                            .message("No project is open — open a project first."),
                        cx,
                    );
                    return;
                };

                window.push_notification(
                    Notification::info("Build Core").message("Building project…"),
                    cx,
                );

                // Channel: background thread → main thread.
                let (tx, rx) = smol::channel::bounded::<Result<PathBuf, String>>(1);
                let root = project_root.clone();

                // Heavy work on a plain OS thread (blocking I/O + compilation).
                std::thread::spawn(move || {
                    smol::block_on(tx.send(run_build(&root))).ok();
                });

                let window_handle = window.window_handle();

                // App::spawn runs on the main thread and can access any window.
                cx.spawn(async move |async_app: &mut AsyncApp| {
                    if let Ok(result) = rx.recv().await {
                        let _ = async_app.update_window(window_handle, |_, window, cx| {
                            match result {
                                Ok(out_path) => window.push_notification(
                                    Notification::success("Build Core").message(format!(
                                        "Project written to {}",
                                        out_path.display()
                                    )),
                                    cx,
                                ),
                                Err(msg) => window.push_notification(
                                    Notification::error("Build Core").message(msg),
                                    cx,
                                ),
                            }
                        });
                    }
                })
                .detach();
            })
    }
}

// ── Build logic ───────────────────────────────────────────────────────────────

fn project_crate_name(root: &PathBuf) -> String {
    root.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("pulsar_project")
        .replace([' ', '-'], "_")
        .to_ascii_lowercase()
}

/// Full build pipeline — runs on a background OS thread.
fn run_build(project_root: &PathBuf) -> Result<PathBuf, String> {
    let crate_name = project_crate_name(project_root);
    // The project root IS the crate — write directly into it.
    let out_dir = project_root.clone();

    tracing::info!("[BuildCore] Starting build → {}", out_dir.display());

    // Blueprints are stored as folders containing graph_save.json — walk the whole
    // project tree to find them (same convention used by the file manager).
    let blueprint_paths: Vec<PathBuf> = find_blueprint_folders(project_root);

    tracing::info!("[BuildCore] {} blueprint(s) found", blueprint_paths.len());

    let mut compiled: Vec<CompiledBlueprint> = Vec::with_capacity(blueprint_paths.len());

    for path in &blueprint_paths {
        // The folder name is the blueprint/class name.
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_owned();

        let graph_file = path.join("graph_save.json");
        let raw = std::fs::read_to_string(&graph_file)
            .map_err(|e| format!("Cannot read {}: {e}", graph_file.display()))?;

        // graph_save.json is prefixed with `//` comment lines by the blueprint
        // editor before the actual JSON payload — strip them before parsing.
        let json = raw.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        // The file is saved as a BlueprintAsset wrapper (format_version,
        // main_graph, local_macros, variables, editor_state, blueprint_metadata).
        // We extract main_graph as a raw JSON value, then redeserialize it as
        // the graphy::GraphDescription that compile_blueprint expects — both
        // types share the same wire format (nodes/connections/metadata).
        let asset: serde_json::Value = serde_json::from_str(&json)
            .map_err(|e| format!("Cannot parse blueprint file {name}: {e}"))?;
        let main_graph = asset.get("main_graph").unwrap_or(&asset);
        let graph: GraphDescription = serde_json::from_value(main_graph.clone())
            .map_err(|e| format!("Cannot parse blueprint {name}: {e}"))?;

        match compile_blueprint(&graph) {
            Ok(source) => {
                tracing::info!("[BuildCore] Compiled: {name}");
                compiled.push(CompiledBlueprint::new(name, source));
            }
            Err(err) => {
                // Non-fatal — log and continue.
                tracing::error!("[BuildCore] Compile error in {name}: {err}");
            }
        }
    }

    let mut spec = ProjectSpec::new(&crate_name)
        .description(format!("Generated Pulsar game project: {crate_name}"));
    for bp in compiled {
        spec = spec.add_blueprint(bp);
    }

    generate_project(&spec)
        .write_to_dir(&out_dir)
        .map_err(|e| format!("Failed to write project: {e}"))?;

    tracing::info!("[BuildCore] Done → {}", out_dir.display());
    Ok(out_dir)
}

/// Walk the project tree and return every directory that contains a
/// `graph_save.json` file — the same convention used by the file manager.
fn find_blueprint_folders(root: &PathBuf) -> Vec<PathBuf> {
    let mut results = Vec::new();
    fn walk(dir: &PathBuf, results: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                if path.join("graph_save.json").exists() {
                    results.push(path);
                } else {
                    walk(&path, results);
                }
            }
        }
    }
    walk(root, &mut results);
    results
}
