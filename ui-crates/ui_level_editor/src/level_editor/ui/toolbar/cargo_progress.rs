//! Real-time cargo build progress via JSON message parsing.
//!
//! ## How progress is tracked
//!
//! Cargo's `--message-format=json` flag splits output:
//!   stdout → machine-readable JSON (what we parse)
//!   stderr → human-readable `Compiling …` lines (visible in the editor terminal)
//!
//! We use `--unit-graph` (stable since 1.65) for a fast pre-flight that returns
//! the exact set of compilation units cargo will process for this invocation.
//! That count becomes the denominator so the bar reflects real work.
//!
//! If `--unit-graph` is unavailable or fails we fall back to counting artifacts
//! as they arrive and scaling dynamically: each new artifact extends the
//! "expected total" slightly, so the bar always reaches 100 % at `build-finished`
//! without stalling or jumping.

use std::io::{BufRead as _, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

/// Run `cargo build --release` in `project_root` and update `progress` (0–100).
///
/// Returns `Ok(())` on success or an error string on failure.  Human-readable
/// cargo output (stderr) is forwarded to the process so it appears in the
/// editor terminal; the JSON stream on stdout is consumed internally.
pub fn run_cargo_build(
    project_root: &Path,
    progress: Arc<AtomicU32>,
) -> Result<(), String> {
    // Fast pre-flight: ask cargo exactly how many units it will compile.
    // This is the same invocation as the real build minus `--message-format`.
    let total = unit_graph_count_for(project_root, &["build", "--release"])
        .unwrap_or(0)
        .max(1) as f32;

    let mut child = Command::new("cargo")
        .args(["build", "--release", "--message-format=json"])
        .current_dir(project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit()) // human lines stay visible in the terminal
        .spawn()
        .map_err(|e| format!("Failed to spawn cargo build: {e}"))?;

    let stdout = BufReader::new(
        child
            .stdout
            .take()
            .ok_or_else(|| "Failed to capture cargo stdout".to_string())?,
    );

    let mut seen: u32 = 0;
    // Rolling total used when unit-graph count was unavailable (fallback mode).
    // In fallback mode we keep the denominator slightly ahead of seen so the
    // bar never reaches 100 % prematurely — `build-finished` sets it to 100.
    let mut rolling_total = total;

    for line in stdout.lines() {
        let Ok(line) = line else { continue };

        if !line.contains(r#""reason""#) {
            continue;
        }

        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };

        match msg["reason"].as_str() {
            Some("compiler-artifact") => {
                seen += 1;

                // In fallback mode (total == 1 placeholder), grow the denominator
                // so the bar stays believable without stalling.
                if rolling_total < seen as f32 + 2.0 {
                    rolling_total = seen as f32 + 2.0;
                }

                // Reserve 5 % for the linker — it produces no JSON events.
                let pct = ((seen as f32 / rolling_total) * 95.0).min(94.0) as u32;
                progress.store(pct, Ordering::Relaxed);
            }

            Some("build-finished") => {
                // The JSON stream is done.  Exit status determines success/failure.
                // Don't touch `progress` here — let the caller set 100 on success.
            }

            _ => {}
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("Failed to wait for cargo: {e}"))?;

    if status.success() {
        progress.store(100, Ordering::Relaxed);
        Ok(())
    } else {
        Err("cargo build failed — check the editor output for details".into())
    }
}

/// Run `cargo check` in `project_root` with JSON progress tracking.
pub fn run_cargo_check(
    project_root: &Path,
    progress: Arc<AtomicU32>,
) -> Result<(), String> {
    let total = unit_graph_count_for(project_root, &["check"])
        .unwrap_or(0)
        .max(1) as f32;

    let mut child = Command::new("cargo")
        .args(["check", "--message-format=json"])
        .current_dir(project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Failed to spawn cargo check: {e}"))?;

    let stdout = BufReader::new(
        child
            .stdout
            .take()
            .ok_or_else(|| "Failed to capture cargo stdout".to_string())?,
    );

    let mut seen: u32 = 0;
    let mut rolling_total = total;

    for line in stdout.lines() {
        let Ok(line) = line else { continue };
        if !line.contains(r#""reason""#) { continue }
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if matches!(msg["reason"].as_str(), Some("compiler-artifact")) {
            seen += 1;
            if rolling_total < seen as f32 + 2.0 { rolling_total = seen as f32 + 2.0; }
            let pct = ((seen as f32 / rolling_total) * 95.0).min(94.0) as u32;
            progress.store(pct, Ordering::Relaxed);
        }
    }

    let status = child.wait().map_err(|e| format!("Failed to wait for cargo: {e}"))?;
    if status.success() {
        progress.store(100, Ordering::Relaxed);
        Ok(())
    } else {
        Err("cargo check failed — check the editor output for details".into())
    }
}

/// Ask cargo how many compilation units a given subcommand will touch, without
/// actually running it.  Returns `None` if the flag is unsupported or the
/// project hasn't been fetched yet.
///
/// `subcommand` is e.g. `&["build", "--release"]` or `&["check"]`.
fn unit_graph_count_for(project_root: &Path, subcommand: &[&str]) -> Option<usize> {
    let mut args: Vec<&str> = subcommand.to_vec();
    args.extend_from_slice(&["--unit-graph", "-Z", "unstable-options"]);

    let out = Command::new("cargo")
        .args(&args)
        .current_dir(project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !out.status.success() {
        // Stable toolchain without -Z unstable-options — use fallback.
        return None;
    }

    let graph: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    // `units` array: one entry per compilation unit cargo will process.
    graph["units"].as_array().map(|a| a.len())
}
