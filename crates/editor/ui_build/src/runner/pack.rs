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
            tracing::error!(target: "pack", "{error}");
            Err(StepError::Failed(
                t!("Build.Error.PackFailed", detail = error).to_string(),
            ))
        }
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
