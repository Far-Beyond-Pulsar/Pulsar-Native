//! Running a build configuration.
//!
//! [`run_configuration`] turns the configuration into a [`plan::Plan`], runs
//! its steps on a worker thread, and reports through one notification:
//! progress while it works, then success, failure (with the compiler errors) or
//! cancellation. If the plan ends in a run step the game is launched and
//! watched. While anything here is running, `PlaybackState::build_running` is
//! set; [`cancel_build`] stops it.

pub mod cargo;
pub mod failure;
pub mod game;
pub mod plan;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use engine_state::build_config::{BuildConfiguration, TargetPlatform, build_cancel};
use engine_state::playback::playback;
use gpui::{App, AppContext as _, AsyncApp, Window};
use parking_lot::Mutex;
use ui::ContextModal as _;
use ui::notification::Notification;

use cargo::{Progress, StepError};
use plan::{Plan, Step, progress_ranges};

/// Notification identity: one progress notification per build, replaced in place.
struct BuildNotification;

/// Ask a running build to stop. It kills the current cargo command and runs
/// no further steps. Does nothing when no build is running.
pub fn cancel_build() {
    build_cancel().read().0.store(true, Ordering::Release);
}

/// The text of the progress notification.
fn progress_message(title: &str, detail: &str, pct: u32) -> String {
    if detail.is_empty() {
        format!("{title} ({pct}%)")
    } else {
        format!("{title}: {detail} ({pct}%)")
    }
}

/// Run `config` against the current project.
pub fn run_configuration(config: &BuildConfiguration, window: &mut Window, cx: &mut App) {
    let name = config.name.clone();

    let Some(project_root) = engine_state::get_project_path().map(PathBuf::from) else {
        window.push_notification(
            Notification::warning("Open a project before building.").title(name),
            cx,
        );
        return;
    };
    let plan = match plan::plan(config, TargetPlatform::host()) {
        Ok(plan) => plan,
        Err(error) => {
            window.push_notification(Notification::warning(error.to_string()).title(name), cx);
            return;
        }
    };

    let state = playback();
    if state.read().build_running {
        window.push_notification(
            Notification::info("A build is already running.").title(name),
            cx,
        );
        return;
    }
    build_cancel().read().0.store(false, Ordering::Release);
    state.update(|s| s.build_running = true);

    let pct = Arc::new(AtomicU32::new(0));
    let detail = Arc::new(Mutex::new(String::new()));
    let step_title = Arc::new(Mutex::new(String::from("Starting")));
    let progress = Progress::new(Arc::clone(&pct), Arc::clone(&detail));

    let (result_tx, result_rx) = smol::channel::bounded::<Result<(), StepError>>(1);
    {
        let plan = plan.clone();
        let root = project_root.clone();
        let step_title = Arc::clone(&step_title);
        std::thread::spawn(move || {
            let _ = result_tx.send_blocking(execute(&plan, &root, &progress, &step_title));
        });
    }

    window.push_notification(
        Notification::info("Starting…")
            .id::<BuildNotification>()
            .title(name.clone())
            .progress(0.0)
            .autohide(false),
        cx,
    );

    let window_handle = window.window_handle();
    cx.spawn(async move |async_app: &mut AsyncApp| {
        let mut last = (u32::MAX, String::new(), String::new());
        let result = loop {
            match result_rx.try_recv() {
                Ok(result) => break Some(result),
                Err(smol::channel::TryRecvError::Closed) => break None,
                Err(smol::channel::TryRecvError::Empty) => {}
            }
            let now = (
                pct.load(Ordering::Relaxed),
                step_title.lock().clone(),
                detail.lock().clone(),
            );
            if now != last {
                let message = progress_message(&now.1, &now.2, now.0);
                let fraction = now.0 as f32 / 100.0;
                last = now;
                let _ = async_app.update_window(window_handle, |_, window, cx| {
                    window.update_notification::<BuildNotification>(message, fraction, cx);
                });
            }
            async_app.background_executor().timer(Duration::from_millis(250)).await;
        };

        // The build is over whatever happened; free the button first.
        playback().update(|s| s.build_running = false);

        match result {
            Some(Ok(())) => {
                let _ = async_app.update_window(window_handle, |_, window, cx| {
                    window.push_notification(
                        Notification::success("Build succeeded")
                            .id::<BuildNotification>()
                            .title(name.clone())
                            .progress(1.0)
                            .autohide_delay(Duration::from_secs(3)),
                        cx,
                    );
                    for warning in &plan.warnings {
                        window.push_notification(
                            Notification::warning(warning.clone()).title(name.clone()),
                            cx,
                        );
                    }
                });
                let run = plan.steps.iter().find_map(|step| match step {
                    Step::Run { invocation, .. } => Some(invocation.clone()),
                    _ => None,
                });
                if let Some(invocation) = run {
                    game::launch_and_monitor(invocation, project_root, window_handle, async_app)
                        .await;
                }
            }
            Some(Err(StepError::Failed(message))) => {
                let _ = async_app.update_window(window_handle, |_, window, cx| {
                    failure::show(message, name.clone(), window, cx);
                });
            }
            Some(Err(StepError::Cancelled)) | None => {
                let cancelled = result.is_some();
                let _ = async_app.update_window(window_handle, |_, window, cx| {
                    let (note, title) = if cancelled {
                        ("Build cancelled", name.clone())
                    } else {
                        ("The build stopped unexpectedly.", name.clone())
                    };
                    window.push_notification(
                        Notification::info(note)
                            .id::<BuildNotification>()
                            .title(title)
                            .autohide_delay(Duration::from_secs(3)),
                        cx,
                    );
                });
            }
        }
    })
    .detach();
}

/// Run every step of `plan` in order, on the calling (worker) thread.
fn execute(
    plan: &Plan,
    project_root: &std::path::Path,
    progress: &Progress,
    step_title: &Mutex<String>,
) -> Result<(), StepError> {
    // The flag `cancel_build` raises; each cargo command watches it too.
    let cancel = build_cancel().read().0.clone();
    let ranges = progress_ranges(plan);
    let total = plan.steps.iter().filter(|s| !matches!(s, Step::Run { .. })).count();

    for (ix, (step, range)) in plan.steps.iter().zip(ranges).enumerate() {
        if cancel.load(Ordering::Acquire) {
            return Err(StepError::Cancelled);
        }
        if matches!(step, Step::Run { .. }) {
            break; // launched by the caller once the build is reported done
        }
        *step_title.lock() = format!("{} ({}/{})", step.title(), ix + 1, total);
        let slice = progress.slice(range);
        slice.status("");

        match step {
            Step::Bootstrap => engine_backend::services::ensure_core_bootstrap(project_root)
                .map_err(StepError::Failed)
                .map(|()| slice.set(100))?,
            Step::Update(invocation) | Step::Clean(invocation) => {
                cargo::run_plain(invocation, project_root, &slice, &cancel)?
            }
            Step::Check { invocation, .. } | Step::Build { invocation, .. } => {
                cargo::run_compiling(invocation, project_root, &slice, &cancel)?
            }
            Step::Run { .. } => unreachable!("handled above"),
        }
    }
    progress.slice((0, 100)).set(100);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_text_includes_the_detail_when_there_is_one() {
        assert_eq!(progress_message("Building (2/3)", "", 40), "Building (2/3) (40%)");
        assert_eq!(
            progress_message("Building (2/3)", "Compiling serde", 40),
            "Building (2/3): Compiling serde (40%)"
        );
    }
}
