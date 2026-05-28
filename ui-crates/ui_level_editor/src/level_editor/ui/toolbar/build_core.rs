//! "Build Core" split-button.
//!
//! Left side  — triggers the currently selected build mode immediately.
//! Right side — chevron opens a popup menu to switch mode or trigger one-off.
//!
//! Modes:
//!   Build        — regenerate + cargo build (default)
//!   Build + Run  — regenerate + cargo build, then launch the game
//!   Check        — cargo check only (fast type-check, no codegen)

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
use ui::button::{Button, ButtonVariants as _, DropdownButton};
use ui::notification::Notification;
use ui::popup_menu::PopupMenuExt as _;
use ui::{ContextModal as _, Disableable as _, IconName, Sizable as _};

use super::super::state::{BuildMode, LevelEditorState};
use super::actions::SetBuildMode;

struct BuildCoreNotification;

pub struct BuildCoreButton;

impl BuildCoreButton {
    pub fn render<V>(
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        _cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let is_playing = state.editor_mode == super::super::state::EditorMode::Play;
        let build_mode = state.build_mode;

        let (label, icon, tooltip) = mode_label_icon_tooltip(build_mode);

        let state_for_click = state_arc.clone();
        let primary = Button::new("build_core_primary")
            .icon(icon)
            .label(label)
            .tooltip(tooltip)
            .when(is_playing, |b| b.disabled(true))
            .on_click(move |_, window, cx| {
                let mode = state_for_click.read().build_mode;
                trigger_build(mode, window, cx);
            });

        let state_for_menu = state_arc.clone();
        DropdownButton::new("build_core_dropdown")
            .button(primary)
            .when(!is_playing, |d| {
                d.popup_menu(move |menu, _, _| {
                    let current = state_for_menu.read().build_mode;
                    menu.label("Build Mode")
                        .separator()
                        .menu_with_check(
                            "Build",
                            current == BuildMode::Build,
                            Box::new(SetBuildMode(BuildMode::Build)),
                        )
                        .menu_with_check(
                            "Build + Run",
                            current == BuildMode::BuildAndRun,
                            Box::new(SetBuildMode(BuildMode::BuildAndRun)),
                        )
                        .menu_with_check(
                            "Check",
                            current == BuildMode::Check,
                            Box::new(SetBuildMode(BuildMode::Check)),
                        )
                })
            })
    }
}

fn mode_label_icon_tooltip(mode: BuildMode) -> (&'static str, IconName, &'static str) {
    match mode {
        BuildMode::Build => (
            "Build",
            IconName::Hammer,
            "Compile all blueprints and generate a runnable Pulsar game crate",
        ),
        BuildMode::BuildAndRun => (
            "Build + Run",
            IconName::Play,
            "Compile and immediately launch the game",
        ),
        BuildMode::Check => (
            "Check",
            IconName::Check,
            "Run cargo check — fast type-check with no codegen",
        ),
    }
}

fn trigger_build(mode: BuildMode, window: &mut Window, cx: &mut App) {
    let Some(project_root) = engine_state::get_project_path().map(PathBuf::from) else {
        window.push_notification(
            Notification::warning("No project open — open a project first."),
            cx,
        );
        return;
    };

    if mode == BuildMode::Check {
        run_check(project_root, window, cx);
    } else {
        run_build_pipeline(project_root, mode, window, cx);
    }
}

fn run_check(project_root: PathBuf, window: &mut Window, cx: &mut App) {
    let (result_tx, result_rx) = smol::channel::bounded::<Result<(), String>>(1);

    std::thread::spawn(move || {
        let result = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&project_root)
            .status()
            .map_err(|e| format!("Failed to spawn cargo check: {e}"))
            .and_then(|s| {
                if s.success() {
                    Ok(())
                } else {
                    Err("cargo check failed — check the editor output for details".into())
                }
            });
        smol::block_on(result_tx.send(result)).ok();
    });

    window.push_notification(
        Notification::info("Checking project…")
            .id::<BuildCoreNotification>()
            .title("Check")
            .autohide(false),
        cx,
    );

    let window_handle = window.window_handle();
    cx.spawn(async move |async_app: &mut AsyncApp| {
        let Ok(result) = result_rx.recv().await else { return };
        let _ = async_app.update_window(window_handle, |_, window, cx| match result {
            Ok(()) => window.push_notification(
                Notification::success("Check passed.")
                    .id::<BuildCoreNotification>()
                    .title("Check")
                    .autohide_delay(Duration::from_secs(3)),
                cx,
            ),
            Err(msg) => window.push_notification(
                Notification::error(msg).id::<BuildCoreNotification>().title("Check"),
                cx,
            ),
        });
    })
    .detach();
}

