use std::io::{BufRead as _, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

/// Shared live status — current crate/repo name being processed.
pub type StatusCell = Arc<parking_lot::Mutex<String>>;

pub fn run_cargo_build(
    project_root: &Path,
    progress: Arc<AtomicU32>,
    status: StatusCell,
) -> Result<(), String> {
    run_cargo(project_root, &["build", "--release"], progress, status)
}

pub fn run_cargo_check(
    project_root: &Path,
    progress: Arc<AtomicU32>,
    status: StatusCell,
) -> Result<(), String> {
    run_cargo(project_root, &["check"], progress, status)
}

/// Run `cargo update` and drive progress via stderr "Updating …" lines.
/// cargo update has no JSON format — all output is human-readable on stderr.
pub fn run_cargo_update(
    project_root: &Path,
    progress: Arc<AtomicU32>,
    status: StatusCell,
) -> Result<(), String> {
    tracing::info!("[CARGO-PROGRESS] spawning: cargo update in {}", project_root.display());

    let mut child = Command::new("cargo")
        .arg("update")
        .current_dir(project_root)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn cargo update: {e}"))?;

    let stderr = BufReader::new(child.stderr.take().unwrap());
    let mut updates_seen: u32 = 0;

    for line in stderr.lines() {
        let Ok(line) = line else { break };
        eprintln!("{line}");

        let trimmed = line.trim_start();
        if trimmed.starts_with("Updating ") || trimmed.starts_with("Locking ") {
            updates_seen += 1;
            let pct = (updates_seen * 3).min(95) as u32;
            progress.store(pct, Ordering::Relaxed);

            let what = trimmed
                .trim_start_matches("Updating ")
                .trim_start_matches("Locking ")
                .trim_matches('`');
            let short = what.split('/').last().unwrap_or(what);
            *status.lock() = format!("Updating {short}");
            tracing::info!("[CARGO-PROGRESS] {trimmed} → {pct}%");
        }
    }

    let exit = child.wait().map_err(|e| format!("Failed to wait for cargo update: {e}"))?;
    if exit.success() {
        progress.store(100, Ordering::Relaxed);
        tracing::info!("[CARGO-PROGRESS] update success → 100%");
        Ok(())
    } else {
        Err("cargo update failed — check the editor output for details".into())
    }
}

fn run_cargo(
    project_root: &Path,
    subcommand: &[&str],
    progress: Arc<AtomicU32>,
    status: StatusCell,
) -> Result<(), String> {
    let mut args: Vec<&str> = subcommand.to_vec();
    args.push("--message-format=json");

    tracing::info!(
        "[CARGO-PROGRESS] spawning: cargo {} in {}",
        args.join(" "),
        project_root.display()
    );

    let mut child = Command::new("cargo")
        .args(&args)
        .current_dir(project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped()) // pipe stderr so we can parse "Updating" lines
        .spawn()
        .map_err(|e| format!("Failed to spawn cargo {}: {e}", subcommand[0]))?;

    let stdout = BufReader::new(child.stdout.take().unwrap());
    let stderr = BufReader::new(child.stderr.take().unwrap());

    // ── stderr thread: parse "Updating …" lines → 0–10 % ────────────────────
    let progress_stderr = Arc::clone(&progress);
    let status_stderr   = Arc::clone(&status);
    let stderr_thread = std::thread::spawn(move || {
        // Count unique repos/registries being updated so we can estimate 0–10 %.
        let mut updates_seen: u32 = 0;

        for line in stderr.lines() {
            let Ok(line) = line else { break };

            // Forward to terminal so the user can still see full cargo output.
            eprintln!("{line}");

            // "    Updating git repository `https://…`"
            // "    Updating crates.io index"
            // "    Locking N packages"
            let trimmed = line.trim_start();
            if trimmed.starts_with("Updating ") || trimmed.starts_with("Locking ") {
                updates_seen += 1;
                // Each update nudges us from 0 → max 9 %.
                let pct = (updates_seen * 2).min(9) as u32;
                progress_stderr.store(pct, Ordering::Relaxed);

                // Extract what's being updated for the status line.
                let what = trimmed
                    .trim_start_matches("Updating ")
                    .trim_start_matches("Locking ")
                    .trim_matches('`');
                // Keep only the meaningful part (strip long URLs).
                let short = what.split('/').last().unwrap_or(what);
                *status_stderr.lock() = format!("Updating {short}");

                tracing::info!("[CARGO-PROGRESS] stderr: {trimmed} → {}%", pct);
            }
        }
    });

    // ── stdout: parse JSON compiler-artifact lines → 10–95 % ────────────────
    let mut seen: u32 = 0;
    let mut rolling_total: f32 = 4.0;

    for line in stdout.lines() {
        let Ok(line) = line else { continue };
        if !line.contains(r#""reason""#) { continue }

        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else { continue };

        match msg["reason"].as_str() {
            Some("compiler-artifact") => {
                seen += 1;
                if rolling_total < seen as f32 + 4.0 {
                    rolling_total = seen as f32 + 4.0;
                }
                // Scale artifact progress over 10–95 %, leaving 5 % for linker.
                let artifact_frac = (seen as f32 / rolling_total) * 85.0; // 85 % of range
                let pct = (10 + artifact_frac as u32).min(94);
                progress.store(pct, Ordering::Relaxed);

                let name  = msg["target"]["name"].as_str().unwrap_or("?");
                let fresh = msg["fresh"].as_bool().unwrap_or(false);
                let kind  = msg["target"]["kind"].as_array()
                    .and_then(|a| a.first()).and_then(|v| v.as_str()).unwrap_or("lib");

                if !fresh {
                    *status.lock() = format!("Compiling {name}");
                }

                tracing::info!(
                    "[CARGO-PROGRESS] #{seen} {name} ({kind}) fresh={fresh} → {pct}%"
                );
            }
            Some("build-finished") => {
                tracing::info!("[CARGO-PROGRESS] build-finished seen={seen}");
            }
            _ => {}
        }
    }

    let _ = stderr_thread.join();

    tracing::info!("[CARGO-PROGRESS] stdout closed seen={seen}");

    let status_val = child.wait()
        .map_err(|e| format!("Failed to wait for cargo: {e}"))?;

    if status_val.success() {
        progress.store(100, Ordering::Relaxed);
        tracing::info!("[CARGO-PROGRESS] success → 100%");
        Ok(())
    } else {
        tracing::error!("[CARGO-PROGRESS] cargo exited non-zero");
        Err(format!(
            "cargo {} failed — check the editor output for details",
            subcommand[0]
        ))
    }
}
