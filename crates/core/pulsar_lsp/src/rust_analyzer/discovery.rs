//! Locating (or installing) the rust-analyzer binary: PATH, rustup, the
//! engine deps directory and a GitHub release download. Entry point:
//! `RustAnalyzerManager::find_or_use_bundled_analyzer`.
use anyhow::{anyhow, Result};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use super::RustAnalyzerManager;

impl RustAnalyzerManager {
    /// Validate that a path is a safe location for a rust-analyzer binary.
    /// Returns true if the path is in a trusted directory or has an expected filename.
    // Real machine: locates and installs the rust-analyzer binary (PATH, ~/.cargo/bin, the engine's deps dir), outside any project.
    #[allow(clippy::disallowed_methods)]
    pub(super) fn is_safe_analyzer_path(path: &PathBuf) -> bool {
        // Must be a file, not a directory.
        if !path.is_file() {
            return false;
        }
        // Filename must match expected names for rust-analyzer.
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if file_name != "rust-analyzer" && file_name != "rust-analyzer.exe" {
            tracing::warn!(
                "Custom rust-analyzer path has unexpected filename '{}' — expected 'rust-analyzer'",
                file_name
            );
            return false;
        }
        // Check that the parent directory is a trusted location.
        if let Some(parent) = path.parent() {
            let parent_str = parent.to_string_lossy().to_lowercase();
            // Allow: ~/.cargo/bin, /usr/bin, /usr/local/bin, or PATH entries.
            if parent_str.contains(".cargo\\bin") || parent_str.contains(".cargo/bin") {
                return true;
            }
            if parent_str == "/usr/bin" || parent_str == "/usr/local/bin" {
                return true;
            }
            // Resolve the full path via PATH lookup and check against PATH entries.
            if let Ok(paths) = std::env::var("PATH") {
                for p in std::env::split_paths(&paths) {
                    let canonical = std::fs::canonicalize(&path).ok();
                    let p_canonical = std::fs::canonicalize(p.join(file_name)).ok();
                    if canonical.is_some() && canonical == p_canonical {
                        return true;
                    }
                }
            }
            // Not in a known-safe directory — warn but still allow (user configured it).
            tracing::warn!(
                "rust-analyzer path '{:?}' is not in a standard location (not in PATH or ~/.cargo/bin)",
                path
            );
        }
        true // Allow by default (user explicitly set this path)
    }

    /// Find rust-analyzer in PATH or use bundled version
    pub(super) fn find_or_use_bundled_analyzer() -> PathBuf {
        if let Some(custom_path) = std::env::var_os("PULSAR_RUST_ANALYZER_PATH") {
            let custom_path = PathBuf::from(custom_path);
            if Self::verify_rust_analyzer_executable(&custom_path).is_ok()
                && Self::is_safe_analyzer_path(&custom_path)
            {
                tracing::debug!("✓ Using rust-analyzer from PULSAR_RUST_ANALYZER_PATH");
                return custom_path;
            }
            tracing::warn!(
                "⚠️  PULSAR_RUST_ANALYZER_PATH is set but invalid: {:?}",
                custom_path
            );
        }

        // First, try using rustup to get the component path (handles rustup proxies)
        if let Some(rustup_path) = Self::find_rust_analyzer_via_rustup() {
            return rustup_path;
        }

        // Try command-based discovery in PATH.
        for candidate in Self::command_name_candidates() {
            let candidate_path = PathBuf::from(candidate);
            if let Ok(version) = Self::verify_rust_analyzer_executable(&candidate_path) {
                tracing::debug!("✓ Found system rust-analyzer via command: {}", candidate);
                tracing::debug!("   Version: {}", version);
                return candidate_path;
            }
        }

        // Check common absolute locations used when apps are launched outside shell PATH.
        for candidate in Self::absolute_rust_analyzer_candidates() {
            if let Ok(version) = Self::verify_rust_analyzer_executable(&candidate) {
                tracing::debug!("✓ Found rust-analyzer at: {:?}", candidate);
                tracing::debug!("   Version: {}", version);
                return candidate;
            }
        }

        // Check engine deps directory.
        let deps_path = Self::get_engine_deps_analyzer_path();
        if let Ok(version) = Self::verify_rust_analyzer_executable(&deps_path) {
            tracing::debug!("✓ Found rust-analyzer in engine deps: {:?}", deps_path);
            tracing::debug!("   Version: {}", version);
            return deps_path;
        }

        // Fallback command (may be installed later in runtime).
        tracing::debug!("⚠️  rust-analyzer not found in standard locations");
        tracing::debug!("   Will attempt to use rust-analyzer from PATH at runtime");
        PathBuf::from("rust-analyzer")
    }

