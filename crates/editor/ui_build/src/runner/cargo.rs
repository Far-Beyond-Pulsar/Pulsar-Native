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

/// Kill `child`, and everything it started, if `cancel` is raised before
/// `done` is. Returns when either is.
fn watch_for_cancel(child: Arc<Mutex<Child>>, cancel: Arc<AtomicBool>, done: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while !done.load(Ordering::Acquire) {
            if cancel.load(Ordering::Acquire) {
                super::process_tree::kill_tree(&mut child.lock());
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

/// How many `compiler-artifact` messages a build of `invocation` emits, from
/// `cargo metadata`: one per package reachable through normal and build
/// dependencies (filtered to the build's target platform), one more per
/// build script, and one per binary of a workspace member. Fresh units are
/// reported too, so this is the total whether or not they need rebuilding.
/// `None` when cargo metadata can't be read; the caller then estimates as it
/// goes.
fn expected_artifacts(invocation: &Invocation, project_root: &Path) -> Option<u32> {
    let target = invocation
        .args
        .iter()
        .position(|arg| arg == "--target")
        .and_then(|i| invocation.args.get(i + 1).cloned())
        .or_else(host_triple)?;
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--filter-platform", &target])
        .envs(invocation.envs.iter().map(|(k, v)| (k, v)))
        .current_dir(project_root)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    count_artifacts(&serde_json::from_slice(&output.stdout).ok()?)
}

fn host_triple() -> Option<String> {
    let output = Command::new("rustc").arg("-vV").output().ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_owned))
}

/// See [`expected_artifacts`].
fn count_artifacts(metadata: &serde_json::Value) -> Option<u32> {
    use std::collections::{HashMap, HashSet};
    let members: HashSet<&str> = metadata["workspace_members"]
        .as_array()?
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    let nodes: HashMap<&str, &serde_json::Value> = metadata["resolve"]["nodes"]
        .as_array()?
        .iter()
        .filter_map(|node| Some((node["id"].as_str()?, node)))
        .collect();
    let packages: HashMap<&str, &serde_json::Value> = metadata["packages"]
        .as_array()?
        .iter()
        .filter_map(|package| Some((package["id"].as_str()?, package)))
        .collect();

    // Packages a build compiles: members and what they reach without dev
    // dependencies (`kind` null is a normal dependency).
    let mut reached: HashSet<&str> = HashSet::new();
    let mut queue: Vec<&str> = members.iter().copied().collect();
    while let Some(id) = queue.pop() {
        if !reached.insert(id) {
            continue;
        }
        let Some(node) = nodes.get(id) else { continue };
        for dep in node["deps"].as_array().into_iter().flatten() {
            let built = dep["dep_kinds"].as_array().is_some_and(|kinds| {
                kinds.iter().any(|kind| kind["kind"].as_str() != Some("dev"))
            });
            if let (true, Some(dep_id)) = (built, dep["pkg"].as_str()) {
                queue.push(dep_id);
            }
        }
    }

    let mut total = 0u32;
    for id in &reached {
        let targets = packages.get(id).and_then(|p| p["targets"].as_array());
        let count = |kind: &str| -> u32 {
            targets
                .into_iter()
                .flatten()
                .filter(|target| {
                    target["kind"]
                        .as_array()
                        .is_some_and(|k| k.iter().any(|k| k.as_str() == Some(kind)))
                })
                .count() as u32
        };
        let library = count("lib") + count("proc-macro") + count("rlib");
        total += library.min(1) + count("custom-build");
        if members.contains(id) {
            total += count("bin");
        }
    }
    (total > 0).then_some(total)
}

/// Run `cargo check` or `cargo build`, following its JSON messages for
/// progress and collecting compiler errors.
pub fn run_compiling(
    invocation: &Invocation,
    project_root: &Path,
    progress: &Progress,
    cancel: &Arc<AtomicBool>,
) -> Result<(), StepError> {
    progress.status("Resolving dependencies");
    let total = expected_artifacts(invocation, project_root);

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
    // Without a total, keep the estimate a few artifacts ahead of what has
    // been seen (it then crawls towards 94 % rather than knowing the end).
    let mut expected = total.map_or(4.0, |total| total as f32);
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
                if total.is_none() {
                    expected = expected.max(seen as f32 + 4.0);
                }
                progress.set(10 + ((seen as f32 / expected).min(1.0) * 84.0) as u32);
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
    fn artifacts_are_counted_from_the_build_graph() {
        // app (lib + bin + build script) -> serde (lib) -> serde_derive
        // (proc-macro); app's dev-dependency on tester is not built.
        let metadata = serde_json::json!({
            "workspace_members": ["app"],
            "packages": [
                {"id": "app", "targets": [
                    {"kind": ["lib"]}, {"kind": ["bin"]}, {"kind": ["custom-build"]}
                ]},
                {"id": "serde", "targets": [{"kind": ["lib"]}]},
                {"id": "serde_derive", "targets": [{"kind": ["proc-macro"]}]},
                {"id": "tester", "targets": [{"kind": ["lib"]}]}
            ],
            "resolve": {"nodes": [
                {"id": "app", "deps": [
                    {"pkg": "serde", "dep_kinds": [{"kind": null}]},
                    {"pkg": "tester", "dep_kinds": [{"kind": "dev"}]}
                ]},
                {"id": "serde", "deps": [
                    {"pkg": "serde_derive", "dep_kinds": [{"kind": null}]}
                ]},
                {"id": "serde_derive", "deps": []},
                {"id": "tester", "deps": []}
            ]}
        });
        // app: lib + bin + build script; serde; serde_derive.
        assert_eq!(count_artifacts(&metadata), Some(5));
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
