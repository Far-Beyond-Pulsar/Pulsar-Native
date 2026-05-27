//! "Build Core" toolbar button.
//!
//! Triggers a full project build:
//!   1. `ensure_core_bootstrap` regenerates all scaffolding files.
//!   2. `cargo update --aggressive` refreshes git-sourced deps.
//!   3. `cargo build` compiles the project.
//!
//! A progress-bar notification tracks the build state and auto-dismisses
//! after completion.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use ui::button::{Button, ButtonVariants as _};
use ui::notification::Notification;
use ui::{ContextModal as _, Disableable as _, IconName};

use super::super::state::LevelEditorState;

/// Typed ID for the build notification — pushing a new one with this ID
/// replaces the previous in-progress notification.
struct BuildCoreNotification;

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
                let Some(project_root) = engine_state::get_project_path().map(PathBuf::from) else {
                    window.push_notification(
                        Notification::warning("No project open — open a project first."),
                        cx,
                    );
                    return;
                };

                // Show an in-progress notification with an animated shimmer bar.
                // autohide(false) keeps it visible until we replace it on completion.
                window.push_notification(
                    Notification::info("Building project…")
                        .id::<BuildCoreNotification>()
                        .title("Build Core")
                        .progress(0.5)   // indeterminate shimmer
                        .autohide(false),
                    cx,
                );

                let (tx, rx) = smol::channel::bounded::<Result<(), String>>(1);

                std::thread::spawn(move || {
                    smol::block_on(tx.send(run_build(&project_root))).ok();
                });

                let window_handle = window.window_handle();
                cx.spawn(async move |async_app: &mut AsyncApp| {
                    if let Ok(result) = rx.recv().await {
                        let _ = async_app.update_window(window_handle, |_, window, cx| {
                            match result {
                                Ok(()) => {
                                    // Replace the in-progress notification with a completed one.
                                    // progress(1.0) renders a full green bar and auto-dismisses
                                    // after the autohide_delay (3 s).
                                    window.push_notification(
                                        Notification::success("Build succeeded.")
                                            .id::<BuildCoreNotification>()
                                            .title("Build Core")
                                            .progress(1.0)
                                            .autohide_delay(Duration::from_secs(3)),
                                        cx,
                                    );
                                }
                                Err(msg) => {
                                    // Replace with an error notification (no progress bar,
                                    // standard 5 s autohide).
                                    window.push_notification(
                                        Notification::error(msg)
                                            .id::<BuildCoreNotification>()
                                            .title("Build Core"),
                                        cx,
                                    );
                                }
                            }
                        });
                    }
                })
                .detach();
            })
    }
}

fn run_build(project_root: &PathBuf) -> Result<(), String> {
    // Regenerate all bootstrap files (Cargo.toml, main.rs, engine_main.rs,
    // classes/mod.rs) before building. JSON/blueprints are the source of truth.
    engine_backend::services::ensure_core_bootstrap(project_root)?;

    // Update git-sourced deps to their latest commits so the build never
    // uses a stale Cargo.lock pointing at an old Pulsar-Native revision.
    let update_status = std::process::Command::new("cargo")
        .args(["update", "--aggressive"])
        .current_dir(project_root)
        .status()
        .map_err(|e| format!("Failed to spawn cargo update: {e}"))?;

    if !update_status.success() {
        tracing::warn!("cargo update returned non-zero; proceeding with build anyway");
    }

    let status = std::process::Command::new("cargo")
        .arg("build")
        .current_dir(project_root)
        .status()
        .map_err(|e| format!("Failed to spawn cargo: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err("cargo build failed — check the editor log for details".into())
    }
}
