//! The pack step: cook the project and pack it into the pak set in the
//! `Content/` folder next to where the next step's build puts the game
//! binary, so the binary runs its packed content (`cargo run` and the built
//! executable find `Content/` beside them).
//!
//! The packing itself is the host's (`pulsar_content::ContentPacker`, which
//! the editor binary links); its log lines are written in the build's task
//! and show in the build dashboard.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use engine_state::build_config::{BuildProfile, TargetPlatform};
use rust_i18n::t;

use super::cargo::{Progress, StepError};

pub fn run(
    platform: Option<TargetPlatform>,
    profile: BuildProfile,
    project_root: &Path,
    progress: &Progress,
    cancel: &Arc<AtomicBool>,
) -> Result<(), StepError> {
    let packer = pulsar_content::packer()
        .ok_or_else(|| StepError::Failed(t!("Build.Error.NoPacker").to_string()))?;
    progress.status(t!("Build.Progress.FindingOutput"));
    let content_dir = content_dir(project_root, platform, profile).map_err(StepError::Failed)?;
    tracing::info!(target: "pack", "Packing content into {}", content_dir.display());

    let report = |fraction: f32, status: String| {
        progress.set((fraction.clamp(0.0, 1.0) * 100.0) as u32);
        if !status.is_empty() {
            progress.status(status);
        }
    };
    let cancelled = || cancel.load(Ordering::Acquire);
    let job = pulsar_content::PackJob {
        project_root,
        content_dir: &content_dir,
        shipping: profile == BuildProfile::Shipping,
        progress: &report,
        cancelled: &cancelled,
    };
    match (packer.pack)(&job) {
        Ok(summary) => {
            tracing::info!(target: "pack", "Content packed: {summary}");
            progress.set(100);
            Ok(())
        }
        Err(_) if cancelled() => Err(StepError::Cancelled),
        Err(error) => {
            let error = with_library_hint(&error, &plugin_manager::load_report());
            tracing::error!(target: "pack", "{error}");
            Err(StepError::Failed(
                t!("Build.Error.PackFailed", detail = error).to_string(),
            ))
        }
    }
}

/// Whether a pack failure is the signature of plugin libraries that are not the same build: a
/// class compiled against an event or a native function that no loaded library declared.
fn is_missing_registration(error: &str) -> bool {
    error.contains("is declared by the module or registered with the engine")
        || error.contains("no native function")
        || error.contains("is gone")
        || error.contains("is not registered")
}

/// `error` with, when it looks like a missing registration, what the editor knows about the
/// plugin libraries: where they were loaded from, which did not load and why, and how old
/// each is, so the cause (a library older than the class that calls it, or one that never
/// loaded) is named instead of left to be guessed.
fn with_library_hint(error: &str, report: &plugin_manager::LoadReport) -> String {
    if !is_missing_registration(error) {
        return error.to_owned();
    }
    use plugin_manager::LibraryOutcome;
    let mut hint = String::from(
        "\n\nLikely cause: a plugin library the editor loaded does not register what this project's class uses. \
         A class and the library that registers its events or functions are built separately, so a library \
         older than the class (or one that failed to load) leaves the lookup empty. Rebuild every plugin \
         with `cargo run -p plugin_builder -- --release`, then restart the editor.",
    );
    // An editor that links its plugins (`--features link-plugins`) has no library of them to be
    // stale: what is stale is the editor itself.
    let linked = plugin_manager::linked_plugins().len();
    if linked > 1 {
        hint.push_str(&format!(
            "\nThis editor links {linked} plugins into its executable (`link-plugins`); a plugin it links \
             is as old as the editor, so rebuild the editor (`just build-linked`) and restart it."
        ));
    }
    match &report.directory {
        Some(directory) => hint.push_str(&format!("\nPlugin libraries were loaded from {}.", directory.display())),
        None => hint.push_str("\nNo plugin library directory was read."),
    }
    let mut failed = Vec::new();
    let mut newest: Option<std::time::SystemTime> = None;
    for library in &report.libraries {
        if let Ok(modified) = std::fs::metadata(&library.path).and_then(|m| m.modified()) {
            newest = Some(newest.map_or(modified, |n| n.max(modified)));
        }
        if let LibraryOutcome::Failed { stage, reason } = &library.outcome {
            failed.push(format!("  {} ({stage}): {reason}", library.file));
        }
    }
    if !failed.is_empty() {
        hint.push_str(&format!("\nLibraries that did not load ({}):\n{}", failed.len(), failed.join("\n")));
    }
    // A library much older than the newest is the likely stale one.
    if let Some(newest) = newest {
        let stale: Vec<String> = report
            .libraries
            .iter()
            .filter_map(|l| {
                let modified = std::fs::metadata(&l.path).and_then(|m| m.modified()).ok()?;
                let age = newest.duration_since(modified).ok()?;
                (age > std::time::Duration::from_secs(30 * 60)).then(|| {
                    format!("  {} is {} h older than the newest library", l.file, age.as_secs() / 3600)
                })
            })
            .collect();
        if !stale.is_empty() {
            hint.push_str(&format!("\nLibraries from an earlier build:\n{}", stale.join("\n")));
        }
    }
    format!("{error}{hint}")
}

