//! Long-form build pipeline: full `cargo build` (+ optional run) and the
//! spawned game-process lifecycle.

use super::crash::save_crash_report;
use super::*;

// ── cargo build (+ optional cargo run) ───────────────────────────────────────

pub(super) fn run_build_pipeline(
    project_root: PathBuf,
    mode: BuildMode,
    // Only Some for BuildAndRun — used to store the game process and update state.
    state_arc: Option<Arc<parking_lot::RwLock<LevelEditorState>>>,
    entity_id: EntityId,
    window: &mut Window,
    cx: &mut App,
) {
    let progress_atomic: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));
    let status_cell = super::super::cargo_progress::StatusCell::default();
    let progress_for_thread = Arc::clone(&progress_atomic);
    let progress_for_ui = Arc::clone(&progress_atomic);
    let status_for_thread = Arc::clone(&status_cell);
    let status_for_ui = Arc::clone(&status_cell);

    let (result_tx, result_rx) = smol::channel::bounded::<Result<(), String>>(1);

    let project_root_thread = project_root.clone();
    std::thread::spawn(move || {
        let result = run_cargo_build(&project_root_thread, progress_for_thread, status_for_thread);
        smol::block_on(result_tx.send(result));
    });

    let title = if mode == BuildMode::BuildAndRun {
        t!("Notification.Title.BuildRun").to_string()
    } else {
        t!("Notification.Title.BuildCore").to_string()
    };

    window.push_notification(
        Notification::info(t!("Notification.Message.StartingBuild").to_string())
            .id::<BuildCoreNotification>()
            .title(title.clone())
            .progress(0.0)
            .autohide(false),
        cx,
    );

    let window_handle = window.window_handle();

    cx.spawn(async move |async_app: &mut AsyncApp| {
        let mut last_pct: u32 = u32::MAX;
        let mut last_status = String::new();

        loop {
            match result_rx.try_recv() {
                Ok(Ok(())) => {
                    let _ = async_app.update_window(window_handle, |_, window, cx| {
                        window.push_notification(
                            Notification::success(
                                t!("Notification.Message.BuildSucceeded").to_string(),
                            )
                            .id::<BuildCoreNotification>()
                            .title(title.clone())
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
                                .title(title.clone()),
                            cx,
                        );
                    });
                    return;
                }
                Err(smol::channel::TryRecvError::Closed) => return,
                Err(smol::channel::TryRecvError::Empty) => {
                    let pct = progress_for_ui.load(Ordering::Relaxed);
                    let status = status_for_ui.lock().clone();
                    if pct != last_pct || status != last_status {
                        last_pct = pct;
                        last_status = status.clone();
                        let msg = if status.is_empty() {
                            format!("Building… ({pct}%)")
                        } else {
                            format!("{status} ({pct}%)")
                        };
                        let _ = async_app.update_window(window_handle, |_, window, cx| {
                            window.update_notification::<BuildCoreNotification>(
                                msg,
                                pct as f32 / 100.0,
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

pub(super) async fn launch_and_monitor(
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
                    Notification::error(
                        t!(
                            "Notification.Message.FailedToLaunchGame",
                            error => e.to_string()
                        )
                        .to_string(),
                    )
                    .title(t!("Notification.Title.BuildRun").to_string()),
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
        smol::block_on(stderr_tx.send(buf));
    });

    // Store the handle and mark running.
    {
        let mut state = state_arc.write();
        *state.build.game_process.lock() = Some(child);
        state.build.game_running = true;
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
            let mut guard = state.build.game_process.lock();
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
            state.build.game_process.lock().take();
            state.build.game_running = false;

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
                        let tail = if tail.len() > 600 {
                            &tail[tail.len() - 600..]
                        } else {
                            tail
                        };
                        match &crash_report_path {
                            Some(p) => {
                                format!("Game crashed.\nReport saved to: {}\n\n{tail}", p.display())
                            }
                            None => format!("Game crashed:\n{tail}"),
                        }
                    };
                    window.push_notification(
                        Notification::error(msg)
                            .title(t!("Notification.Title.BuildRun").to_string()),
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

pub(super) fn run_cargo_build(
    project_root: &PathBuf,
    progress: Arc<AtomicU32>,
    status: super::super::cargo_progress::StatusCell,
) -> Result<(), String> {
    engine_backend::services::ensure_core_bootstrap(project_root)?;
    super::super::cargo_progress::run_cargo_build(project_root, progress, status)
}