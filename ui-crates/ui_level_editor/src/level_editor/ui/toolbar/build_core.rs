//! "Build Core" toolbar button.
//!
//! Build flow:
//!   1. `ensure_core_bootstrap` regenerates all scaffolding files.
//!   2. `cargo update` refreshes git-sourced deps.
//!   3. `cargo build --message-format=json` compiles; `compiler-artifact`
//!      events are counted to drive a real progress bar.
//!
//! A progress-bar notification tracks the build state in real time and
//! auto-dismisses 3 s after completion.

use std::io::{BufRead as _, BufReader};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use ui::button::{Button, ButtonVariants as _};
use ui::notification::Notification;
use ui::{ContextModal as _, Disableable as _, IconName};

use super::super::state::LevelEditorState;

/// Typed marker so pushing a new build notification replaces the old one.
struct BuildCoreNotification;

pub struct BuildCoreButton;

impl BuildCoreButton {
    pub fn render<V>(
        state: &LevelEditorState,
        _state_arc: crate::level_editor::StateEntity,
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

                // Shared progress value: 0..=100 (percentage).
                // Written by the build thread, polled by the UI task.
                let progress_atomic: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));
                let progress_for_thread = Arc::clone(&progress_atomic);
                let progress_for_ui = Arc::clone(&progress_atomic);

                // Channel for the final result (success or error message).
                let (result_tx, result_rx) = smol::channel::bounded::<Result<(), String>>(1);

                // Kick off the build on a plain OS thread so cargo can block.
                std::thread::spawn(move || {
                    let result = run_build(&project_root, progress_for_thread);
                    smol::block_on(result_tx.send(result)).ok();
                });

                // Show the initial "building" notification with 0% progress.
                window.push_notification(
                    Notification::info("Building project…")
                        .id::<BuildCoreNotification>()
                        .title("Build Core")
                        .progress(0.0)
                        .autohide(false),
                    cx,
                );

                let window_handle = window.window_handle();

                cx.spawn(async move |async_app: &mut AsyncApp| {
                    let mut last_pct: u32 = 0;

                    loop {
                        // Non-blocking check for completion.
                        match result_rx.try_recv() {
                            Ok(result) => {
                                // Build finished — push the completion notification
                                // (same ID replaces the in-progress one).
                                let _ = async_app.update_window(window_handle, |_, window, cx| {
                                    match result {
                                        Ok(()) => window.push_notification(
                                            Notification::success("Build succeeded.")
                                                .id::<BuildCoreNotification>()
                                                .title("Build Core")
                                                .progress(1.0)
                                                .autohide_delay(Duration::from_secs(3)),
                                            cx,
                                        ),
                                        Err(msg) => window.push_notification(
                                            Notification::error(msg)
                                                .id::<BuildCoreNotification>()
                                                .title("Build Core"),
                                            cx,
                                        ),
                                    }
                                });
                                return;
                            }
                            Err(smol::channel::TryRecvError::Closed) => return,
                            Err(smol::channel::TryRecvError::Empty) => {
                                // Still building — update the bar if progress moved.
                                let pct = progress_for_ui.load(Ordering::Relaxed);
                                if pct != last_pct {
                                    last_pct = pct;
                                    let _ = async_app.update_window(window_handle, |_, window, cx| {
                                        window.push_notification(
                                            Notification::info(format!(
                                                "Building project… ({pct}%)"
                                            ))
                                            .id::<BuildCoreNotification>()
                                            .title("Build Core")
                                            .progress(pct as f32 / 100.0)
                                            .autohide(false),
                                            cx,
                                        )
                                    });
                                }
                            }
                        }

                        // Poll at ~4 Hz — smooth enough for a build progress bar.
                        async_app
                            .background_executor()
                            .timer(Duration::from_millis(250))
                            .await;
                    }
                })
                .detach();
            })
    }
}

/// Run the full build pipeline, updating `progress` (0–100) via an atomic.
fn run_build(project_root: &PathBuf, progress: Arc<AtomicU32>) -> Result<(), String> {
    // ── Regenerate scaffolding ────────────────────────────────────────────────
    // JSON/blueprints are the source of truth; always regenerate before building.
    engine_backend::services::ensure_core_bootstrap(project_root)?;

    // ── Refresh git-sourced deps ──────────────────────────────────────────────
    let update_ok = std::process::Command::new("cargo")
        .arg("update")
        .current_dir(project_root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !update_ok {
        tracing::warn!("cargo update returned non-zero; proceeding with build anyway");
    }

    // ── Estimate total package count for progress denominator ─────────────────
    let total = estimate_package_count(project_root);

    // ── cargo build --message-format=json ────────────────────────────────────
    // Capture stdout for JSON progress; let stderr through so error text
    // is visible in whatever terminal/log the editor writes to.
    let mut child = std::process::Command::new("cargo")
        .args(["build", "--message-format=json"])
        .current_dir(project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Failed to spawn cargo build: {e}"))?;

    let stdout = BufReader::new(
        child
            .stdout
            .take()
            .ok_or_else(|| "Failed to capture cargo stdout".to_string())?,
    );

    let mut compiled: u32 = 0;

    for line in stdout.lines() {
        let Ok(line) = line else { continue };

        // Fast path: skip lines that clearly aren't compiler-artifact JSON.
        // Full parse only when we see the key we care about.
        if line.contains(r#""compiler-artifact""#) {
            compiled += 1;
            // Cap at 95 % — the last 5 % is reserved for the link step.
            let pct = ((compiled as f32 / total as f32) * 95.0).min(95.0) as u32;
            progress.store(pct, Ordering::Relaxed);
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("Failed to wait on cargo build: {e}"))?;

    if status.success() {
        progress.store(100, Ordering::Relaxed);
        Ok(())
    } else {
        Err("cargo build failed — check the editor output for details".into())
    }
}

/// Ask cargo for the total number of resolved packages; used to scale
/// the progress bar.  Falls back to 100 if metadata is unavailable.
fn estimate_package_count(project_root: &PathBuf) -> f32 {
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--format-version=1", "--no-deps"])
        .current_dir(project_root)
        .output();

    // `--no-deps` only gives workspace members (fast), so multiply by a
    // reasonable factor for transitive deps.  The exact number doesn't
    // matter; it just controls when the bar reaches 95 %.
    let workspace_members = output
        .ok()
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
        .and_then(|v| v["packages"].as_array().map(|a| a.len()))
        .unwrap_or(5) as f32;

    // Heuristic: average project has ~30× more transitive deps than workspace members.
    (workspace_members * 30.0).max(50.0)
}
