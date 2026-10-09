//! Running one cargo command with live progress and cancellation.

use std::io::{BufRead as _, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use editor_task_queue::TaskContext;
use parking_lot::Mutex;

use super::plan::Invocation;

/// Separates compiler errors inside a failure message, so the failure dialog
/// can show each one on its own.
pub const ERROR_SEPARATOR: &str = "--- PULSAR BUILD ERROR ---";

/// Why a step stopped.
#[derive(Debug, PartialEq, Eq)]
pub enum StepError {
    Failed(String),
    Cancelled,
}

/// Where a step reports progress: a slice of the overall bar and the status
/// line. `set` takes the step's own 0–100 and maps it into the slice.
#[derive(Clone)]
pub struct Progress {
    pct: Arc<AtomicU32>,
    status: Arc<Mutex<String>>,
    from: u32,
    to: u32,
    task: Option<TaskContext>,
}

impl Progress {
    pub fn new(pct: Arc<AtomicU32>, status: Arc<Mutex<String>>) -> Self {
        Self {
            pct,
            status,
            from: 0,
            to: 100,
            task: None,
        }
    }

    pub fn with_task_context(mut self, task: TaskContext) -> Self {
        self.task = Some(task);
        self
    }

    /// The same bar, restricted to `from..to` percent.
    pub fn slice(&self, (from, to): (u32, u32)) -> Self {
        Self {
            from,
            to,
            ..self.clone()
        }
    }

    pub fn set(&self, internal: u32) {
        let span = self.to.saturating_sub(self.from);
        let pct = self.from + internal.min(100) * span / 100;
        self.pct.store(pct, Ordering::Relaxed);
        if let Some(task) = &self.task {
            task.report_progress(pct as f32 / 100.0, self.status.lock().clone());
        }
    }

    pub fn status(&self, text: impl Into<String>) {
        let text = text.into();
        *self.status.lock() = text.clone();
        if let Some(task) = &self.task {
            task.report_progress(self.pct.load(Ordering::Relaxed) as f32 / 100.0, text);
        }
    }
}

fn command(invocation: &Invocation, project_root: &Path) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.arg(invocation.subcommand)
        .args(&invocation.args)
        .envs(invocation.envs.iter().map(|(k, v)| (k, v)))
        .current_dir(project_root);
    cmd
}

