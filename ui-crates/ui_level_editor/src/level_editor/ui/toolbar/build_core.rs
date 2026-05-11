//! "Build Core" toolbar button.
//!
//! Calls `blueprint_compiler::compile_project` — all blueprint parsing and
//! type conversion is the compiler's responsibility, not ours.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use ui::button::{Button, ButtonVariants as _};
use ui::notification::Notification;
use ui::{ContextModal as _, Disableable as _, IconName};

use blueprint_compiler::project::{ProjectSpec, generate_project};

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

                let (tx, rx) = smol::channel::bounded::<Result<PathBuf, String>>(1);

                std::thread::spawn(move || {
                    smol::block_on(tx.send(run_build(&project_root))).ok();
                });

                let window_handle = window.window_handle();
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

fn run_build(project_root: &PathBuf) -> Result<PathBuf, String> {
    let crate_name = project_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("pulsar_project")
        .replace([' ', '-'], "_")
        .to_ascii_lowercase();

    let compiled = blueprint_compiler::compile_project(project_root)?;

    let mut spec = ProjectSpec::new(&crate_name)
        .description(format!("Generated Pulsar game project: {crate_name}"));
    for bp in compiled {
        spec = spec.add_blueprint(bp);
    }

    generate_project(&spec)
        .write_to_dir(project_root)
        .map_err(|e| format!("Failed to write project: {e}"))?;

    Ok(project_root.clone())
}
