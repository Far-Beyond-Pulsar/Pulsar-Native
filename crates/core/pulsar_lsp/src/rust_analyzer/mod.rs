//! Rust Analyzer LSP process manager.
//!
//! Handles process lifecycle, the LSP JSON-RPC protocol loop, progress
//! reporting, and all `textDocument/*` request helpers.  The GPUI
//! `EventEmitter` implementation emits [`AnalyzerEvent`] so that UI
//! components can subscribe to indexing progress and diagnostics.

mod path_utils;
pub use path_utils::{path_to_uri, uri_to_path};

use anyhow::{anyhow, Result};
use gpui::{Context, EventEmitter, Window};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use ui::diagnostics::{CodeAction, Diagnostic, DiagnosticSeverity, TextEdit};

#[derive(Clone, Debug, PartialEq)]
pub enum AnalyzerStatus {
    Idle,
    Starting,
    Indexing { progress: f32, message: String },
    Ready,
    Error(String),
    Stopped,
}

#[derive(Clone, Debug)]
pub enum AnalyzerEvent {
    StatusChanged(AnalyzerStatus),
    IndexingProgress { progress: f32, message: String },
    Ready,
    Error(String),
    Diagnostics(Vec<Diagnostic>),
}

#[derive(Debug)]
enum ProgressUpdate {
    Progress { progress: f32, message: String },
    Ready,
    Error(String),
    ProcessExited(ExitStatus),
    Diagnostics(Vec<Diagnostic>),
}

pub struct RustAnalyzerManager {
    /// Path to rust-analyzer executable
    analyzer_path: PathBuf,
    /// Current workspace root
    workspace_root: Option<PathBuf>,
    /// LSP process handle (wrapped in Arc for thread safety)
    process: Arc<Mutex<Option<Child>>>,
    /// Process stdin handle (separate for thread safety)
    stdin: Arc<Mutex<Option<std::process::ChildStdin>>>,
    /// Current status
    status: AnalyzerStatus,
    /// Whether the manager is initialized
    initialized: bool,
    /// Last indexing update
    last_update: Option<Instant>,
    /// Number of requests sent
    request_id: Arc<Mutex<i64>>,
    /// Progress updates channel receiver
    progress_rx: Option<Receiver<ProgressUpdate>>,
    /// Pending request callbacks (using flume for async support)
    pending_requests: Arc<Mutex<HashMap<i64, flume::Sender<serde_json::Value>>>>,
    /// Whether we've attempted installation on failure
    install_attempted: bool,
    /// Time when we first received diagnostics (indicator that analysis is working)
    first_diagnostics_time: Option<Instant>,
    /// Whether initial analysis has been marked as complete
    initial_analysis_complete: bool,
}

impl EventEmitter<AnalyzerEvent> for RustAnalyzerManager {}

impl RustAnalyzerManager {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        let analyzer_path = Self::find_or_use_bundled_analyzer();

        tracing::debug!("🔧 Rust Analyzer Manager initialized");
        tracing::debug!("   Using: {:?}", analyzer_path);

