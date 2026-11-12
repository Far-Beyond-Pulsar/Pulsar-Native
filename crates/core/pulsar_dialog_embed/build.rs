//! Builds the `pulsar-dialog` helper for the target and leaves its bytes in
//! `OUT_DIR` for the library to embed.
//!
//! The helper lives in its own repository (Pulsar-Dialog). This script fetches
//! it at the pinned revision into a cache under `CARGO_HOME`, builds it in
//! release for the target, and copies the executable out. The cache is shared
//! by every checkout and survives `cargo clean`, so the helper is built once
//! per revision and target.
//!
//! Building the helper must never break building the engine: when it cannot be
//! built (no git, no network, a skipped build) the script warns and embeds
//! nothing, and the engine falls back to the platform's own message boxes.
//!
//! Knobs, all optional:
//!   PULSAR_SKIP_DIALOG_HELPER=1   embed nothing (lint and check jobs)
//!   PULSAR_DIALOG_HELPER_EXE=...  embed this executable instead of building one
//!   PULSAR_DIALOG_REV / _REPO     another revision or repository (development)

// Build script: it fetches and builds into a cache on the real disk, outside any project.
#![allow(clippy::disallowed_methods)]

use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The Pulsar-Dialog revision the engine ships. Bump it to ship a newer helper.
const DIALOG_REV: &str = "85544da6ff4e4e31a65524782a9e9916a07ca63e";
const DIALOG_REPO: &str = "https://github.com/Far-Beyond-Pulsar/Pulsar-Dialog";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    for variable in [
        "PULSAR_SKIP_DIALOG_HELPER",
        "PULSAR_DIALOG_HELPER_EXE",
        "PULSAR_DIALOG_REV",
        "PULSAR_DIALOG_REPO",
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
    }

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let bytes = match produce() {
        Ok(Some(exe)) => fs::read(&exe).unwrap_or_else(|error| {
            println!("cargo:warning=pulsar-dialog helper not embedded: {}: {error}", exe.display());
            Vec::new()
        }),
        Ok(None) => Vec::new(),
        Err(error) => {
            println!("cargo:warning=pulsar-dialog helper not embedded: {error}");
            Vec::new()
        }
    };
    let sha256 = if bytes.is_empty() {
        String::new()
    } else {
        hex(&Sha256::digest(&bytes))
    };
    fs::write(out.join("pulsar-dialog.bin"), &bytes).expect("write the embedded helper");
    fs::write(
        out.join("embedded.rs"),
        format!("pub const SHA256: &str = \"{sha256}\";\n"),
    )
    .expect("write embedded.rs");
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn set(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty() && value != "0")
}

/// The helper executable to embed, if there is one.
fn produce() -> Result<Option<PathBuf>, String> {
    if set("PULSAR_SKIP_DIALOG_HELPER").is_some() {
        return Ok(None);
    }
    if let Some(path) = env::var_os("PULSAR_DIALOG_HELPER_EXE").filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        println!("cargo:rerun-if-changed={}", path.display());
        return if path.is_file() {
            Ok(Some(path))
        } else {
            Err(format!(
                "PULSAR_DIALOG_HELPER_EXE names {}, which is not a file",
                path.display()
            ))
        };
    }

    let target = env::var("TARGET").map_err(|_| "cargo did not set TARGET".to_string())?;
    let rev = env::var("PULSAR_DIALOG_REV").unwrap_or_else(|_| DIALOG_REV.to_string());
    let repo = env::var("PULSAR_DIALOG_REPO").unwrap_or_else(|_| DIALOG_REPO.to_string());
    let short = &rev[..rev.len().min(12)];
    let cache = cache_root()?;
    let source = cache.join(format!("src-{short}"));
    checkout(&source, &repo, &rev)?;
    build(&source, &cache.join(format!("target-{short}")), &target)
}

/// `CARGO_HOME/pulsar-dialog`: outside every checkout, so the cargo config of
/// the repository being built does not reach the helper's build.
fn cache_root() -> Result<PathBuf, String> {
    let home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("USERPROFILE")
                .or_else(|| env::var_os("HOME"))
                .map(|home| PathBuf::from(home).join(".cargo"))
        })
        .ok_or("no CARGO_HOME or home directory for the helper's build cache")?;
    let cache = home.join("pulsar-dialog");
    fs::create_dir_all(&cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    Ok(cache)
}

fn run(command: &mut Command, what: &str) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|e| format!("{what}: could not run ({e})"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tail: Vec<&str> = stderr.lines().rev().take(12).collect();
    Err(format!(
        "{what} failed: {}",
        tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
    ))
}

fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir);
    command
}

/// The repository at exactly `rev` in `source`.
fn checkout(source: &Path, repo: &str, rev: &str) -> Result<(), String> {
    if source.join(".git").is_dir() {
        let head = run(git(source).args(["rev-parse", "HEAD"]), "git rev-parse");
        if head.is_ok_and(|head| head == rev) {
            return Ok(());
        }
        let _ = fs::remove_dir_all(source);
    }
    // Fetch into a private directory and move it into place, so two builds at
    // once (or an interrupted one) never leave a half checkout under the real name.
    let staging = source.with_extension(format!("staging-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let fetched = run(Command::new("git").arg("init").arg("-q").arg(&staging), "git init")
        .and_then(|_| run(git(&staging).args(["remote", "add", "origin", repo]), "git remote add"))
        .and_then(|_| {
            run(
                git(&staging).args(["fetch", "-q", "--depth", "1", "origin", rev]),
                "git fetch",
            )
        })
        .and_then(|_| run(git(&staging).args(["checkout", "-q", "FETCH_HEAD"]), "git checkout"));
    if let Err(error) = fetched {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!("{error} (is git installed and {repo} reachable?)"));
    }
    if fs::rename(&staging, source).is_err() {
        // Another build got there first.
        let _ = fs::remove_dir_all(&staging);
        if !source.join(".git").is_dir() {
            return Err(format!("could not place the checkout at {}", source.display()));
        }
    }
    Ok(())
}

/// Build the helper in release for `target`; the path of the executable.
fn build(source: &Path, target_dir: &Path, target: &str) -> Result<Option<PathBuf>, String> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(source)
        .args(["build", "--release", "--locked", "-p", "pulsar-dialog", "--target", target])
        .arg("--target-dir")
        .arg(target_dir);
    // The parent build's flags and wrappers belong to the engine, not the
    // helper: its linker flags, `-A warnings`, a clippy wrapper.
    for variable in [
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTDOC",
    ] {
        command.env_remove(variable);
    }
    run(&mut command, "building the pulsar-dialog helper")?;
    let exe = target_dir.join(target).join("release").join(if target.contains("windows") {
        "pulsar-dialog.exe"
    } else {
        "pulsar-dialog"
    });
    if exe.is_file() {
        Ok(Some(exe))
    } else {
        Err(format!("the build left no executable at {}", exe.display()))
    }
}
