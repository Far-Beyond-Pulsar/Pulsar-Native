//! Launching the built game and watching it until it exits.

use std::io::{BufReader, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use gpui::{AnyWindowHandle, AppContext as _, AsyncApp};
use ui::ContextModal as _;
use ui::notification::Notification;

use super::plan::Invocation;

const TITLE: &str = "Run";

/// Start `invocation` (a `cargo run`), record its process in the engine's
/// playback state, and wait for it to end, reporting a crash if it did.
pub async fn launch_and_monitor(
    invocation: Invocation,
    project_root: PathBuf,
    window_handle: AnyWindowHandle,
    async_app: &mut AsyncApp,
) {
    // stderr is piped so a crash can be reported; stdout goes to the editor's
    // own terminal so game logging stays visible.
    let spawned = Command::new("cargo")
        .arg(invocation.subcommand)
        .args(&invocation.args)
        .envs(invocation.envs.iter().map(|(k, v)| (k, v)))
        .current_dir(&project_root)
        .env("RUST_BACKTRACE", "1")
        // Publish the game for the profiler (listed in its start screen;
        // nothing is recorded until a recording starts).
        .env(profiling::remote::ENV_FLAG, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            let _ = async_app.update_window(window_handle, |_, window, cx| {
                window.push_notification(
                    Notification::error(format!("Could not launch the game: {error}")).title(TITLE),
                    cx,
                );
            });
            return;
        }
    };

    // Drain stderr off-thread so the pipe never blocks the game.
    let stderr_pipe = child.stderr.take();
    let (stderr_tx, stderr_rx) = smol::channel::bounded::<String>(1);
    std::thread::spawn(move || {
        let Some(pipe) = stderr_pipe else { return };
        let mut buffer = String::new();
        let _ = BufReader::new(pipe).read_to_string(&mut buffer);
        let _ = smol::block_on(stderr_tx.send(buffer));
    });

    let playback = engine_state::playback::playback();
    let game = engine_state::playback::game_process();
    *game.read().0.lock() = Some(child);
    playback.update(|s| s.game_running = true);

    let exit_status = loop {
        async_app
            .background_executor()
            .timer(Duration::from_millis(500))
            .await;

        let process = game.read();
        let mut guard = process.0.lock();
        match guard.as_mut() {
            // Stopped by the Stop button, which already took and killed it.
            None => break None,
            Some(child) => match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(_) => break None,
            },
        }
    };

    game.read().0.lock().take();
    playback.update(|s| s.game_running = false);

    let stderr = stderr_rx.try_recv().unwrap_or_default();
    let failed = exit_status.is_some_and(|s| !s.success());
    let report = (failed && !stderr.trim().is_empty())
        .then(|| save_crash_report(&project_root, &stderr))
        .flatten();

    if !stderr.trim().is_empty() {
        if failed {
            tracing::error!("[run] game stderr:\n{}", stderr.trim());
        } else {
            tracing::warn!("[run] game stderr:\n{}", stderr.trim());
        }
    }
    if !failed {
        tracing::info!("[run] game exited cleanly");
        return;
    }

    let message = if stderr.trim().is_empty() {
        "The game exited with a non-zero status code.".to_string()
    } else {
        let text = stderr.trim();
        let tail = text.get(text.len().saturating_sub(600)..).unwrap_or(text);
        match &report {
            Some(path) => format!(
                "The game crashed.\nReport saved to {}\n\n{tail}",
                path.display()
            ),
            None => format!("The game crashed:\n{tail}"),
        }
    };
    let _ = async_app.update_window(window_handle, |_, window, cx| {
        window.push_notification(Notification::error(message).title(TITLE), cx);
    });
}

/// Write `stderr` to `<project>/.pulsar/crash-reports/crash_<time>.log`.
fn save_crash_report(project_root: &Path, stderr: &str) -> Option<PathBuf> {
    let dir = project_root.join(".pulsar").join("crash-reports");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        tracing::warn!("[run] could not create the crash-reports folder: {error}");
        return None;
    }
    let path = dir.join(format!("crash_{}.log", timestamp()));
    match std::fs::write(&path, stderr) {
        Ok(()) => {
            tracing::info!("[run] crash report written to {}", path.display());
            Some(path)
        }
        Err(error) => {
            tracing::warn!("[run] could not write the crash report: {error}");
            None
        }
    }
}

/// `YYYY-MM-DD_HH-MM-SS` (UTC), safe in a file name.
fn timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (year, month, day) = civil_from_days(seconds / 86_400);
    let of_day = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}_{:02}-{:02}-{:02}",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60
    )
}

/// Days since 1970-01-01 to a (year, month, day) in the proleptic Gregorian
/// calendar.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u64;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u64;
    let year = (year_of_era + era * 400 + i64::from(month <= 2)) as u64;
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_come_out_right() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(59), (1970, 3, 1), "1970 is not a leap year");
        assert_eq!(
            civil_from_days(11_016),
            (2000, 2, 29),
            "2000 is a leap year"
        );
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
    }
}
