//! "Build Core" split-button.
//!
//! Left side  — triggers the currently selected build mode immediately.
//! Right side — chevron opens a popup menu to switch mode.
//!
//! Modes:
//!   Build        — regenerate + cargo build
//!   Build + Run  — regenerate + cargo build, then `cargo run` (process tracked)
//!   Check        — cargo check only (fast type-check, no codegen)
//!
//! While a Build+Run process is alive the dropdown is disabled and a Stop
//! button appears next to it.  Killing the process re-enables everything.

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
use ui::{h_flex, ContextModal as _, Disableable as _, IconName, Sizable as _};

use super::super::state::{BuildMode, EditorMode, LevelEditorState};
use super::actions::SetBuildMode;

struct BuildCoreNotification;

pub struct BuildCoreButton;

impl BuildCoreButton {
    pub fn render<V>(
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let is_playing = state.editor_mode == EditorMode::Play;
        let game_running = state.game_running;
        let build_mode = state.build_mode;
        let entity_id = cx.entity().entity_id();

        let (label, icon, tooltip) = mode_label_icon_tooltip(build_mode);

        // ── Primary button ────────────────────────────────────────────────────
        let state_for_click = state_arc.clone();
        let primary = Button::new("build_core_primary")
            .icon(icon)
            .label(label)
            .tooltip(tooltip)
            .when(is_playing || game_running, |b| b.disabled(true))
            .on_click(move |_, window, cx| {
                let mode = state_for_click.read().build_mode;
                trigger_build(mode, state_for_click.clone(), entity_id, window, cx);
            });

        // ── Dropdown (chevron) ────────────────────────────────────────────────
        let state_for_menu = state_arc.clone();
        let dropdown = DropdownButton::new("build_core_dropdown")
            .button(primary)
            .when(!is_playing && !game_running, |d| {
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
            });

        // ── Stop button (only while game is running) ──────────────────────────
        let state_for_stop = state_arc.clone();
        let stop_btn = Button::new("build_core_stop")
            .icon(IconName::Square)
            .label("Stop")
            .tooltip("Stop the running game")
            .on_click(move |_, _, cx| {
                let mut state = state_for_stop.write();
                if let Some(mut child) = state.game_process.lock().take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                state.game_running = false;
                cx.notify(entity_id);
            });

        h_flex()
            .gap_1()
            .items_center()
            .child(dropdown)
            .when(game_running, |el| el.child(stop_btn))
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

fn project_root() -> Option<PathBuf> {
    engine_state::get_project_path().map(PathBuf::from)
}

fn trigger_build(
    mode: BuildMode,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    entity_id: EntityId,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(root) = project_root() else {
        window.push_notification(
            Notification::warning("No project open — open a project first."),
            cx,
        );
        return;
    };

    match mode {
        BuildMode::Check => run_check(root, window, cx),
        BuildMode::Build => run_build_pipeline(root, mode, None, entity_id, window, cx),
        BuildMode::BuildAndRun => {
            run_build_pipeline(root, mode, Some(state_arc), entity_id, window, cx)
        }
    }
}

// ── cargo check ──────────────────────────────────────────────────────────────

fn run_check(project_root: PathBuf, window: &mut Window, cx: &mut App) {
    let (result_tx, result_rx) = smol::channel::bounded::<Result<(), String>>(1);

    std::thread::spawn(move || {
        let result = std::process::Command::new("cargo")
            .args(["check"])
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

// ── cargo build (+ optional cargo run) ───────────────────────────────────────

fn run_build_pipeline(
    project_root: PathBuf,
    mode: BuildMode,
    // Only Some for BuildAndRun — used to store the game process and update state.
    state_arc: Option<Arc<parking_lot::RwLock<LevelEditorState>>>,
    entity_id: EntityId,
    window: &mut Window,
    cx: &mut App,
) {
    let progress_atomic: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));
    let progress_for_thread = Arc::clone(&progress_atomic);
    let progress_for_ui = Arc::clone(&progress_atomic);

    let (result_tx, result_rx) = smol::channel::bounded::<Result<(), String>>(1);

    let project_root_thread = project_root.clone();
    std::thread::spawn(move || {
        let result = run_cargo_build(&project_root_thread, progress_for_thread);
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
                Ok(Ok(())) => {
                    // Build succeeded.
                    let _ = async_app.update_window(window_handle, |_, window, cx| {
                        window.push_notification(
                            Notification::success("Build succeeded.")
                                .id::<BuildCoreNotification>()
                                .title(title)
                                .progress(1.0)
                                .autohide_delay(Duration::from_secs(3)),
                            cx,
                        );
                    });

                    if mode == BuildMode::BuildAndRun {
                        if let Some(state) = state_arc {
                            launch_and_monitor(
                                project_root,
                                state,
                                entity_id,
                                window_handle,
                                async_app,
                            )
                            .await;
                        }
                    }
                    return;
                }
                Ok(Err(msg)) => {
                    let _ = async_app.update_window(window_handle, |_, window, cx| {
                        window.push_notification(
                            Notification::error(msg)
                                .id::<BuildCoreNotification>()
                                .title(title),
                            cx,
                        );
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
                            );
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

// ── Game process lifecycle ────────────────────────────────────────────────────

async fn launch_and_monitor(
    project_root: PathBuf,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    entity_id: EntityId,
    window_handle: AnyWindowHandle,
    async_app: &mut AsyncApp,
) {
    // Pipe stderr so we can capture crash output and surface it as a notification.
    // stdout is inherited so any game console output goes to the editor's terminal.
    let mut child = match std::process::Command::new("cargo")
        .args(["run", "--release"])
        .current_dir(&project_root)
        .env("RUST_BACKTRACE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = async_app.update_window(window_handle, |_, window, cx| {
                window.push_notification(
                    Notification::error(format!("Failed to launch game: {e}"))
                        .title("Build + Run"),
                    cx,
                );
            });
            return;
        }
    };

    // Drain stderr on a background thread so the pipe never blocks the game.
    let stderr_pipe = child.stderr.take();
    let (stderr_tx, stderr_rx) = smol::channel::bounded::<String>(1);
    std::thread::spawn(move || {
        let Some(pipe) = stderr_pipe else { return };
        use std::io::Read as _;
        let mut buf = String::new();
        let _ = BufReader::new(pipe).read_to_string(&mut buf);
        smol::block_on(stderr_tx.send(buf)).ok();
    });

    // Store the handle and mark running.
    {
        let mut state = state_arc.write();
        *state.game_process.lock() = Some(child);
        state.game_running = true;
    }
    let _ = async_app.update_window(window_handle, |_, _, cx| cx.notify(entity_id));

    // Poll until the process exits.
    loop {
        async_app
            .background_executor()
            .timer(Duration::from_millis(500))
            .await;

        let exit_status = {
            let state = state_arc.read();
            let mut guard = state.game_process.lock();
            match guard.as_mut() {
                None => Some(None), // Stop button already killed it — treat as exited.
                Some(child) => match child.try_wait() {
                    Ok(Some(status)) => Some(Some(status)),
                    Ok(None) => None, // still running
                    Err(_) => Some(None),
                },
            }
        };

        if let Some(status) = exit_status {
            // Clean up the handle.
            let mut state = state_arc.write();
            state.game_process.lock().take();
            state.game_running = false;

            // Surface a notification if the process exited with an error.
            let stderr = stderr_rx.try_recv().unwrap_or_default();
            let failed = status.map(|s| !s.success()).unwrap_or(false);

            // Write full stderr to a crash report file if the process failed.
            let crash_report_path = if failed && !stderr.trim().is_empty() {
                save_crash_report(&project_root, &stderr)
            } else {
                None
            };

            if !stderr.trim().is_empty() {
                if failed {
                    tracing::error!("[BUILD+RUN] game stderr:\n{}", stderr.trim());
                } else {
                    tracing::warn!("[BUILD+RUN] game stderr:\n{}", stderr.trim());
                }
            }

            let _ = async_app.update_window(window_handle, |_, window, cx| {
                cx.notify(entity_id);
                if failed {
                    let msg = if stderr.trim().is_empty() {
                        "Game exited with a non-zero status code.".to_string()
                    } else {
                        let tail = stderr.trim();
                        let tail = if tail.len() > 600 { &tail[tail.len() - 600..] } else { tail };
                        match &crash_report_path {
                            Some(p) => format!("Game crashed.\nReport saved to: {}\n\n{tail}", p.display()),
                            None => format!("Game crashed:\n{tail}"),
                        }
                    };
                    window.push_notification(
                        Notification::error(msg).title("Build + Run"),
                        cx,
                    );
                } else {
                    tracing::info!("[BUILD+RUN] game process exited cleanly");
                }
            });
            return;
        }
    }
}

// ── cargo build (blocking, called on a thread) ────────────────────────────────

fn run_cargo_build(project_root: &PathBuf, progress: Arc<AtomicU32>) -> Result<(), String> {
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

    super::cargo_progress::run_cargo_build(project_root, progress)
}

fn save_crash_report(project_root: &PathBuf, stderr: &str) -> Option<PathBuf> {
    let crash_dir = project_root.join(".pulsar").join("crash-reports");
    if let Err(e) = std::fs::create_dir_all(&crash_dir) {
        tracing::warn!("[BUILD+RUN] could not create crash-reports dir: {e}");
        return None;
    }

    // Timestamp in a filename-safe format: YYYY-MM-DD_HH-MM-SS
    let ts = {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // Manual formatting to avoid pulling in chrono.
        let secs = now % 60;
        let mins = (now / 60) % 60;
        let hours = (now / 3600) % 24;
        let days = now / 86400;
        // Days since Unix epoch → approximate calendar date (good enough for filenames).
        let (y, m, d) = days_to_ymd(days);
        format!("{y:04}-{m:02}-{d:02}_{hours:02}-{mins:02}-{secs:02}")
    };

    let path = crash_dir.join(format!("crash_{ts}.log"));
    match std::fs::write(&path, stderr) {
        Ok(()) => {
            tracing::info!("[BUILD+RUN] crash report written to {}", path.display());
            Some(path)
        }
        Err(e) => {
            tracing::warn!("[BUILD+RUN] could not write crash report: {e}");
            None
        }
    }
}

/// Convert days since Unix epoch (1970-01-01) to (year, month, day).
fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Gregorian proleptic calendar approximation, sufficient for filenames.
    let mut year = 1970u64;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let days_in_year = if leap { 366 } else { 365 };
        if days < days_in_year { break; }
        days -= days_in_year;
        year += 1;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days_in_month = [31u64, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u64;
    for dim in &days_in_month {
        if days < *dim { break; }
        days -= dim;
        month += 1;
    }
    (year, month, days + 1)
}

