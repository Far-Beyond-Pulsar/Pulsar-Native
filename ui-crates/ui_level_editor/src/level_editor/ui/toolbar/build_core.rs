//! "Build Core" toolbar button.
//!
//! Pressing the button:
//! 1. Scans `<project>/blueprints/` for `*.json` blueprint files.
//! 2. Deserialises each one as a [`GraphDescription`] and compiles it with
//!    [`blueprint_compiler::compile_blueprint`].
//! 3. Feeds every compiled blueprint into [`blueprint_compiler::project::generate_project`].
//! 4. Writes the resulting game crate to `<project>/build/<project_name>/`.
//!
//! All heavy work runs on a background thread so the UI stays responsive.
//! Progress and errors are written to the `tracing` log.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use ui::button::{Button, ButtonVariants as _};
use ui::{Disableable as _, IconName};

use blueprint_compiler::{
    compile_blueprint,
    project::{CompiledBlueprint, ProjectSpec, generate_project},
    GraphDescription,
};

use super::super::state::LevelEditorState;

/// Toolbar component that renders the "Build Core" button.
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
            .icon(IconName::Play)
            .label("Build Core")
            .tooltip("Compile all blueprints and generate a runnable Pulsar game crate")
            .when(is_playing, |b: Button| b.disabled(true))
            .on_click(move |_, _, _| {
                let Some(project_root) = engine_state::get_project_path().map(PathBuf::from) else {
                    tracing::warn!("[BuildCore] No project open — build skipped");
                    return;
                };

                std::thread::spawn(move || {
                    run_build(&project_root);
                });
            })
    }
}

// ── Build logic ───────────────────────────────────────────────────────────────

/// Derive a crate-friendly name from the last path component (replaces spaces/hyphens with `_`).
fn project_crate_name(root: &PathBuf) -> String {
    root.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("pulsar_project")
        .replace([' ', '-'], "_")
        .to_ascii_lowercase()
}

/// The full build pipeline, intended to run on a background thread.
fn run_build(project_root: &PathBuf) {
    let crate_name = project_crate_name(project_root);
    let bp_dir = project_root.join("blueprints");
    let out_dir = project_root.join("build").join(&crate_name);

    tracing::info!(
        "[BuildCore] Starting project build → {}",
        out_dir.display()
    );

    // ── Collect blueprint JSON files ──────────────────────────────────────────
    let blueprint_paths: Vec<PathBuf> = match bp_dir.exists() {
        false => {
            tracing::info!(
                "[BuildCore] No blueprints/ directory found at {} — generating empty project",
                bp_dir.display()
            );
            vec![]
        }
        true => {
            let entries = match std::fs::read_dir(&bp_dir) {
                Ok(e) => e,
                Err(err) => {
                    tracing::error!("[BuildCore] Cannot read blueprints dir: {err}");
                    return;
                }
            };
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
                .collect()
        }
    };

    tracing::info!(
        "[BuildCore] Found {} blueprint file(s) to compile",
        blueprint_paths.len()
    );

    // ── Compile each blueprint ────────────────────────────────────────────────
    let mut compiled: Vec<CompiledBlueprint> = Vec::with_capacity(blueprint_paths.len());

    for path in &blueprint_paths {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_owned();

        let raw = match std::fs::read_to_string(path) {
            Ok(r) => r,
            Err(err) => {
                tracing::error!("[BuildCore] Cannot read {}: {err}", path.display());
                continue;
            }
        };

        let graph: GraphDescription = match serde_json::from_str(&raw) {
            Ok(g) => g,
            Err(err) => {
                tracing::error!(
                    "[BuildCore] Cannot parse blueprint {name}: {err}"
                );
                continue;
            }
        };

        match compile_blueprint(&graph) {
            Ok(source) => {
                tracing::info!("[BuildCore] Compiled blueprint: {name}");
                compiled.push(CompiledBlueprint::new(name, source));
            }
            Err(err) => {
                tracing::error!("[BuildCore] Compile error in {name}: {err}");
            }
        }
    }

    // ── Generate the project ──────────────────────────────────────────────────
    let mut spec = ProjectSpec::new(&crate_name)
        .description(format!("Generated Pulsar game project: {crate_name}"));

    for bp in compiled {
        spec = spec.add_blueprint(bp);
    }

    let project = generate_project(&spec);

    match project.write_to_dir(&out_dir) {
        Ok(()) => {
            tracing::info!(
                "[BuildCore] Project written to {} ({} files)",
                out_dir.display(),
                project.files.len()
            );
        }
        Err(err) => {
            tracing::error!("[BuildCore] Failed to write project: {err}");
        }
    }
}