#[cfg(test)]
mod hint_tests {
    use super::*;
    use plugin_manager::{LibraryOutcome, LibraryReport, LoadReport};

    #[test]
    fn a_missing_event_names_the_libraries() {
        let report = LoadReport {
            directory: Some("plugins/editor".into()),
            directory_exists: true,
            libraries: vec![LibraryReport {
                file: "input_plugin.dll".into(),
                path: "plugins/editor/input_plugin.dll".into(),
                outcome: LibraryOutcome::Failed { stage: "prepare", reason: "built against other shared libraries".into() },
                renamed_imports: vec![],
            }],
        };
        let error = "script class `PlayerCharacter`: no event `input::IA_Jump.triggered` is declared by the module or registered with the engine";
        let hinted = with_library_hint(error, &report);
        assert!(hinted.starts_with(error));
        assert!(hinted.contains("plugin_builder"));
        assert!(hinted.contains("plugins/editor"));
        assert!(hinted.contains("input_plugin.dll (prepare): built against other shared libraries"));
        // Other failures are left alone.
        assert_eq!(with_library_hint("disk full", &report), "disk full");
    }
}

/// `<target dir>[/<triple>]/<profile dir>/Content`: beside the binary the
/// build makes. The target directory is Cargo's own answer (it honours
/// `CARGO_TARGET_DIR` and `.cargo/config.toml`).
fn content_dir(
    project_root: &Path,
    platform: Option<TargetPlatform>,
    profile: BuildProfile,
) -> Result<PathBuf, String> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(project_root)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            t!(
                "Build.Error.CouldNotStart",
                subcommand = "metadata",
                error = e
            )
            .to_string()
        })?;
    if !output.status.success() {
        return Err(t!(
            "Build.Error.CommandFailed",
            subcommand = "metadata",
            detail = String::from_utf8_lossy(&output.stderr).trim()
        )
        .to_string());
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let target = metadata["target_directory"]
        .as_str()
        .ok_or("cargo metadata names no target directory")?;
    Ok(output_dir(Path::new(target), platform, profile).join(pulsar_content::CONTENT_DIR_NAME))
}

/// Where Cargo puts the binaries of `platform` in `profile`.
fn output_dir(target: &Path, platform: Option<TargetPlatform>, profile: BuildProfile) -> PathBuf {
    let mut dir = target.to_path_buf();
    if let Some(triple) = platform.and_then(TargetPlatform::triple) {
        dir.push(triple);
    }
    dir.push(match profile {
        BuildProfile::Debug => "debug",
        BuildProfile::Release | BuildProfile::Shipping => "release",
    });
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_goes_beside_the_binary_the_build_makes() {
        let target = Path::new("t");
        assert_eq!(
            output_dir(target, None, BuildProfile::Debug),
            Path::new("t/debug")
        );
        assert_eq!(
            output_dir(target, None, BuildProfile::Shipping),
            Path::new("t/release")
        );
        let linux = Some(TargetPlatform::LinuxX86_64Gnu);
        let triple = TargetPlatform::LinuxX86_64Gnu.triple().unwrap();
        assert_eq!(
            output_dir(target, linux, BuildProfile::Release),
            Path::new("t").join(triple).join("release")
        );
    }
}
