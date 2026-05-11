//! "Build Core" toolbar button.
//!
//! Triggers `cargo build` in the open project root.  This module has zero
//! knowledge of blueprints or any other subsystem — it just invokes the
//! standard Rust build toolchain.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use ui::button::{Button, ButtonVariants as _};
use ui::notification::Notification;
use ui::{ContextModal as _, Disableable as _, IconName};

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
                                Ok(_) => window.push_notification(
                                    Notification::success("Build Core")
                                        .message("Build succeeded."),
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
    let status = std::process::Command::new("cargo")
        .arg("build")
        .current_dir(project_root)
        .status()
        .map_err(|e| format!("Failed to spawn cargo: {e}"))?;

    if status.success() {
        Ok(project_root.clone())
    } else {
        Err("cargo build failed".into())
    }
}
