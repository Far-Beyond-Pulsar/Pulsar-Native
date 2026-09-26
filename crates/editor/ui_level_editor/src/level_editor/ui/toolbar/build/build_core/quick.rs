//! Quick single cargo tasks: scratch build/check, `cargo check`, `cargo update`,
//! and `cargo update` + build & run.

use super::pipeline::launch_and_monitor;
use super::*;

// ── scratch (clean + build/check) ────────────────────────────────────────────

pub(super) fn run_scratch(
    project_root: PathBuf,
    inner_mode: BuildMode, // Build, BuildAndRun, or Check
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
        let result = (|| -> Result<(), String> {
            engine_backend::services::ensure_core_bootstrap(&project_root_thread)?;
            // Clean occupies 0–20 %.
            super::super::cargo_progress::run_cargo_clean(
                &project_root_thread,
                Arc::clone(&progress_for_thread),
                Arc::clone(&status_for_thread),
                20,
            )?;
            // Main command occupies 20–100 %.
            match inner_mode {
                BuildMode::Check | BuildMode::CheckScratch => {
                    super::super::cargo_progress::run_cargo_check_from(
                        &project_root_thread,
                        Arc::clone(&progress_for_thread),
                        Arc::clone(&status_for_thread),
                        20,
                    )
                }
                _ => super::super::cargo_progress::run_cargo_build_from(
                    &project_root_thread,
                    Arc::clone(&progress_for_thread),
                    Arc::clone(&status_for_thread),
                    20,
                ),
            }
        })();
        smol::block_on(result_tx.send(result));
    });

    let title = match inner_mode {
        BuildMode::Check => t!("Notification.Title.CheckScratch").to_string(),
        BuildMode::BuildAndRun => t!("Notification.Title.BuildRunScratch").to_string(),
        _ => t!("Notification.Title.BuildScratch").to_string(),
    };

    window.push_notification(
        Notification::info(t!("Notification.Message.CleaningBuildArtifacts").to_string())
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
                    if matches!(inner_mode, BuildMode::BuildAndRun) {
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
                        show_build_failure(msg, title.clone(), window, cx);
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

// ── cargo check ──────────────────────────────────────────────────────────────

pub(super) fn run_check(project_root: PathBuf, window: &mut Window, cx: &mut App) {
    let progress_atomic: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));
    let status_cell = super::super::cargo_progress::StatusCell::default();
    let progress_for_thread = Arc::clone(&progress_atomic);
    let progress_for_ui = Arc::clone(&progress_atomic);
    let status_for_thread = Arc::clone(&status_cell);
    let status_for_ui = Arc::clone(&status_cell);
    let (result_tx, result_rx) = smol::channel::bounded::<Result<(), String>>(1);

    std::thread::spawn(move || {
        let result = super::super::cargo_progress::run_cargo_check(
            &project_root,
            progress_for_thread,
            status_for_thread,
        );
        smol::block_on(result_tx.send(result));
    });

    window.push_notification(
        Notification::info(t!("Notification.Message.StartingCheck").to_string())
            .id::<BuildCoreNotification>()
            .title(t!("Notification.Title.Check").to_string())
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
                Ok(result) => {
                    let _ = async_app.update_window(window_handle, |_, window, cx| match result {
                        Ok(()) => window.push_notification(
                            Notification::success(
                                t!("Notification.Message.CheckPassed").to_string(),
                            )
                            .id::<BuildCoreNotification>()
                            .title(t!("Notification.Title.Check").to_string())
                            .progress(1.0)
                            .autohide_delay(Duration::from_secs(3)),
                            cx,
                        ),
                        Err(msg) => show_build_failure(
                            msg,
                            t!("Notification.Title.Check").to_string(),
                            window,
                            cx,
                        ),
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
                            format!("Checking… ({pct}%)")
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

// ── cargo update ─────────────────────────────────────────────────────────────

pub(super) fn run_update(project_root: PathBuf, window: &mut Window, cx: &mut App) {
    let progress_atomic: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));
    let status_cell = super::super::cargo_progress::StatusCell::default();
    let progress_for_thread = Arc::clone(&progress_atomic);
    let progress_for_ui = Arc::clone(&progress_atomic);
    let status_for_thread = Arc::clone(&status_cell);
    let status_for_ui = Arc::clone(&status_cell);
    let (result_tx, result_rx) = smol::channel::bounded::<Result<(), String>>(1);

    std::thread::spawn(move || {
        let result = super::super::cargo_progress::run_cargo_update(
            &project_root,
            progress_for_thread,
            status_for_thread,
        );
        smol::block_on(result_tx.send(result));
    });

    window.push_notification(
        Notification::info(t!("Notification.Message.UpdatingDependencies").to_string())
            .id::<BuildCoreNotification>()
            .title(t!("Notification.Title.Update").to_string())
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
                Ok(result) => {
                    let _ = async_app.update_window(window_handle, |_, window, cx| match result {
                        Ok(()) => window.push_notification(
                            Notification::success(
                                t!("Notification.Message.DependenciesUpdated").to_string(),
                            )
                            .id::<BuildCoreNotification>()
                            .title(t!("Notification.Title.Update").to_string())
                            .progress(1.0)
                            .autohide_delay(Duration::from_secs(3)),
                            cx,
                        ),
                        Err(msg) => show_build_failure(
                            msg,
                            t!("Notification.Title.Update").to_string(),
                            window,
                            cx,
                        ),
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
                            format!("Updating dependencies… ({pct}%)")
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

// ── cargo update, then build + run ──────────────────────────────────────────

/// `cargo update` (0–30 %) then `cargo build --release` (30–100 %), then
/// launch the game. Use this after pushing changes to a git-dependency engine
/// repo so the project's `Cargo.lock` is bumped to the new commit before
/// building — otherwise the build silently reuses the previously-locked rev.
pub(super) fn run_update_build_and_run(
    project_root: PathBuf,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    entity_id: EntityId,
    window: &mut Window,
    cx: &mut App,
) {
    const UPDATE_UP_TO_PCT: u32 = 30;

    let progress_atomic: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));
    let status_cell = super::super::cargo_progress::StatusCell::default();
    let progress_for_thread = Arc::clone(&progress_atomic);
    let progress_for_ui = Arc::clone(&progress_atomic);
    let status_for_thread = Arc::clone(&status_cell);
    let status_for_ui = Arc::clone(&status_cell);
    let (result_tx, result_rx) = smol::channel::bounded::<Result<(), String>>(1);

    let title = t!("Notification.Title.UpdateBuildRun").to_string();
    let project_root_thread = project_root.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            super::super::cargo_progress::run_cargo_update_to(
                &project_root_thread,
                Arc::clone(&progress_for_thread),
                Arc::clone(&status_for_thread),
                UPDATE_UP_TO_PCT,
            )?;
            super::super::cargo_progress::run_cargo_build_from(
                &project_root_thread,
                Arc::clone(&progress_for_thread),
                Arc::clone(&status_for_thread),
                UPDATE_UP_TO_PCT,
            )
        })();
        smol::block_on(result_tx.send(result));
    });

    window.push_notification(
        Notification::info(t!("Notification.Message.UpdatingDependencies").to_string())
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
                    launch_and_monitor(
                        project_root,
                        state_arc,
                        entity_id,
                        window_handle,
                        async_app,
                    )
                    .await;
                    return;
                }
                Ok(Err(msg)) => {
                    let _ = async_app.update_window(window_handle, |_, window, cx| {
                        show_build_failure(msg, title.clone(), window, cx);
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