        Self {
            analyzer_path,
            workspace_root: None,
            process: Arc::new(Mutex::new(None)),
            stdin: Arc::new(Mutex::new(None)),
            status: AnalyzerStatus::Idle,
            initialized: false,
            last_update: None,
            request_id: Arc::new(Mutex::new(0)),
            progress_rx: None,
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            install_attempted: false,
            first_diagnostics_time: None,
            initial_analysis_complete: false,
        }
    }

    /// Validate that a path is a safe location for a rust-analyzer binary.
    /// Returns true if the path is in a trusted directory or has an expected filename.
    fn is_safe_analyzer_path(path: &PathBuf) -> bool {
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
    fn find_or_use_bundled_analyzer() -> PathBuf {
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

    fn command_name_candidates() -> Vec<&'static str> {
        if cfg!(windows) {
            vec!["rust-analyzer.exe", "rust-analyzer"]
        } else {
            vec!["rust-analyzer"]
        }
    }

    fn absolute_rust_analyzer_candidates() -> Vec<PathBuf> {
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

    fn rustup_candidates() -> Vec<PathBuf> {
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

    fn output_text(output: &std::process::Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    fn looks_like_missing_rustup_component(message: &str) -> bool {
        let msg = message.to_lowercase();
        msg.contains("unknown binary")
            || msg.contains("not installed for the toolchain")
            || msg.contains("component 'rust-analyzer' is not installed")
            || msg.contains("did you mean 'rustup component add rust-analyzer'")
    }

    fn verify_rust_analyzer_executable(path: &PathBuf) -> Result<String> {
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
    fn find_rust_analyzer_via_rustup() -> Option<PathBuf> {
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
    fn get_engine_deps_analyzer_path() -> PathBuf {
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
    fn install_rust_analyzer_to_deps() -> Result<PathBuf> {
        tracing::debug!("📦 Attempting to install rust-analyzer...");

        if let Ok(installed_path) = Self::install_rust_analyzer_via_rustup() {
            return Ok(installed_path);
        }

        tracing::debug!("   Rustup installation not available, trying manual download...");
        Self::download_rust_analyzer_binary()
    }

    /// Try to install rust-analyzer via rustup component
    fn install_rust_analyzer_via_rustup() -> Result<PathBuf> {
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

    fn release_asset_name() -> Result<String> {
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

    fn download_file(url: &str, dest: &PathBuf) -> Result<()> {
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
    fn download_rust_analyzer_binary() -> Result<PathBuf> {
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

    /// Start rust-analyzer for the given workspace
    pub fn start(&mut self, workspace_root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        tracing::debug!("🚀 Starting rust-analyzer for: {:?}", workspace_root);

        self.workspace_root = Some(workspace_root.clone());
        self.status = AnalyzerStatus::Starting;
        self.install_attempted = false;
        self.first_diagnostics_time = None;
        self.initial_analysis_complete = false;
        cx.emit(AnalyzerEvent::StatusChanged(AnalyzerStatus::Starting));
        cx.notify();

        self.stop_internal();

        let (progress_tx, progress_rx) = channel();
        self.progress_rx = Some(progress_rx);

        let analyzer_path = self.analyzer_path.clone();
        let process_arc = self.process.clone();
        let stdin_arc = self.stdin.clone();
        let request_id_arc = self.request_id.clone();
        let pending_requests_arc = self.pending_requests.clone();

        cx.spawn_in(window, async move |manager, cx| {
            let workspace_root_for_spawn = workspace_root.clone();
            let progress_tx_for_spawn = progress_tx.clone();
            let process_arc_clone = process_arc.clone();
            let stdin_arc_clone = stdin_arc.clone();
            let pending_requests_clone = pending_requests_arc.clone();
            let spawn_result = std::thread::spawn(move || {
                Self::spawn_process_blocking(
                    &analyzer_path,
                    &workspace_root_for_spawn,
                    process_arc_clone,
                    stdin_arc_clone,
                    progress_tx_for_spawn,
                    pending_requests_clone,
                )
            })
            .join();

            match spawn_result {
                Ok(Ok(resolved_analyzer_path)) => {
                    tracing::debug!("✓ rust-analyzer process spawned successfully");
                    tracing::debug!("   Active binary: {:?}", resolved_analyzer_path);

                    let _ = manager.update(cx, |manager, _cx| {
                        manager.analyzer_path = resolved_analyzer_path;
                    });

                    let workspace_root_for_init = workspace_root.clone();
                    let stdin_arc_for_init = stdin_arc.clone();
                    let request_id_arc_for_init = request_id_arc.clone();
                    let progress_tx_for_init = progress_tx.clone();

                    std::thread::spawn(move || {
                        if let Err(e) = Self::send_initialize_request(
                            &workspace_root_for_init,
                            stdin_arc_for_init,
                            request_id_arc_for_init,
                        ) {
                            tracing::error!("❌ Failed to send initialize request: {}", e);
                            let _ = progress_tx_for_init
                                .send(ProgressUpdate::Error(format!("Init failed: {}", e)));
                        }
                    });

                    let _ = manager.update(cx, |manager, cx| {
                        manager.status = AnalyzerStatus::Indexing {
                            progress: 0.0,
                            message: "Initializing...".to_string(),
                        };
                        // DO NOT set initialized=true here - wait for initialize response
                        tracing::debug!("[LSP ANALYZER] Sent initialize request, waiting for response before setting initialized flag");
                        cx.emit(AnalyzerEvent::IndexingProgress {
                            progress: 0.0,
                            message: "Initializing...".to_string(),
                        });
                        cx.notify();
                    });
                }
                Ok(Err(e)) => {
                    tracing::error!("❌ Failed to spawn rust-analyzer: {}", e);
                    let error_msg = format!("Failed to spawn: {}", e);
                    let _ = manager.update(cx, |manager, cx| {
                        manager.status = AnalyzerStatus::Error(error_msg.clone());
                        cx.emit(AnalyzerEvent::Error(error_msg));
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("❌ Thread panicked: {:?}", e);
                    let _ = manager.update(cx, |manager, cx| {
                        manager.status = AnalyzerStatus::Error("Thread panic".to_string());
                        cx.emit(AnalyzerEvent::Error("Thread panic".to_string()));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn spawn_process_blocking(
        analyzer_path: &PathBuf,
        workspace_root: &PathBuf,
        process_arc: Arc<Mutex<Option<Child>>>,
        stdin_arc: Arc<Mutex<Option<std::process::ChildStdin>>>,
        progress_tx: Sender<ProgressUpdate>,
        pending_requests: Arc<Mutex<HashMap<i64, flume::Sender<serde_json::Value>>>>,
    ) -> Result<PathBuf> {
        tracing::debug!("Spawning rust-analyzer process...");
        tracing::debug!("  Binary: {:?}", analyzer_path);
        tracing::debug!("  Workspace: {:?}", workspace_root);

        let mut active_analyzer_path = analyzer_path.clone();

        // Validate candidate before spawn so rustup proxy stubs can trigger self-healing install.
        if let Err(e) = Self::verify_rust_analyzer_executable(&active_analyzer_path) {
            tracing::warn!("⚠️  rust-analyzer candidate failed validation: {}", e);
            tracing::debug!("   Attempting to install rust-analyzer...");
            active_analyzer_path = Self::install_rust_analyzer_to_deps()?;
            tracing::debug!(
                "   Using installed rust-analyzer: {:?}",
                active_analyzer_path
            );
        }

        let spawn_result = Command::new(&active_analyzer_path)
            .current_dir(workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        let mut child = match spawn_result {
            Ok(child) => child,
            Err(e) => {
                tracing::error!("❌ Failed to spawn rust-analyzer: {}", e);
                tracing::error!("   Attempting to install rust-analyzer to engine deps...");

                match Self::install_rust_analyzer_to_deps() {
                    Ok(installed_path) => {
                        tracing::debug!(
                            "✓ Successfully installed rust-analyzer, retrying spawn..."
                        );

                        Command::new(&installed_path)
                            .current_dir(workspace_root)
                            .stdin(Stdio::piped())
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped())
                            .spawn()
                            .map_err(|e| anyhow!("Failed to spawn after installation: {}", e))?
                    }
                    Err(install_err) => {
                        tracing::error!("❌ Failed to install rust-analyzer: {}", install_err);
                        return Err(anyhow!(
                            "Failed to spawn and install: spawn error: {}, install error: {}",
                            e,
                            install_err
                        ));
                    }
                }
            }
        };

        let pid = child.id();
        tracing::debug!("✓ rust-analyzer process spawned (PID: {})", pid);

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Failed to take stdin"))?;

        if let Some(stderr) = child.stderr.take() {
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    tracing::error!("[rust-analyzer stderr] {}", line);
                }
                tracing::error!("❌ rust-analyzer stderr stream ended");
            });
        }

        if let Some(stdout) = child.stdout.take() {
            let progress_tx_stdout = progress_tx.clone();
            let pending_requests_clone = Arc::clone(&pending_requests);
            thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut buffer = String::new();

                loop {
                    buffer.clear();

                    if reader.read_line(&mut buffer).is_err() || buffer.is_empty() {
                        break;
                    }

                    if !buffer.starts_with("Content-Length:") {
                        continue;
                    }

                    let content_len: usize =
                        match buffer.trim_start_matches("Content-Length:").trim().parse() {
                            Ok(len) => len,
                            Err(_) => continue,
                        };

                    buffer.clear();
                    if reader.read_line(&mut buffer).is_err() {
                        break;
                    }

                    let mut content_buffer = vec![0u8; content_len];
                    if let Ok(_) = std::io::Read::read_exact(&mut reader, &mut content_buffer) {
                        if let Ok(content) = String::from_utf8(content_buffer) {
                            if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&content) {
                                if let Some(id) = msg.get("id").and_then(|id| id.as_i64()) {
                                    if let Ok(mut pending) = pending_requests_clone.lock() {
                                        if let Some(tx) = pending.remove(&id) {
                                            tracing::debug!("[LSP ANALYZER] Received response for request id={}", id);
                                            let _ = tx.send(msg.clone());
                                            continue;
                                        }
                                    }
                                    // Handle initialize response specially (id=17 based on our send_initialize_request code)
                                    if id == 17 {
                                        tracing::debug!("[LSP ANALYZER] Received initialize response: result={:?} error={:?}", 
                                            msg.get("result"), msg.get("error"));
                                        if msg.get("error").is_some() {
                                            tracing::debug!("[LSP ANALYZER] Initialize response contains an error!");
                                            let _ = progress_tx_stdout.send(ProgressUpdate::Error(
                                                format!("Initialize error: {:?}", msg.get("error")),
                                            ));
                                        } else if msg.get("result").is_some() {
                                            tracing::debug!("[LSP ANALYZER] Initialize succeeded, server is ready");
                                            // Initialization complete - server is ready for document operations
                                        }
                                    }
                                }
                            }

                            Self::handle_lsp_message(&content, &progress_tx_stdout);
                        }
                    }
                }
                tracing::error!("❌ rust-analyzer stdout stream ended");
            });
        }

        {
            let mut stdin_lock = stdin_arc.lock().unwrap();
            *stdin_lock = Some(stdin);
        }

        let progress_tx_exit = progress_tx.clone();
        thread::spawn(move || match child.wait() {
            Ok(status) => {
                tracing::debug!("❌ rust-analyzer exited with status: {:?}", status);
                let _ = progress_tx_exit.send(ProgressUpdate::ProcessExited(status));
            }
            Err(e) => {
                tracing::error!("❌ Failed to wait for rust-analyzer: {}", e);
                let _ = progress_tx_exit.send(ProgressUpdate::Error(format!("Wait failed: {}", e)));
            }
        });

        {
            let _process_lock = process_arc.lock().unwrap();
        }

        Ok(active_analyzer_path)
    }

    fn send_initialize_request(
        workspace_root: &PathBuf,
        stdin_arc: Arc<Mutex<Option<std::process::ChildStdin>>>,
        request_id_arc: Arc<Mutex<i64>>,
    ) -> Result<()> {
        let workspace_str = workspace_root.to_string_lossy().replace("\\", "/");
        let uri = if workspace_str.starts_with("C:/") || workspace_str.starts_with("c:/") {
            format!("file:///{}", workspace_str)
        } else {
            format!("file://{}", workspace_str)
        };

        tracing::debug!(
            "[LSP ANALYZER] Sending initialize with workspace URI: {}",
            uri
        );

        let mut req_id = request_id_arc
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;
        *req_id += 1;
        let id = *req_id;
        drop(req_id);

        let init_request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "processId": std::process::id(),
                "rootUri": uri,
                "capabilities": {
                    "workspace": {
                        "configuration": true,
                        "workspaceFolders": true,
                        "symbol": {
                            "symbolKind": {
                                "valueSet": [1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26]
                            }
                        },
                        "didChangeWatchedFiles": {
                            "dynamicRegistration": false
                        }
                    },
                    "textDocument": {
                        "synchronization": {
                            "dynamicRegistration": false,
                            "willSave": false,
                            "willSaveWaitUntil": false,
                            "didSave": true
                        },
                        "completion": {
                            "dynamicRegistration": false,
                            "contextSupport": true,
                            "completionItem": {
                                "snippetSupport": true,
                                "commitCharactersSupport": true,
                                "documentationFormat": ["markdown", "plaintext"],
                                "deprecatedSupport": true,
                                "preselectSupport": true,
                                "tagSupport": { "valueSet": [1] },
                                "insertReplaceSupport": true,
                                "resolveSupport": {
                                    "properties": ["documentation", "detail", "additionalTextEdits"]
                                },
                                "insertTextModeSupport": { "valueSet": [1, 2] },
                                "labelDetailsSupport": true
                            },
                            "completionItemKind": {
                                "valueSet": [1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25]
                            }
                        },
                        "hover": {
                            "dynamicRegistration": false,
                            "contentFormat": ["markdown", "plaintext"]
                        },
                        "signatureHelp": {
                            "dynamicRegistration": false,
                            "signatureInformation": {
                                "documentationFormat": ["markdown", "plaintext"],
                                "parameterInformation": {
                                    "labelOffsetSupport": true
                                },
                                "activeParameterSupport": true
                            },
                            "contextSupport": true
                        },
                        "definition": {
                            "dynamicRegistration": false,
                            "linkSupport": true
                        },
                        "references": { "dynamicRegistration": false },
                        "documentHighlight": { "dynamicRegistration": false },
                        "documentSymbol": {
                            "dynamicRegistration": false,
                            "symbolKind": {
                                "valueSet": [1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26]
                            },
                            "hierarchicalDocumentSymbolSupport": true
                        },
                        "codeAction": {
                            "dynamicRegistration": false,
                            "codeActionLiteralSupport": {
                                "codeActionKind": {
                                    "valueSet": ["", "quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source", "source.organizeImports"]
                                }
                            },
                            "resolveSupport": { "properties": ["edit"] },
                            "dataSupport": true
                        },
                        "rename": {
                            "dynamicRegistration": false,
                            "prepareSupport": true
                        },
                        "publishDiagnostics": {
                            "relatedInformation": true,
                            "versionSupport": true,
                            "tagSupport": { "valueSet": [1, 2] },
                            "codeDescriptionSupport": true,
                            "dataSupport": true
                        },
                        "foldingRange": {
                            "dynamicRegistration": false,
                            "rangeLimit": 5000,
                            "lineFoldingOnly": true
                        },
                        "semanticTokens": {
                            "dynamicRegistration": false,
                            "tokenTypes": [
                                "namespace","type","class","enum","interface","struct","typeParameter",
                                "parameter","variable","property","enumMember","event","function",
                                "method","macro","keyword","modifier","comment","string","number",
                                "regexp","operator","decorator"
                            ],
                            "tokenModifiers": [
                                "declaration","definition","readonly","static","deprecated",
                                "abstract","async","modification","documentation","defaultLibrary"
                            ],
                            "formats": ["relative"],
                            "requests": { "range": true, "full": { "delta": true } },
                            "multilineTokenSupport": false,
                            "overlappingTokenSupport": false
                        },
                        "inlayHint": {
                            "dynamicRegistration": false,
                            "resolveSupport": { "properties": ["tooltip", "textEdits", "label.tooltip", "label.location", "label.command"] }
                        }
                    },
                    "window": {
                        "workDoneProgress": true,
                        "showMessage": { "messageActionItem": { "additionalPropertiesSupport": true } },
                        "showDocument": { "support": true }
                    },
                    "general": {
                        "staleRequestSupport": {
                            "cancel": true,
                            "retryOnContentModified": ["textDocument/semanticTokens/full", "textDocument/semanticTokens/full/delta", "textDocument/semanticTokens/range"]
                        },
                        "regularExpressions": { "engine": "ECMAScript", "version": "ES2020" },
                        "markdown": { "parser": "marked", "version": "1.1.0" }
                    },
                    "experimental": {
                        "serverStatusNotification": true
                    }
                },
                "initializationOptions": {
                    "checkOnSave": true,
                    "check": {
                        "command": "clippy"
                    },
                    "cargo": {
                        "buildScripts": { "enable": true },
                        "features": "all"
                    },
                    "procMacro": {
                        "enable": true
                    },
                    "completion": {
                        "autoimport": { "enable": true },
                        "autoself": { "enable": true },
                        "callable": { "snippets": "fill_arguments" },
                        "limit": null
                    },
                    "inlayHints": {
                        "enable": true,
                        "renderColons": true,
                        "typeHints": { "enable": true },
                        "parameterHints": { "enable": true },
                        "chainingHints": { "enable": true }
                    }
                }
            }
        });

        tracing::debug!("[LSP ANALYZER] Sending initialize request with id={}", id);

        let mut stdin_lock = stdin_arc.lock().map_err(|e| anyhow!("Lock error: {}", e))?;
        if let Some(stdin) = stdin_lock.as_mut() {
            let content = serde_json::to_string(&init_request)?;
            let message = format!("Content-Length: {}\r\n\r\n{}", content.len(), content);

            stdin.write_all(message.as_bytes())?;
            stdin.flush()?;

            tracing::debug!(
                "[LSP ANALYZER] Initialize request sent, now sending initialized notification"
            );

            let initialized_notification = json!({
                "jsonrpc": "2.0",
                "method": "initialized",
                "params": {}
            });

            let content = serde_json::to_string(&initialized_notification)?;
            let message = format!("Content-Length: {}\r\n\r\n{}", content.len(), content);

            stdin.write_all(message.as_bytes())?;
            stdin.flush()?;

            tracing::debug!("[LSP ANALYZER] Initialized notification sent");
        } else {
            return Err(anyhow!("stdin not available"));
        }

        Ok(())
    }

    fn handle_lsp_message(content: &str, progress_tx: &Sender<ProgressUpdate>) {
        if let Ok(msg) = serde_json::from_str::<Value>(content) {
            if msg.get("id").is_some() {
                return;
            }

            if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
                match method {
                    "$/progress" => Self::handle_progress_notification(&msg, progress_tx),
                    "textDocument/publishDiagnostics" => {
                        Self::handle_diagnostics_notification(&msg, progress_tx)
                    }
                    "window/workDoneProgress/create" => {
                        tracing::debug!("📊 Work done progress created")
                    }
                    "rust-analyzer/serverStatus" => Self::handle_server_status(&msg, progress_tx),
                    _ => tracing::debug!("📨 Unhandled LSP notification: {}", method),
                }
            }
        }
    }

    fn handle_progress_notification(msg: &Value, progress_tx: &Sender<ProgressUpdate>) {
        if let Some(params) = msg.get("params") {
            if let Some(value) = params.get("value") {
                if let Some(kind) = value.get("kind").and_then(|k| k.as_str()) {
                    match kind {
                        "begin" => {
                            let title = value
                                .get("title")
                                .and_then(|t| t.as_str())
                                .unwrap_or("Processing");
                            let message = if title.contains("Fetching") || title.contains("Loading")
                            {
                                "Loading metadata...".to_string()
                            } else if title.contains("Indexing") || title.contains("Building") {
                                "Indexing crates...".to_string()
                            } else {
                                title.to_string()
                            };
                            let _ = progress_tx.send(ProgressUpdate::Progress {
                                progress: 0.0,
                                message,
                            });
                        }
                        "report" => {
                            let message =
                                value.get("message").and_then(|m| m.as_str()).unwrap_or("");
                            let percentage = value
                                .get("percentage")
                                .and_then(|p| p.as_u64())
                                .unwrap_or(0);
                            let display_message = if !message.is_empty() {
                                if message.contains('/') {
                                    format!("Analyzing ({})...", message)
                                } else {
                                    message.to_string()
                                }
                            } else {
                                format!("{}%", percentage)
                            };
                            let _ = progress_tx.send(ProgressUpdate::Progress {
                                progress: (percentage as f32) / 100.0,
                                message: display_message,
                            });
                        }
                        "end" => {}
                        _ => {}
                    }
                }
            }
        }
    }

    fn handle_diagnostics_notification(msg: &Value, progress_tx: &Sender<ProgressUpdate>) {
        if let Some(params) = msg.get("params") {
            if let Some(diagnostics_array) = params.get("diagnostics").and_then(|d| d.as_array()) {
                if let Some(uri) = params.get("uri").and_then(|u| u.as_str()) {
                    let file_path = path_utils::uri_to_path(uri);
                    let diagnostics: Vec<Diagnostic> = diagnostics_array
                        .iter()
                        .filter_map(|diag| Self::parse_diagnostic(diag, &file_path))
                        .collect();

                    if !diagnostics.is_empty() {
                        let _ = progress_tx.send(ProgressUpdate::Diagnostics(diagnostics));
                    }
                }
            }
        }
    }

    fn parse_diagnostic(diag: &Value, file_path: &str) -> Option<Diagnostic> {
        let range = diag.get("range")?;
        let message = diag.get("message")?.as_str()?;
        let start = range.get("start")?;
        let severity_num = diag.get("severity")?.as_u64()?;

        let line = start.get("line")?.as_u64()? as usize + 1;
        let column = start.get("character")?.as_u64()? as usize + 1;

        let (end_line, end_column) = range.get("end").map_or((None, None), |end| {
            let el = end
                .get("line")
                .and_then(|l| l.as_u64())
                .map(|l| l as usize + 1);
            let ec = end
                .get("character")
                .and_then(|c| c.as_u64())
                .map(|c| c as usize + 1);
            (el, ec)
        });

        let severity = match severity_num {
            1 => DiagnosticSeverity::Error,
            2 => DiagnosticSeverity::Warning,
            3 => DiagnosticSeverity::Information,
            4 => DiagnosticSeverity::Hint,
            _ => DiagnosticSeverity::Information,
        };

        let code = diag.get("code").and_then(|c| {
            if c.is_string() {
                c.as_str().map(|s| s.to_string())
            } else if c.is_number() {
                c.as_i64().map(|n| n.to_string())
            } else {
                None
            }
        });

        let mut code_actions = Vec::new();
        Self::extract_code_actions_from_data(diag, &mut code_actions);
        Self::extract_code_actions_from_related_info(diag, file_path, &mut code_actions);

        Some(Diagnostic {
            file_path: file_path.to_string(),
            line,
            column,
            end_line,
            end_column,
            severity,
            message: message.to_string(),
            code,
            source: Some("rust-analyzer".to_string()),
            code_actions,
            raw_lsp_diagnostic: Some(diag.clone()),
        })
    }

    fn extract_code_actions_from_data(diag: &Value, code_actions: &mut Vec<CodeAction>) {
        if let Some(data) = diag.get("data") {
            if let Some(fixes) = data.get("fixes").and_then(|f| f.as_array()) {
                for fix in fixes {
                    if let Some(title) = fix.get("title").and_then(|t| t.as_str()) {
                        if let Some(edit) = fix.get("edit") {
                            let mut edits = Vec::new();
                            Self::extract_text_edits_from_changes(edit, &mut edits);
                            Self::extract_text_edits_from_doc_changes(edit, &mut edits);
                            if !edits.is_empty() {
                                code_actions.push(CodeAction {
                                    title: title.to_string(),
                                    edits,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    fn extract_text_edits_from_changes(edit: &Value, edits: &mut Vec<TextEdit>) {
        if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
            for (edit_uri, edit_array) in changes {
                if let Some(edit_list) = edit_array.as_array() {
                    let edit_file = path_utils::uri_to_path(edit_uri);
                    for text_edit in edit_list {
                        if let Some(te) = Self::parse_text_edit(text_edit, &edit_file) {
                            edits.push(te);
                        }
                    }
                }
            }
        }
    }

    fn extract_text_edits_from_doc_changes(edit: &Value, edits: &mut Vec<TextEdit>) {
        if let Some(doc_changes) = edit.get("documentChanges").and_then(|c| c.as_array()) {
            for doc_change in doc_changes {
                if let Some(text_doc) = doc_change.get("textDocument") {
                    let edit_file = text_doc
                        .get("uri")
                        .and_then(|u| u.as_str())
                        .map(path_utils::uri_to_path)
                        .unwrap_or_default();

                    if let Some(edit_list) = doc_change.get("edits").and_then(|e| e.as_array()) {
                        for text_edit in edit_list {
                            if let Some(te) = Self::parse_text_edit(text_edit, &edit_file) {
                                edits.push(te);
                            }
                        }
                    }
                }
            }
        }
    }

    fn parse_text_edit(text_edit: &Value, file_path: &str) -> Option<TextEdit> {
        let edit_range = text_edit.get("range")?;
        let new_text = text_edit.get("newText")?.as_str()?;
        let edit_start = edit_range.get("start")?;
        let edit_end = edit_range.get("end")?;

        Some(TextEdit {
            file_path: file_path.to_string(),
            start_line: edit_start.get("line")?.as_u64()? as usize + 1,
            start_column: edit_start.get("character")?.as_u64()? as usize + 1,
            end_line: edit_end.get("line")?.as_u64()? as usize + 1,
            end_column: edit_end.get("character")?.as_u64()? as usize + 1,
            new_text: new_text.to_string(),
        })
    }

    fn extract_code_actions_from_related_info(
        diag: &Value,
        file_path: &str,
        code_actions: &mut Vec<CodeAction>,
    ) {
        if let Some(related_info) = diag.get("relatedInformation").and_then(|r| r.as_array()) {
            for info in related_info {
                if let Some(info_message) = info.get("message").and_then(|m| m.as_str()) {
                    if info_message.to_lowercase().contains("remove") {
                        if let Some(location) = info.get("location") {
                            if let Some(info_range) = location.get("range") {
                                if let (Some(info_start), Some(info_end)) =
                                    (info_range.get("start"), info_range.get("end"))
                                {
                                    let info_uri = location
                                        .get("uri")
                                        .and_then(|u| u.as_str())
                                        .map(path_utils::uri_to_path)
                                        .unwrap_or_else(|| file_path.to_string());

                                    let edit = TextEdit {
                                        file_path: info_uri,
                                        start_line: info_start
                                            .get("line")
                                            .and_then(|l| l.as_u64())
                                            .unwrap_or(0)
                                            as usize
                                            + 1,
                                        start_column: info_start
                                            .get("character")
                                            .and_then(|c| c.as_u64())
                                            .unwrap_or(0)
                                            as usize
                                            + 1,
                                        end_line: info_end
                                            .get("line")
                                            .and_then(|l| l.as_u64())
                                            .unwrap_or(0)
                                            as usize
                                            + 1,
                                        end_column: info_end
                                            .get("character")
                                            .and_then(|c| c.as_u64())
                                            .unwrap_or(0)
                                            as usize
                                            + 1,
                                        new_text: String::new(),
                                    };

                                    code_actions.push(CodeAction {
                                        title: info_message.trim_end_matches('.').to_string(),
                                        edits: vec![edit],
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn handle_server_status(msg: &Value, progress_tx: &Sender<ProgressUpdate>) {
        if let Some(params) = msg.get("params") {
            tracing::debug!("🔔 rust-analyzer/serverStatus: {:?}", params);
            if let Some(quiescent) = params.get("quiescent").and_then(|q| q.as_bool()) {
                if quiescent {
                    tracing::debug!("✅ rust-analyzer is quiescent (all indexing complete)");
                    let _ = progress_tx.send(ProgressUpdate::Ready);
                }
            }
        }
    }

    /// Stop rust-analyzer
    pub fn stop(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        tracing::debug!("🛑 Stopping rust-analyzer");
        self.stop_internal();
        self.status = AnalyzerStatus::Stopped;
        cx.emit(AnalyzerEvent::StatusChanged(AnalyzerStatus::Stopped));
        cx.notify();
    }

    fn stop_internal(&mut self) {
        {
            let mut stdin_lock = self.stdin.lock().unwrap();
            *stdin_lock = None;
        }

        {
            let mut pending = self.pending_requests.lock().unwrap();
            pending.clear();
        }

        let mut process_lock = self.process.lock().unwrap();
        if let Some(mut child) = process_lock.take() {
            let _ = child.kill();
            let _ = child.wait();
            tracing::debug!("✓ rust-analyzer process terminated");
        }
        self.initialized = false;
        self.progress_rx = None;
    }

    /// Restart rust-analyzer
    pub fn restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        tracing::debug!("🔄 Restarting rust-analyzer");
        if let Some(workspace) = self.workspace_root.clone() {
            self.stop(window, cx);
            thread::sleep(Duration::from_millis(500));
            self.start(workspace, window, cx);
        }
    }

    /// Get current status
    pub fn status(&self) -> &AnalyzerStatus {
        &self.status
    }

    /// Check if analyzer is running
    pub fn is_running(&self) -> bool {
        matches!(
            self.status,
            AnalyzerStatus::Starting | AnalyzerStatus::Indexing { .. } | AnalyzerStatus::Ready
        )
    }

    /// Check if analyzer is fully initialized (LSP handshake complete)
    /// Per LSP spec, server must complete initialize handshake before accepting document operations.
    /// We know this is complete once we transition to Indexing status (which happens after initialize response).
    pub fn is_initialized(&self) -> bool {
        matches!(
            self.status,
            AnalyzerStatus::Indexing { .. } | AnalyzerStatus::Ready
        )
    }

    /// Send didOpen notification for a file
    pub fn did_open_file(
        &self,
        file_path: &PathBuf,
        content: &str,
        language_id: &str,
    ) -> Result<()> {
        // CRITICAL: Per LSP spec, must wait for initialize response before sending didOpen
        if !self.is_initialized() {
            tracing::debug!(
                "[LSP ANALYZER] did_open_file: NOT YET INITIALIZED, deferring didOpen (status={:?}, initialized={})",
                self.status,
                self.initialized
            );
            return Err(anyhow!("Analyzer not yet initialized"));
        }

        if !self.is_running() {
            tracing::debug!("[LSP ANALYZER] did_open_file called but analyzer not running");
            return Err(anyhow!("Analyzer not running"));
        }

        let uri = self.path_to_uri(file_path);
        tracing::debug!(
            "[LSP ANALYZER] did_open_file: path={:?} uri={} content_len={}",
            file_path,
            uri,
            content.len()
        );

        let notification = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": content
                }
            }
        });

        self.send_notification(notification)
    }

    /// Send didChange notification for a file
    pub fn did_change_file(&self, file_path: &PathBuf, content: &str, version: i32) -> Result<()> {
        if !self.is_running() {
            return Ok(());
        }

        let uri = self.path_to_uri(file_path);
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "version": version
                },
                "contentChanges": [{
                    "text": content
                }]
            }
        });

        self.send_notification(notification)
    }

    /// Send didSave notification for a file (triggers re-analysis)
    pub fn did_save_file(&self, file_path: &PathBuf, content: &str) -> Result<()> {
        if !self.is_running() {
            return Ok(());
        }

        let uri = self.path_to_uri(file_path);
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didSave",
            "params": {
                "textDocument": {
                    "uri": uri
                },
                "text": content
            }
        });

        tracing::debug!("💾 Notifying rust-analyzer of file save: {:?}", file_path);
        self.send_notification(notification)
    }

    /// Send didClose notification for a file
    pub fn did_close_file(&self, file_path: &PathBuf) -> Result<()> {
        if !self.is_running() {
            return Ok(());
        }

        let uri = self.path_to_uri(file_path);
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didClose",
            "params": {
                "textDocument": {
                    "uri": uri
                }
            }
        });

        self.send_notification(notification)
    }

    fn path_to_uri(&self, path: &PathBuf) -> String {
        path_utils::path_to_uri(path)
    }

    fn send_notification(&self, notification: Value) -> Result<()> {
        if let Some(method) = notification.get("method").and_then(|m| m.as_str()) {
            if method == "textDocument/didOpen" {
                if let Some(params) = notification.get("params") {
                    tracing::debug!("[LSP ANALYZER] Sending didOpen notification: {:?}", params);
                }
            } else if method == "textDocument/didChange" {
                tracing::debug!("[LSP ANALYZER] Sending didChange notification");
            }
        }

        let mut stdin_lock = self
            .stdin
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;
        if let Some(stdin) = stdin_lock.as_mut() {
            let content = serde_json::to_string(&notification)?;
            let message = format!("Content-Length: {}\r\n\r\n{}", content.len(), content);
            stdin.write_all(message.as_bytes())?;
            stdin.flush()?;
            Ok(())
        } else {
            Err(anyhow!("stdin not available"))
        }
    }

    /// Send a request to rust-analyzer and return a receiver for the async response.
    pub fn send_request_async(
        &self,
        method: &str,
        params: Value,
    ) -> Result<flume::Receiver<Value>> {
        if !self.is_running() {
            return Err(anyhow!("rust-analyzer is not running"));
        }

        let mut req_id = self
            .request_id
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;
        *req_id += 1;
        let id = *req_id;
        drop(req_id);

        let (response_tx, response_rx) = flume::unbounded();

        {
            let mut pending = self
                .pending_requests
                .lock()
                .map_err(|e| anyhow!("Lock error: {}", e))?;
            pending.insert(id, response_tx);
        }

        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        if method == "textDocument/hover" {
            tracing::debug!(
                "[LSP ANALYZER] sending hover request: id={} params={}",
                id,
                serde_json::to_string(&params).unwrap_or_default()
            );
        }

        let mut stdin_lock = self
            .stdin
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;
        if let Some(stdin) = stdin_lock.as_mut() {
            let content = serde_json::to_string(&request)?;
            let message = format!("Content-Length: {}\r\n\r\n{}", content.len(), content);
            stdin.write_all(message.as_bytes())?;
            stdin.flush()?;
        } else {
            let mut pending = self
                .pending_requests
                .lock()
                .map_err(|e| anyhow!("Lock error: {}", e))?;
            pending.remove(&id);
            return Err(anyhow!("stdin not available"));
        }
        drop(stdin_lock);

        Ok(response_rx)
    }

    /// Send a request and block until a response is received (5-second timeout).
    ///
    /// Prefer [`send_request_async`](Self::send_request_async) for better performance.
    pub fn send_request(&self, method: &str, params: Value) -> Result<Value> {
        let rx = self.send_request_async(method, params)?;
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(response) => Ok(response),
            Err(e) => Err(anyhow!("Request timeout: {}", e)),
        }
    }

    /// Drain the progress channel and apply any pending updates.
    ///
    /// Should be called periodically from the UI thread (e.g., from a timer
    /// subscription on the `RustAnalyzerManager` entity).
    pub fn update_progress_from_thread(&mut self, cx: &mut Context<Self>) {
        let mut updates = Vec::new();
        if let Some(rx) = &self.progress_rx {
            while let Ok(update) = rx.try_recv() {
                updates.push(update);
            }
        }

        for update in updates {
            self.handle_progress_update(update, cx);
        }

        if !self.initial_analysis_complete {
            if let Some(last_update) = self.last_update {
                if last_update.elapsed() > Duration::from_secs(3)
                    && matches!(self.status, AnalyzerStatus::Indexing { .. })
                {
                    self.initial_analysis_complete = true;
                    self.status = AnalyzerStatus::Ready;
                    tracing::debug!("✅ Initial analysis complete (timeout - no updates for 3s)");
                    cx.emit(AnalyzerEvent::Ready);
                    cx.notify();
                }
            }
        }
    }

    fn handle_progress_update(&mut self, update: ProgressUpdate, cx: &mut Context<Self>) {
        match update {
            ProgressUpdate::Progress { progress, message } => {
                self.status = AnalyzerStatus::Indexing {
                    progress,
                    message: message.clone(),
                };
                self.last_update = Some(Instant::now());
                cx.emit(AnalyzerEvent::IndexingProgress { progress, message });
                cx.notify();
            }
            ProgressUpdate::Ready => {
                if !self.initial_analysis_complete {
                    self.initial_analysis_complete = true;
                    self.status = AnalyzerStatus::Ready;
                    tracing::debug!("✅ Initial analysis marked as complete");
                    cx.emit(AnalyzerEvent::Ready);
                    cx.notify();
                }
            }
            ProgressUpdate::Error(e) => {
                self.status = AnalyzerStatus::Error(e.clone());
                cx.emit(AnalyzerEvent::Error(e));
                cx.notify();
            }
            ProgressUpdate::ProcessExited(status) => {
                let error_msg = if status.success() {
                    "rust-analyzer exited normally".to_string()
                } else {
                    format!("rust-analyzer exited with error (status: {:?})", status)
                };
                tracing::debug!("❌ {}", error_msg);
                self.status = AnalyzerStatus::Error(error_msg.clone());
                self.initialized = false;
                cx.emit(AnalyzerEvent::Error(error_msg));
                cx.notify();
            }
            ProgressUpdate::Diagnostics(diagnostics) => {
                if self.first_diagnostics_time.is_none() {
                    self.first_diagnostics_time = Some(Instant::now());
                    tracing::debug!("📊 First diagnostics received - analyzer is working");
                }

                if let Some(first_time) = self.first_diagnostics_time {
                    if !self.initial_analysis_complete
                        && first_time.elapsed() > Duration::from_secs(2)
                    {
                        self.initial_analysis_complete = true;
                        self.status = AnalyzerStatus::Ready;
                        tracing::debug!(
                            "✅ Initial analysis complete based on diagnostics (received for 2s)"
                        );
                        cx.emit(AnalyzerEvent::Ready);
                        cx.notify();
                    }
                }

                tracing::debug!(
                    "📤 EMITTING AnalyzerEvent::Diagnostics with {} diagnostics",
                    diagnostics.len()
                );
                cx.emit(AnalyzerEvent::Diagnostics(diagnostics));
            }
        }
    }

    /// Request hover information at a specific position
    pub fn hover(&self, file_path: &PathBuf, line: usize, column: usize) -> Result<Value> {
        if !self.is_running() {
            return Err(anyhow!("rust-analyzer is not running"));
        }

        let uri = self.path_to_uri(file_path);
        let params = json!({
            "textDocument": { "uri": uri },
            "position": {
                "line": line.saturating_sub(1),
                "character": column.saturating_sub(1)
            }
        });

        self.send_request("textDocument/hover", params)
    }

    /// Request go-to-definition at a specific position
    pub fn definition(&self, file_path: &PathBuf, line: usize, column: usize) -> Result<Value> {
        if !self.is_running() {
            return Err(anyhow!("rust-analyzer is not running"));
        }

        let uri = self.path_to_uri(file_path);
        let params = json!({
            "textDocument": { "uri": uri },
            "position": {
                "line": line.saturating_sub(1),
                "character": column.saturating_sub(1)
            }
        });

        self.send_request("textDocument/definition", params)
    }

    /// Request code actions for a range; returns an async receiver.
    pub fn request_code_actions_async(
        &self,
        file_path: &PathBuf,
        start_line: usize,
        start_column: usize,
        end_line: usize,
        end_column: usize,
        diagnostic_message: Option<&str>,
    ) -> Result<flume::Receiver<Value>> {
        if !self.is_running() {
            tracing::warn!("📛 request_code_actions_async: rust-analyzer is not running!");
            return Err(anyhow!("rust-analyzer is not running"));
        }

        tracing::debug!(
            "📤 request_code_actions_async: file={:?}, range={}:{}-{}:{}, msg={:?}",
            file_path,
            start_line,
            start_column,
            end_line,
            end_column,
            diagnostic_message
        );

        let uri = self.path_to_uri(file_path);

        let diagnostics = if let Some(msg) = diagnostic_message {
            vec![json!({
                "range": {
                    "start": {
                        "line": start_line.saturating_sub(1),
                        "character": start_column.saturating_sub(1)
                    },
                    "end": {
                        "line": end_line.saturating_sub(1),
                        "character": end_column.saturating_sub(1)
                    }
                },
                "message": msg,
                "severity": 1
            })]
        } else {
            vec![]
        };

        let params = json!({
            "textDocument": { "uri": uri },
            "range": {
                "start": {
                    "line": start_line.saturating_sub(1),
                    "character": start_column.saturating_sub(1)
                },
                "end": {
                    "line": end_line.saturating_sub(1),
                    "character": end_column.saturating_sub(1)
                }
            },
            "context": {
                "diagnostics": diagnostics,
                "only": ["quickfix"],
                "triggerKind": 1
            }
        });

        self.send_request_async("textDocument/codeAction", params)
    }

    /// Request code actions using the raw LSP diagnostic for better matching.
    pub fn request_code_actions_with_diagnostic(
        &self,
        file_path: &PathBuf,
        raw_diagnostic: &Value,
    ) -> Result<flume::Receiver<Value>> {
        if !self.is_running() {
            tracing::warn!(
                "📛 request_code_actions_with_diagnostic: rust-analyzer is not running!"
            );
            return Err(anyhow!("rust-analyzer is not running"));
        }

        let uri = self.path_to_uri(file_path);

        let range = raw_diagnostic.get("range").cloned().unwrap_or_else(|| {
            json!({
                "start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": 0}
            })
        });

        let params = json!({
            "textDocument": { "uri": uri },
            "range": range,
            "context": {
                "diagnostics": [raw_diagnostic],
                "only": ["quickfix"],
                "triggerKind": 1
            }
        });

        self.send_request_async("textDocument/codeAction", params)
    }

    /// Parse a code action response into a list of [`CodeAction`]s.
    pub fn parse_code_actions(response: &Value) -> Vec<CodeAction> {
        let mut actions = Vec::new();

        tracing::debug!(
            "📥 parse_code_actions received: {}",
            serde_json::to_string_pretty(response).unwrap_or_default()
        );

        if let Some(arr) = response.as_array() {
            for action in arr {
                tracing::debug!(
                    "📋 Parsing action: {}",
                    action
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("no title")
                );
                if let Some(parsed) = Self::parse_single_code_action(action) {
                    actions.push(parsed);
                }
            }
        }

        actions
    }

    /// Parse a single code action JSON value into a [`CodeAction`].
    pub fn parse_single_code_action(action: &Value) -> Option<CodeAction> {
        let title = action
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("Unknown action")
            .to_string();

        let mut edits = Vec::new();

        if let Some(edit) = action.get("edit") {
            if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
                for (uri, edit_array) in changes {
                    if let Some(edit_list) = edit_array.as_array() {
                        let file_path = Self::uri_to_path(uri);
                        for text_edit in edit_list {
                            if let Some(te) = Self::parse_text_edit(text_edit, &file_path) {
                                edits.push(te);
                            }
                        }
                    }
                }
            }

            if let Some(doc_changes) = edit.get("documentChanges").and_then(|c| c.as_array()) {
                for doc_change in doc_changes {
                    if let Some(text_doc) = doc_change.get("textDocument") {
                        let file_path = text_doc
                            .get("uri")
                            .and_then(|u| u.as_str())
                            .map(Self::uri_to_path)
                            .unwrap_or_default();

                        if let Some(edit_list) = doc_change.get("edits").and_then(|e| e.as_array())
                        {
                            for text_edit in edit_list {
                                if let Some(te) = Self::parse_text_edit(text_edit, &file_path) {
                                    edits.push(te);
                                }
                            }
                        }
                    }
                }
            }
        }

        if !edits.is_empty() {
            Some(CodeAction { title, edits })
        } else {
            None
        }
    }

    /// Convert a file URI to a local path string.
    fn uri_to_path(uri: &str) -> String {
        path_utils::uri_to_path(uri)
    }

    /// Return unresolved code actions (have `data` but no `edit`).
    pub fn get_unresolved_actions(response: &Value) -> Vec<Value> {
        let mut unresolved = Vec::new();

        if let Some(arr) = response.as_array() {
            for action in arr {
                if action.get("data").is_some() && action.get("edit").is_none() {
                    unresolved.push(action.clone());
                }
            }
        }

        unresolved
    }

    /// Resolve a code action (fetch its text edits) asynchronously.
    pub fn resolve_code_action_async(&self, action: &Value) -> Result<flume::Receiver<Value>> {
        if !self.is_running() {
            return Err(anyhow!("rust-analyzer is not running"));
        }

        self.send_request_async("codeAction/resolve", action.clone())
    }
}

impl Drop for RustAnalyzerManager {
    fn drop(&mut self) {
        self.stop_internal();
    }
}