fn run_build_pipeline(project_root: PathBuf, mode: BuildMode, window: &mut Window, cx: &mut App) {
    let progress_atomic: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));
    let progress_for_thread = Arc::clone(&progress_atomic);
    let progress_for_ui = Arc::clone(&progress_atomic);

    let (result_tx, result_rx) = smol::channel::bounded::<Result<(), String>>(1);

    std::thread::spawn(move || {
        let result = run_build(&project_root, progress_for_thread, mode);
        smol::block_on(result_tx.send(result)).ok();
    });

    let title = if mode == BuildMode::BuildAndRun { "Build + Run" } else { "Build Core" };

    window.push_notification(
        Notification::info("Building project…")
            .id::<BuildCoreNotification>()
            .title(title)
            .progress(0.0)
            .autohide(false),
        cx,
    );

    let window_handle = window.window_handle();
    cx.spawn(async move |async_app: &mut AsyncApp| {
        let mut last_pct: u32 = 0;
        loop {
            match result_rx.try_recv() {
                Ok(result) => {
                    let _ = async_app.update_window(window_handle, |_, window, cx| {
                        match result {
                            Ok(()) => window.push_notification(
                                Notification::success("Build succeeded.")
                                    .id::<BuildCoreNotification>()
                                    .title(title)
                                    .progress(1.0)
                                    .autohide_delay(Duration::from_secs(3)),
                                cx,
                            ),
                            Err(msg) => window.push_notification(
                                Notification::error(msg)
                                    .id::<BuildCoreNotification>()
                                    .title(title),
                                cx,
                            ),
                        }
                    });
                    return;
                }
                Err(smol::channel::TryRecvError::Closed) => return,
                Err(smol::channel::TryRecvError::Empty) => {
                    let pct = progress_for_ui.load(Ordering::Relaxed);
                    if pct != last_pct {
                        last_pct = pct;
                        let _ = async_app.update_window(window_handle, |_, window, cx| {
                            window.push_notification(
                                Notification::info(format!("Building project… ({pct}%)"))
                                    .id::<BuildCoreNotification>()
                                    .title(title)
                                    .progress(pct as f32 / 100.0)
                                    .autohide(false),
                                cx,
                            )
                        });
                    }
                }
            }

            async_app
                .background_executor()
                .timer(Duration::from_millis(250))
                .await;
        }
    })
    .detach();
}

fn run_build(
    project_root: &PathBuf,
    progress: Arc<AtomicU32>,
    mode: BuildMode,
) -> Result<(), String> {
    engine_backend::services::ensure_core_bootstrap(project_root)?;

    let update_ok = std::process::Command::new("cargo")
        .arg("update")
        .current_dir(project_root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !update_ok {
        tracing::warn!("cargo update returned non-zero; proceeding with build anyway");
    }

    let total = estimate_package_count(project_root);

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
        if line.contains(r#""compiler-artifact""#) {
            compiled += 1;
            let pct = ((compiled as f32 / total as f32) * 95.0).min(95.0) as u32;
            progress.store(pct, Ordering::Relaxed);
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("Failed to wait on cargo build: {e}"))?;

    if !status.success() {
        return Err("cargo build failed — check the editor output for details".into());
    }

    progress.store(100, Ordering::Relaxed);

    if mode == BuildMode::BuildAndRun {
        launch_game(project_root)?;
    }

    Ok(())
}

fn launch_game(project_root: &PathBuf) -> Result<(), String> {
    // Find the built binary: target/debug/<project_name>
    let project_name = project_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("game")
        .to_string();

    let binary = project_root
        .join("target")
        .join("debug")
        .join(&project_name);

    std::process::Command::new(&binary)
        .current_dir(project_root)
        .spawn()
        .map_err(|e| format!("Failed to launch game binary '{}': {e}", binary.display()))?;

    Ok(())
}

fn estimate_package_count(project_root: &PathBuf) -> f32 {
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--format-version=1", "--no-deps"])
        .current_dir(project_root)
        .output();

    let workspace_members = output
        .ok()
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
        .and_then(|v| v["packages"].as_array().map(|a| a.len()))
        .unwrap_or(5) as f32;

    (workspace_members * 30.0).max(50.0)
}
