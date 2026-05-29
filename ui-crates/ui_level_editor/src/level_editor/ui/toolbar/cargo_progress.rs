//! Real-time cargo build progress via JSON message parsing.
//!
//! ## How it works
//!
//! 1. `cargo metadata --format-version=1` is run upfront (fast — reads the
//!    lockfile) to count the exact number of packages in the resolved dep graph.
//!    This is the same set cargo will emit `compiler-artifact` events for, so it
//!    is an accurate denominator regardless of whether the build is clean or
//!    incremental.
//!
//! 2. `cargo build --release --message-format=json` is run.  For every
//!    `compiler-artifact` event on stdout the atomic progress counter advances.
//!    Fresh (cached) artifacts are counted instantly; actually-compiled ones
//!    arrive after their compilation finishes — so the bar naturally accelerates
//!    through already-cached crates and slows down only where real work is done.
//!
//! 3. The final 5 % (95→100) is reserved for the linker step, which emits no
//!    JSON events but can take several seconds on large projects.

use std::io::{BufRead as _, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

/// Run `cargo build --release` in `project_root` and update `progress` (0–100).
///
/// Returns `Ok(())` on success or an error string on failure.  Stderr is
/// forwarded to the parent process so build errors appear in the editor terminal.
pub fn run_cargo_build(
    project_root: &Path,
    progress: Arc<AtomicU32>,
) -> Result<(), String> {
    // Accurate total: resolve.nodes covers every package cargo will touch.
    let total = resolve_package_count(project_root);

    let mut child = Command::new("cargo")
        .args(["build", "--release", "--message-format=json"])
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

    let mut seen: u32 = 0;

    for line in stdout.lines() {
        let Ok(line) = line else { continue };

        // Fast-path: skip lines that definitely aren't JSON reason objects.
        if !line.contains(r#""reason""#) {
            continue;
        }

        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };

        match msg["reason"].as_str() {
            Some("compiler-artifact") => {
                seen += 1;
                // Reserve 5 % for the linker — it emits no JSON events.
                let pct = ((seen as f32 / total) * 95.0).min(95.0) as u32;
                progress.store(pct, Ordering::Relaxed);
            }
            Some("build-finished") => {
                // The build is done; exit status will confirm success/failure.
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

/// Count every package in the fully-resolved dependency graph by parsing
/// `cargo metadata`.  This matches exactly what cargo will emit
/// `compiler-artifact` events for, giving an accurate denominator.
///
/// Falls back to a generous default if metadata is unavailable.
fn resolve_package_count(project_root: &Path) -> f32 {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version=1"])
        .current_dir(project_root)
        // Suppress noise; we only care about the JSON.
        .stderr(Stdio::null())
        .output();

    let count = output
        .ok()
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
        // `resolve.nodes` is the full transitive dep graph — one entry per package.
        .and_then(|v| v["resolve"]["nodes"].as_array().map(|a| a.len()))
        .unwrap_or(0);

    if count > 0 {
        count as f32
    } else {
        // Fallback: generous estimate so the bar at least moves.
        150.0
    }
}