    pub(super) fn command_name_candidates() -> Vec<&'static str> {
        if cfg!(windows) {
            vec!["rust-analyzer.exe", "rust-analyzer"]
        } else {
            vec!["rust-analyzer"]
        }
    }

    pub(super) fn absolute_rust_analyzer_candidates() -> Vec<PathBuf> {
        let exe_name = if cfg!(windows) {
            "rust-analyzer.exe"
        } else {
            "rust-analyzer"
        };

        let mut candidates = Vec::new();

        if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
            candidates.push(PathBuf::from(cargo_home).join("bin").join(exe_name));
        }

        if let Ok(home) = std::env::var("HOME") {
            candidates.push(
                PathBuf::from(home)
                    .join(".cargo")
                    .join("bin")
                    .join(exe_name),
            );
        }

        if let Ok(user_profile) = std::env::var("USERPROFILE") {
            candidates.push(
                PathBuf::from(user_profile)
                    .join(".cargo")
                    .join("bin")
                    .join(exe_name),
            );
        }

        candidates
    }

    pub(super) fn rustup_candidates() -> Vec<PathBuf> {
        let rustup_name = if cfg!(windows) {
            "rustup.exe"
        } else {
            "rustup"
        };

        let mut candidates = vec![PathBuf::from(rustup_name)];

        if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
            candidates.push(PathBuf::from(cargo_home).join("bin").join(rustup_name));
        }

        if let Ok(home) = std::env::var("HOME") {
            candidates.push(
                PathBuf::from(home)
                    .join(".cargo")
                    .join("bin")
                    .join(rustup_name),
            );
        }

        if let Ok(user_profile) = std::env::var("USERPROFILE") {
            candidates.push(
                PathBuf::from(user_profile)
                    .join(".cargo")
                    .join("bin")
                    .join(rustup_name),
            );
        }

        let mut seen = HashSet::new();
        candidates
            .into_iter()
            .filter(|path| seen.insert(path.clone()))
            .collect()
    }

    pub(super) fn output_text(output: &std::process::Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    pub(super) fn looks_like_missing_rustup_component(message: &str) -> bool {
        let msg = message.to_lowercase();
        msg.contains("unknown binary")
            || msg.contains("not installed for the toolchain")
            || msg.contains("component 'rust-analyzer' is not installed")
            || msg.contains("did you mean 'rustup component add rust-analyzer'")
    }

    pub(super) fn verify_rust_analyzer_executable(path: &PathBuf) -> Result<String> {
        let output = Command::new(path)
            .arg("--version")
            .output()
            .map_err(|e| anyhow!("failed to execute {:?}: {}", path, e))?;

        let message = Self::output_text(&output);
        if !output.status.success() {
            if Self::looks_like_missing_rustup_component(&message) {
                return Err(anyhow!(
                    "rustup proxy detected without rust-analyzer component"
                ));
            }
            return Err(anyhow!(
                "binary {:?} did not execute correctly: {}",
                path,
                message.trim()
            ));
        }

        if Self::looks_like_missing_rustup_component(&message) {
            return Err(anyhow!(
                "rustup proxy detected without rust-analyzer component"
            ));
        }

        let version = message
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        if version.is_empty() {
            return Err(anyhow!("could not determine rust-analyzer version"));
        }

        Ok(version)
    }

    /// Try to find rust-analyzer via rustup (handles rustup proxy wrappers)
    pub(super) fn find_rust_analyzer_via_rustup() -> Option<PathBuf> {
        for rustup_cmd in Self::rustup_candidates() {
            if let Ok(output) = Command::new(&rustup_cmd)
                .args(["which", "rust-analyzer"])
                .output()
            {
                if output.status.success() {
                    let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !path_str.is_empty() {
                        let path = PathBuf::from(path_str);
                        if let Ok(version) = Self::verify_rust_analyzer_executable(&path) {
                            tracing::debug!(
                                "✓ Found rust-analyzer via rustup ({:?}): {:?}",
                                rustup_cmd,
                                path
                            );
                            tracing::debug!("   Version: {}", version);
                            return Some(path);
                        }
                    }
                } else {
                    let message = Self::output_text(&output);
                    if Self::looks_like_missing_rustup_component(&message) {
                        tracing::debug!(
                            "ℹ️  rust-analyzer component is not installed for rustup at {:?}",
                            rustup_cmd
                        );
                    }
                }
            }
        }

        None
    }

    /// Get the path where we should install rust-analyzer in engine deps
    pub(super) fn get_engine_deps_analyzer_path() -> PathBuf {
        let exe_name = if cfg!(windows) {
            "rust-analyzer.exe"
        } else {
            "rust-analyzer"
        };

        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                return exe_dir.join("deps").join(exe_name);
            }
        }

        PathBuf::from("deps").join(exe_name)
    }

    /// Download and install rust-analyzer to the engine deps directory
    pub(super) fn install_rust_analyzer_to_deps() -> Result<PathBuf> {
        tracing::debug!("📦 Attempting to install rust-analyzer...");

        if let Ok(installed_path) = Self::install_rust_analyzer_via_rustup() {
            return Ok(installed_path);
        }

        tracing::debug!("   Rustup installation not available, trying manual download...");
        Self::download_rust_analyzer_binary()
    }

    /// Try to install rust-analyzer via rustup component
    pub(super) fn install_rust_analyzer_via_rustup() -> Result<PathBuf> {
        tracing::debug!("   Trying to install via rustup...");

        let mut errors = Vec::new();

        for rustup_cmd in Self::rustup_candidates() {
            let add_output = match Command::new(&rustup_cmd)
                .args(["component", "add", "rust-analyzer"])
                .output()
            {
                Ok(output) => output,
                Err(e) => {
                    errors.push(format!("{:?}: failed to run rustup: {}", rustup_cmd, e));
                    continue;
                }
            };

            if !add_output.status.success() {
                errors.push(format!(
                    "{:?}: component add failed: {}",
                    rustup_cmd,
                    Self::output_text(&add_output).trim()
                ));
                continue;
            }

            let which_output = match Command::new(&rustup_cmd)
                .args(["which", "rust-analyzer"])
                .output()
            {
                Ok(output) => output,
                Err(e) => {
                    errors.push(format!(
                        "{:?}: failed to locate rust-analyzer: {}",
                        rustup_cmd, e
                    ));
                    continue;
                }
            };

            if !which_output.status.success() {
                errors.push(format!(
                    "{:?}: rustup which failed: {}",
                    rustup_cmd,
                    Self::output_text(&which_output).trim()
                ));
                continue;
            }

            let path_str = String::from_utf8_lossy(&which_output.stdout)
                .trim()
                .to_string();
            let path = PathBuf::from(path_str);
            let version = match Self::verify_rust_analyzer_executable(&path) {
                Ok(version) => version,
                Err(e) => {
                    errors.push(format!(
                        "{:?}: installed binary verification failed: {}",
                        rustup_cmd, e
                    ));
                    continue;
                }
            };

            tracing::debug!("✓ rust-analyzer installed and verified via rustup!");
            tracing::debug!("   Path: {:?}", path);
            tracing::debug!("   Version: {}", version);
            return Ok(path);
        }

        Err(anyhow!(
            "Rustup installation failed for all candidates: {}",
            errors.join(" | ")
        ))
    }

    pub(super) fn release_asset_name() -> Result<String> {
        let target = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("windows", "x86_64") => "x86_64-pc-windows-msvc.exe",
            ("windows", "aarch64") => "aarch64-pc-windows-msvc.exe",
            ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
            ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
            ("macos", "x86_64") => "x86_64-apple-darwin",
            ("macos", "aarch64") => "aarch64-apple-darwin",
            (os, arch) => {
                return Err(anyhow!(
                    "Unsupported platform for automatic rust-analyzer installation: {}/{}",
                    os,
                    arch
                ))
            }
        };

        Ok(format!("rust-analyzer-{}", target))
    }

    pub(super) fn download_file(url: &str, dest: &PathBuf) -> Result<()> {
        if cfg!(windows) {
            let powershell_result = Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-Command",
                    &format!(
                        "Invoke-WebRequest -Uri '{}' -OutFile '{}'",
                        url,
                        dest.display()
                    ),
                ])
                .output();

            if let Ok(output) = powershell_result {
                if output.status.success() {
                    return Ok(());
                }
            }

            let curl_result = Command::new("curl")
                .args(["-fL", url, "-o", &dest.to_string_lossy()])
                .output()
                .map_err(|e| anyhow!("failed to run downloader: {}", e))?;

            if curl_result.status.success() {
                return Ok(());
            }

            return Err(anyhow!(
                "download failed: {}",
                Self::output_text(&curl_result).trim()
            ));
        }

        let curl_result = Command::new("curl")
            .args(["-fL", url, "-o", &dest.to_string_lossy()])
            .output();
        if let Ok(output) = curl_result {
            if output.status.success() {
                return Ok(());
            }
        }

        let wget_result = Command::new("wget")
            .args(["-O", &dest.to_string_lossy(), url])
            .output()
            .map_err(|e| anyhow!("failed to run downloader: {}", e))?;

        if wget_result.status.success() {
            return Ok(());
        }

        Err(anyhow!(
            "download failed: {}",
            Self::output_text(&wget_result).trim()
        ))
    }

    /// Download rust-analyzer binary directly from GitHub
    // Real machine: locates and installs the rust-analyzer binary (PATH, ~/.cargo/bin, the engine's deps dir), outside any project.
    #[allow(clippy::disallowed_methods)]
    pub(super) fn download_rust_analyzer_binary() -> Result<PathBuf> {
        tracing::debug!("   Downloading rust-analyzer to engine deps directory...");

        let deps_path = Self::get_engine_deps_analyzer_path();
        let deps_dir = deps_path
            .parent()
            .ok_or_else(|| anyhow!("Invalid deps path"))?;

        fs::create_dir_all(deps_dir)?;
        tracing::debug!("   Created deps directory: {:?}", deps_dir);

        let asset_name = Self::release_asset_name()?;
        let url = format!(
            "https://github.com/rust-lang/rust-analyzer/releases/latest/download/{}",
            asset_name
        );

        tracing::debug!("   Downloading from: {}", url);

        Self::download_file(&url, &deps_path)?;
        tracing::debug!("✓ Downloaded rust-analyzer successfully");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&deps_path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&deps_path, perms)?;
            tracing::debug!("✓ Made rust-analyzer executable");
        }

        let version = Self::verify_rust_analyzer_executable(&deps_path)
            .map_err(|e| anyhow!("Downloaded rust-analyzer but verification failed: {}", e))?;

        tracing::debug!("✓ rust-analyzer installed successfully!");
        tracing::debug!("   Version: {}", version);
        Ok(deps_path)
    }
}
