use std::io::{BufRead as _, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

/// Shared live status — current crate/repo name being processed.
pub type StatusCell = Arc<parking_lot::Mutex<String>>;

/// Run `cargo build --release`, occupying the full 0–100 % range.
pub fn run_cargo_build(
    project_root: &Path,
    progress: Arc<AtomicU32>,
    status: StatusCell,
) -> Result<(), String> {
    run_cargo(project_root, &["build", "--release"], progress, status, 0)
}

/// Run `cargo build --release` starting at `from_pct` (used after a clean phase).
pub fn run_cargo_build_from(
    project_root: &Path,
    progress: Arc<AtomicU32>,
    status: StatusCell,
    from_pct: u32,
) -> Result<(), String> {
    run_cargo(
        project_root,
        &["build", "--release"],
        progress,
        status,
        from_pct,
    )
}

/// Run `cargo check`, occupying the full 0–100 % range.
pub fn run_cargo_check(
    project_root: &Path,
    progress: Arc<AtomicU32>,
    status: StatusCell,
) -> Result<(), String> {
    run_cargo(project_root, &["check"], progress, status, 0)
}

/// Run `cargo check` starting at `from_pct` (used after a clean phase).
pub fn run_cargo_check_from(
    project_root: &Path,
    progress: Arc<AtomicU32>,
    status: StatusCell,
    from_pct: u32,
) -> Result<(), String> {
    run_cargo(project_root, &["check"], progress, status, from_pct)
}

/// Run `cargo clean`. Progress goes from 0 → `up_to_pct` (typically 20).
/// Status is set to "Cleaning…" then cleared on completion.
pub fn run_cargo_clean(
    project_root: &Path,
    progress: Arc<AtomicU32>,
    status: StatusCell,
    up_to_pct: u32,
) -> Result<(), String> {
    tracing::info!(
        "[CARGO-PROGRESS] spawning: cargo clean in {}",
        project_root.display()
    );
    *status.lock() = "Cleaning build artifacts…".to_string();
    progress.store(2, Ordering::Relaxed);

    let mut child = Command::new("cargo")
        .arg("clean")
        .current_dir(project_root)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn cargo clean: {e}"))?;

    let stderr = BufReader::new(child.stderr.take().unwrap());
    for line in stderr.lines() {
        let Ok(line) = line else { break };
        eprintln!("{line}");
        tracing::debug!("[CARGO-PROGRESS] clean: {line}");
    }

    let exit = child
        .wait()
        .map_err(|e| format!("Failed to wait for cargo clean: {e}"))?;
    if exit.success() {
        progress.store(up_to_pct, Ordering::Relaxed);
        *status.lock() = String::new();
        tracing::info!("[CARGO-PROGRESS] clean done → {up_to_pct}%");
        Ok(())
    } else {
        Err("cargo clean failed — check the editor output for details".into())
    }
}

/// Run `cargo update`, occupying the full 0–100 % range.
pub fn run_cargo_update(
    project_root: &Path,
    progress: Arc<AtomicU32>,
    status: StatusCell,
) -> Result<(), String> {
    run_cargo_update_to(project_root, progress, status, 100)
}

/// Run `cargo update` and drive progress via stderr "Updating …" lines,
/// scaling the 0–100 internal range down to `[0, up_to_pct]` (used when
/// chained before a build phase that occupies the remainder).
/// cargo update has no JSON format — all output is human-readable on stderr.
pub fn run_cargo_update_to(
    project_root: &Path,
    progress: Arc<AtomicU32>,
    status: StatusCell,
    up_to_pct: u32,
) -> Result<(), String> {
    tracing::info!(
        "[CARGO-PROGRESS] spawning: cargo update in {}",
        project_root.display()
    );

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
            let internal = (updates_seen * 3).min(95);
            let pct = internal * up_to_pct / 100;
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

    let exit = child
        .wait()
        .map_err(|e| format!("Failed to wait for cargo update: {e}"))?;
    if exit.success() {
        progress.store(up_to_pct, Ordering::Relaxed);
        *status.lock() = String::new();
        tracing::info!("[CARGO-PROGRESS] update success → {up_to_pct}%");
        Ok(())
    } else {
        Err("cargo update failed — check the editor output for details".into())
    }
}

/// Scale an internal 0–100 value into the [from_pct, 100] range.
fn scale_pct(internal: u32, from_pct: u32) -> u32 {
    from_pct + (internal * (100 - from_pct) / 100)
}

fn run_cargo(
    project_root: &Path,
    subcommand: &[&str],
    progress: Arc<AtomicU32>,
    status: StatusCell,
    from_pct: u32,
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

    // ── stderr thread: parse "Updating …" lines → from_pct..(from_pct+10) ────
    let progress_stderr = Arc::clone(&progress);
    let status_stderr = Arc::clone(&status);
    let stderr_thread = std::thread::spawn(move || {
        let mut updates_seen: u32 = 0;
        for line in stderr.lines() {
            let Ok(line) = line else { break };
            eprintln!("{line}");
            let trimmed = line.trim_start();
            if trimmed.starts_with("Updating ") || trimmed.starts_with("Locking ") {
                updates_seen += 1;
                // Internal 0–9 % stderr range, then scaled to [from_pct, from_pct+10].
                let internal = (updates_seen * 2).min(9);
                let pct = scale_pct(internal, from_pct).min(from_pct + 10);
                progress_stderr.store(pct, Ordering::Relaxed);

                let what = trimmed
                    .trim_start_matches("Updating ")
                    .trim_start_matches("Locking ")
                    .trim_matches('`');
                let short = what.split('/').last().unwrap_or(what);
                *status_stderr.lock() = format!("Updating {short}");
                tracing::info!("[CARGO-PROGRESS] stderr: {trimmed} → {pct}%");
            }
        }
    });

    // ── stdout: parse JSON compiler-artifact lines → (from_pct+10)..95 % ────
    // Artifacts occupy the band from (from_pct + 10) up to 95, leaving 5 % for
    // the linker step which produces no JSON events.
    let artifact_start = from_pct + 10;
    let mut seen: u32 = 0;
    let mut rolling_total: f32 = 4.0;

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
                if rolling_total < seen as f32 + 4.0 {
                    rolling_total = seen as f32 + 4.0;
                }
                // Map 0-100% internal → artifact_start..94 externally.
                let frac = (seen as f32 / rolling_total) * (94 - artifact_start) as f32;
                let pct = (artifact_start + frac as u32).min(94);
                progress.store(pct, Ordering::Relaxed);

                let name = msg["target"]["name"].as_str().unwrap_or("?");
                let fresh = msg["fresh"].as_bool().unwrap_or(false);
                let kind = msg["target"]["kind"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    .unwrap_or("lib");
                if !fresh {
                    *status.lock() = format!("Compiling {name}");
                }
                tracing::info!("[CARGO-PROGRESS] #{seen} {name} ({kind}) fresh={fresh} → {pct}%");
            }
            Some("build-finished") => {
                tracing::info!("[CARGO-PROGRESS] build-finished seen={seen}");
            }
            _ => {}
        }
    }

    let _ = stderr_thread.join();
    tracing::info!("[CARGO-PROGRESS] stdout closed seen={seen}");

    let status_val = child
        .wait()
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