/// Kill `child` if `cancel` is raised before `done` is. Returns when either is.
fn watch_for_cancel(child: Arc<Mutex<Child>>, cancel: Arc<AtomicBool>, done: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while !done.load(Ordering::Acquire) {
            if cancel.load(Ordering::Acquire) {
                let _ = child.lock().kill();
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
}

/// Pull `Updating …` / `Locking …` out of cargo's stderr for the status line.
fn registry_activity(line: &str) -> Option<String> {
    let line = line.trim_start();
    let what = line
        .strip_prefix("Updating ")
        .or_else(|| line.strip_prefix("Locking "))?
        .replace('`', "");
    Some(format!(
        "Updating {}",
        what.rsplit('/').next().unwrap_or(&what)
    ))
}

/// Run `cargo check` or `cargo build`, following its JSON messages for
/// progress and collecting compiler errors.
pub fn run_compiling(
    invocation: &Invocation,
    project_root: &Path,
    progress: &Progress,
    cancel: &Arc<AtomicBool>,
) -> Result<(), StepError> {
    let mut cmd = command(invocation, project_root);
    cmd.arg("--message-format=json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    tracing::info!("[build] {}", invocation.display());

    let mut child = cmd.spawn().map_err(|e| {
        StepError::Failed(format!(
            "Could not start cargo {}: {e}",
            invocation.subcommand
        ))
    })?;
    let stdout = BufReader::new(child.stdout.take().expect("piped"));
    let stderr = BufReader::new(child.stderr.take().expect("piped"));
    let child = Arc::new(Mutex::new(child));
    let done = Arc::new(AtomicBool::new(false));
    watch_for_cancel(child.clone(), cancel.clone(), done.clone());

    // stderr carries registry activity and, when there is no JSON, the failure.
    let stderr_progress = progress.clone();
    let stderr_thread = std::thread::spawn(move || {
        let mut text = String::new();
        for line in stderr.lines().map_while(Result::ok) {
            if let Some(activity) = registry_activity(&line) {
                stderr_progress.status(activity);
            }
            text.push_str(&line);
            text.push('\n');
        }
        text
    });

    // Artifacts fill 10–94 %; the first 10 % is dependency resolution and the
    // last 6 % linking, which produces no messages.
    let mut seen = 0u32;
    let mut expected = 4.0f32;
    let mut errors: Vec<String> = Vec::new();
    for line in stdout.lines().map_while(Result::ok) {
        if !line.contains(r#""reason""#) {
            continue;
        }
        let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match message["reason"].as_str() {
            Some("compiler-message") => {
                let m = &message["message"];
                if m["level"].as_str() == Some("error") {
                    if let Some(rendered) = m["rendered"].as_str() {
                        if !errors.iter().any(|e| e == rendered) {
                            errors.push(rendered.to_owned());
                        }
                    }
                }
            }
            Some("compiler-artifact") => {
                seen += 1;
                expected = expected.max(seen as f32 + 4.0);
                progress.set(10 + ((seen as f32 / expected) * 84.0) as u32);
                if !message["fresh"].as_bool().unwrap_or(false) {
                    let name = message["target"]["name"].as_str().unwrap_or("?");
                    progress.status(format!("Compiling {name}"));
                }
            }
            _ => {}
        }
    }

    let stderr_text = stderr_thread.join().unwrap_or_default();
    let status = child.lock().wait();
    done.store(true, Ordering::Release);

    if cancel.load(Ordering::Acquire) {
        return Err(StepError::Cancelled);
    }
    match status {
        Ok(status) if status.success() => {
            progress.set(100);
            Ok(())
        }
        Ok(_) => {
            let detail = if errors.is_empty() {
                stderr_text.trim().to_owned()
            } else {
                errors.join(&format!("\n\n{ERROR_SEPARATOR}\n\n"))
            };
            Err(StepError::Failed(format!(
                "cargo {} failed:\n\n{detail}",
                invocation.subcommand
            )))
        }
        Err(e) => Err(StepError::Failed(format!("Could not wait for cargo: {e}"))),
    }
}

/// Run a cargo command with human-readable output (`update`, `clean`),
/// reporting registry activity on the status line.
pub fn run_plain(
    invocation: &Invocation,
    project_root: &Path,
    progress: &Progress,
    cancel: &Arc<AtomicBool>,
) -> Result<(), StepError> {
    let mut cmd = command(invocation, project_root);
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    tracing::info!("[build] {}", invocation.display());

    let mut child = cmd.spawn().map_err(|e| {
        StepError::Failed(format!(
            "Could not start cargo {}: {e}",
            invocation.subcommand
        ))
    })?;
    let stderr = BufReader::new(child.stderr.take().expect("piped"));
    let child = Arc::new(Mutex::new(child));
    let done = Arc::new(AtomicBool::new(false));
    watch_for_cancel(child.clone(), cancel.clone(), done.clone());

    let mut output = String::new();
    let mut activity = 0u32;
    for line in stderr.lines().map_while(Result::ok) {
        if let Some(text) = registry_activity(&line) {
            activity += 1;
            progress.set((activity * 3).min(95));
            progress.status(text);
        }
        output.push_str(&line);
        output.push('\n');
    }

    let status = child.lock().wait();
    done.store(true, Ordering::Release);

    if cancel.load(Ordering::Acquire) {
        return Err(StepError::Cancelled);
    }
    match status {
        Ok(status) if status.success() => {
            progress.set(100);
            Ok(())
        }
        Ok(_) => Err(StepError::Failed(format!(
            "cargo {} failed:\n\n{}",
            invocation.subcommand,
            output.trim()
        ))),
        Err(e) => Err(StepError::Failed(format!("Could not wait for cargo: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_maps_into_its_slice() {
        let bar = Progress::new(Arc::default(), Arc::default());
        let slice = bar.slice((20, 60));
        slice.set(0);
        assert_eq!(bar.pct.load(Ordering::Relaxed), 20);
        slice.set(50);
        assert_eq!(bar.pct.load(Ordering::Relaxed), 40);
        slice.set(100);
        assert_eq!(bar.pct.load(Ordering::Relaxed), 60);
        slice.set(500);
        assert_eq!(bar.pct.load(Ordering::Relaxed), 60, "clamped");
    }

    #[test]
    fn registry_lines_become_short_status_text() {
        assert_eq!(
            registry_activity("    Updating crates.io index").as_deref(),
            Some("Updating crates.io index")
        );
        assert_eq!(
            registry_activity("  Locking `serde` v1.0 -> v1.1").as_deref(),
            Some("Updating serde v1.0 -> v1.1")
        );
        assert!(registry_activity("   Compiling foo").is_none());
    }
}
