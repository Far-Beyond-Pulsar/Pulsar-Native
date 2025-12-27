use anyhow::{anyhow, Result};
use gpui::{Context, EventEmitter, Window};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio, ExitStatus};
use std::sync::{Arc, Mutex};
use std::io::{BufRead, BufReader, Write, Read};
use std::time::{Duration, Instant};
use std::thread;
use std::sync::mpsc::{channel, Sender, Receiver};
use std::fs;
use ui::diagnostics::{Diagnostic, DiagnosticSeverity};

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

use std::collections::HashMap;

impl EventEmitter<AnalyzerEvent> for RustAnalyzerManager {}

impl RustAnalyzerManager {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let analyzer_path = Self::find_or_use_bundled_analyzer();

        tracing::info!("🔧 Rust Analyzer Manager initialized");
        tracing::info!("   Using: {:?}", analyzer_path);

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

    /// Find rust-analyzer in PATH or use bundled version
    fn find_or_use_bundled_analyzer() -> PathBuf {
        // First, try using rustup to get the component path (handles rustup proxies)
        if let Some(rustup_path) = Self::find_rust_analyzer_via_rustup() {
            return rustup_path;
        }

        // Try to find in PATH first
        let candidates = vec![
            "rust-analyzer.exe",
            "rust-analyzer",
        ];

        for candidate in &candidates {
            if let Ok(output) = Command::new(candidate).arg("--version").output() {
                if output.status.success() {
                    let version_output = String::from_utf8_lossy(&output.stdout);
                    // Check if this is a rustup proxy by looking for the error message
                    if version_output.contains("Unknown binary") || version_output.contains("official toolchain") {
                        tracing::info!("⚠️  Found rustup proxy, but rust-analyzer component not installed");
                        continue;
                    }
                    tracing::info!("✓ Found system rust-analyzer: {}", candidate);
                    tracing::info!("   Version: {}", version_output.trim());
                    return PathBuf::from(candidate);
                }
            }
        }

        // Check cargo bin directory
        if let Ok(home) = std::env::var("CARGO_HOME") {
            let cargo_bin = PathBuf::from(home).join("bin").join("rust-analyzer.exe");
            if cargo_bin.exists() {
                tracing::info!("✓ Found rust-analyzer in cargo bin: {:?}", cargo_bin);
                return cargo_bin;
            }
        }

        if let Ok(home) = std::env::var("USERPROFILE") {
            let cargo_bin = PathBuf::from(home).join(".cargo").join("bin").join("rust-analyzer.exe");
            if cargo_bin.exists() {
                tracing::info!("✓ Found rust-analyzer in user cargo bin: {:?}", cargo_bin);
                return cargo_bin;
            }
        }

        // Check engine deps directory
        let deps_path = Self::get_engine_deps_analyzer_path();
        if deps_path.exists() {
            tracing::info!("✓ Found rust-analyzer in engine deps: {:?}", deps_path);
            return deps_path;
        }

        // Fallback to rust-analyzer command (may not exist)
        tracing::info!("⚠️  rust-analyzer not found in standard locations");
        tracing::info!("   Will attempt to use 'rust-analyzer' from PATH");
        PathBuf::from("rust-analyzer")
    }

    /// Try to find rust-analyzer via rustup (handles rustup proxy wrappers)
    fn find_rust_analyzer_via_rustup() -> Option<PathBuf> {
        // Try to use rustup to find the actual rust-analyzer binary
        let rustup_cmd = if cfg!(windows) { "rustup.exe" } else { "rustup" };
        
        // First check if rust-analyzer component is installed
        if let Ok(output) = Command::new(rustup_cmd)
            .args(&["component", "list", "--installed"])
            .output()
        {
            if output.status.success() {
                let installed = String::from_utf8_lossy(&output.stdout);
                if installed.contains("rust-analyzer") {
                    // Component is installed, try to get its path
                    if let Ok(output) = Command::new(rustup_cmd)
                        .args(&["which", "rust-analyzer"])
                        .output()
                    {
                        if output.status.success() {
                            let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                            let path = PathBuf::from(path_str);
                            if path.exists() {
                                tracing::info!("✓ Found rust-analyzer via rustup: {:?}", path);
                                return Some(path);
                            }
                        }
                    }
                } else {
                    tracing::info!("ℹ️  rust-analyzer component not installed via rustup");
                    tracing::info!("   You can install it with: rustup component add rust-analyzer");
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

        // Get the current executable's directory (engine binary location)
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                return exe_dir.join("deps").join(exe_name);
            }
        }

        // Fallback to current directory
        PathBuf::from("deps").join(exe_name)
    }

    /// Download and install rust-analyzer to the engine deps directory
    fn install_rust_analyzer_to_deps() -> Result<PathBuf> {
        tracing::info!("📦 Attempting to install rust-analyzer...");

        // First, try to install via rustup (easiest and most reliable)
        if let Ok(installed_path) = Self::install_rust_analyzer_via_rustup() {
            return Ok(installed_path);
        }

        // If rustup installation fails, fall back to manual download
        tracing::info!("   Rustup installation not available, trying manual download...");
        Self::download_rust_analyzer_binary()
    }

    /// Try to install rust-analyzer via rustup component
    fn install_rust_analyzer_via_rustup() -> Result<PathBuf> {
        let rustup_cmd = if cfg!(windows) { "rustup.exe" } else { "rustup" };
        
        tracing::info!("   Trying to install via rustup...");
        
        // Try to install the rust-analyzer component
        let output = Command::new(rustup_cmd)
            .args(&["component", "add", "rust-analyzer"])
            .output()
            .map_err(|e| anyhow!("Failed to run rustup: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Rustup component add failed: {}", stderr));
        }

        tracing::info!("✓ Installed rust-analyzer component via rustup");

        // Now get the path to the installed binary
        let output = Command::new(rustup_cmd)
            .args(&["which", "rust-analyzer"])
            .output()
            .map_err(|e| anyhow!("Failed to locate rust-analyzer: {}", e))?;

        if !output.status.success() {
            return Err(anyhow!("Failed to locate rust-analyzer after installation"));
        }

        let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let path = PathBuf::from(path_str);

        if !path.exists() {
            return Err(anyhow!("rust-analyzer path does not exist: {:?}", path));
        }

        // Verify it works
        if let Ok(output) = Command::new(&path).arg("--version").output() {
            if output.status.success() {
                let version = String::from_utf8_lossy(&output.stdout);
                tracing::info!("✓ rust-analyzer installed and verified via rustup!");
                tracing::info!("   Version: {}", version.trim());
                return Ok(path);
            }
        }

        Err(anyhow!("rust-analyzer installed but verification failed"))
    }

    /// Download rust-analyzer binary directly from GitHub
    fn download_rust_analyzer_binary() -> Result<PathBuf> {
        tracing::info!("   Downloading rust-analyzer to engine deps directory...");

        let deps_path = Self::get_engine_deps_analyzer_path();
        let deps_dir = deps_path.parent().ok_or_else(|| anyhow!("Invalid deps path"))?;

        // Create deps directory if it doesn't exist
        fs::create_dir_all(deps_dir)?;
        tracing::info!("   Created deps directory: {:?}", deps_dir);

        // Determine platform and download URL
        let (platform, extension) = if cfg!(target_os = "windows") {
            ("pc-windows-msvc", ".exe")
        } else if cfg!(target_os = "linux") {
            ("unknown-linux-gnu", "")
        } else if cfg!(target_os = "macos") {
            ("apple-darwin", "")
        } else {
            return Err(anyhow!("Unsupported platform for automatic installation"));
        };

        let url = format!(
            "https://github.com/rust-lang/rust-analyzer/releases/latest/download/rust-analyzer-x86_64-{}{}",
            platform, extension
        );

        tracing::info!("   Downloading from: {}", url);
        tracing::info!("   This may take a moment...");

        // Download using curl or wget (cross-platform)
        let download_result = if cfg!(windows) {
            // Try PowerShell Invoke-WebRequest on Windows
            Command::new("powershell")
                .args(&[
                    "-NoProfile",
                    "-Command",
                    &format!("Invoke-WebRequest -Uri '{}' -OutFile '{}'", url, deps_path.display()),
                ])
                .output()
        } else {
            // Try curl on Unix-like systems
            Command::new("curl")
                .args(&["-L", &url, "-o", &deps_path.to_string_lossy()])
                .output()
        };

        match download_result {
            Ok(output) if output.status.success() => {
                tracing::info!("✓ Downloaded rust-analyzer successfully");

                // Make executable on Unix-like systems
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = fs::metadata(&deps_path)?.permissions();
                    perms.set_mode(0o755);
                    fs::set_permissions(&deps_path, perms)?;
                    tracing::info!("✓ Made rust-analyzer executable");
                }

                // Verify the downloaded file works
                if let Ok(output) = Command::new(&deps_path).arg("--version").output() {
                    if output.status.success() {
                        let version = String::from_utf8_lossy(&output.stdout);
                        tracing::info!("✓ rust-analyzer installed successfully!");
                        tracing::info!("   Version: {}", version.trim());
                        return Ok(deps_path);
                    }
                }

                Err(anyhow!("Downloaded rust-analyzer but failed to verify"))
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(anyhow!("Failed to download rust-analyzer: {}", stderr))
            }
            Err(e) => Err(anyhow!("Failed to execute download command: {}", e)),
        }
    }

    /// Start rust-analyzer for the given workspace
    pub fn start(&mut self, workspace_root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!("🚀 Starting rust-analyzer for: {:?}", workspace_root);

        self.workspace_root = Some(workspace_root.clone());
        self.status = AnalyzerStatus::Starting;
        self.install_attempted = false; // Reset installation attempt flag
        self.first_diagnostics_time = None; // Reset diagnostics tracking
        self.initial_analysis_complete = false; // Reset completion flag
        cx.emit(AnalyzerEvent::StatusChanged(AnalyzerStatus::Starting));
        cx.notify();

        // Stop existing process if any
        self.stop_internal();

        // Create channel for progress updates
        let (progress_tx, progress_rx) = channel();
        self.progress_rx = Some(progress_rx);

        // Spawn async task to start the process
        let analyzer_path = self.analyzer_path.clone();
        let process_arc = self.process.clone();
        let stdin_arc = self.stdin.clone();
        let request_id_arc = self.request_id.clone();
        let pending_requests_arc = self.pending_requests.clone();

        cx.spawn_in(window, async move |manager, cx| {
            // Spawn the process in a background thread
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
            }).join();

            match spawn_result {
                Ok(Ok(())) => {
                    tracing::info!("✓ rust-analyzer process spawned successfully");

                    // Send initialize request in a background thread
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
                            let _ = progress_tx_for_init.send(ProgressUpdate::Error(format!("Init failed: {}", e)));
                        }
                        // Note: We removed monitor_progress() call here
                        // Real rust-analyzer sends progress via LSP $/progress notifications
                        // The fake progress monitor was masking actual errors
                    });

                    // Update status to indexing
                    let _ = manager.update(cx, |manager, cx| {
                        manager.status = AnalyzerStatus::Indexing {
                            progress: 0.0,
                            message: "Initializing...".to_string(),
                        };
                        manager.initialized = true;
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
        }).detach();
    }

    fn spawn_process_blocking(
        analyzer_path: &PathBuf,
        workspace_root: &PathBuf,
        process_arc: Arc<Mutex<Option<Child>>>,
        stdin_arc: Arc<Mutex<Option<std::process::ChildStdin>>>,
        progress_tx: Sender<ProgressUpdate>,
        pending_requests: Arc<Mutex<HashMap<i64, flume::Sender<serde_json::Value>>>>,
    ) -> Result<()> {
        tracing::info!("Spawning rust-analyzer process...");
        tracing::info!("  Binary: {:?}", analyzer_path);
        tracing::info!("  Workspace: {:?}", workspace_root);

        let spawn_result = Command::new(analyzer_path)
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

                // Try to install rust-analyzer
                match Self::install_rust_analyzer_to_deps() {
                    Ok(installed_path) => {
                        tracing::info!("✓ Successfully installed rust-analyzer, retrying spawn...");

                        // Retry spawning with the newly installed analyzer
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
                        return Err(anyhow!("Failed to spawn and install: spawn error: {}, install error: {}", e, install_err));
                    }
                }
            }
        };

        let pid = child.id();
        tracing::info!("✓ rust-analyzer process spawned (PID: {})", pid);

        // Take stdin for our use
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("Failed to take stdin"))?;

        // Monitor stderr in a thread
        if let Some(stderr) = child.stderr.take() {
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().flatten() {
                    tracing::error!("[rust-analyzer stderr] {}", line);
                }
                tracing::error!("❌ rust-analyzer stderr stream ended");
            });
        }

        // Monitor stdout for LSP messages in a thread
        if let Some(stdout) = child.stdout.take() {
            let progress_tx_stdout = progress_tx.clone();
            let pending_requests_clone = Arc::clone(&pending_requests);
            thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut buffer = String::new();

                loop {
                    buffer.clear();
                    
                    // Read Content-Length header
                    if reader.read_line(&mut buffer).is_err() || buffer.is_empty() {
                        break;
                    }
                    
                    if !buffer.starts_with("Content-Length:") {
                        continue;
                    }
                    
                    let content_len: usize = match buffer
                        .trim_start_matches("Content-Length:")
                        .trim()
                        .parse()
                    {
                        Ok(len) => len,
                        Err(_) => continue,
                    };
                    
                    // Read empty line
                    buffer.clear();
                    if reader.read_line(&mut buffer).is_err() {
                        break;
                    }
                    
                    // Read the JSON content
                    let mut content_buffer = vec![0u8; content_len];
                    if let Ok(_) = std::io::Read::read_exact(&mut reader, &mut content_buffer) {
                        if let Ok(content) = String::from_utf8(content_buffer) {
                            // Try to parse as JSON first
                            if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&content) {
                                // Check if this is a response to a pending request
                                if let Some(id) = msg.get("id").and_then(|id| id.as_i64()) {
                                    if let Ok(mut pending) = pending_requests_clone.lock() {
                                        if let Some(tx) = pending.remove(&id) {
                                            let _ = tx.send(msg.clone());
                                            continue; // Don't process as a notification
                                        }
                                    }
                                }
                            }
                            
                            // Otherwise handle as notification/progress
                            Self::handle_lsp_message(&content, &progress_tx_stdout);
                        }
                    }
                }
                tracing::error!("❌ rust-analyzer stdout stream ended");
            });
        }

        // Store stdin and process
        {
            let mut stdin_lock = stdin_arc.lock().unwrap();
            *stdin_lock = Some(stdin);
        }

        // Monitor process exit in a separate thread
        let progress_tx_exit = progress_tx.clone();
        thread::spawn(move || {
            match child.wait() {
                Ok(status) => {
                    tracing::info!("❌ rust-analyzer exited with status: {:?}", status);
                    let _ = progress_tx_exit.send(ProgressUpdate::ProcessExited(status));
                }
                Err(e) => {
                    tracing::error!("❌ Failed to wait for rust-analyzer: {}", e);
                    let _ = progress_tx_exit.send(ProgressUpdate::Error(format!("Wait failed: {}", e)));
                }
            }
        });

        {
            let process_lock = process_arc.lock().unwrap();
            // Note: We can't store the child here since we already called wait() on it in another thread
            // This is intentional - the monitoring thread owns the child
        }

        Ok(())
    }

    fn send_initialize_request(
        workspace_root: &PathBuf,
        stdin_arc: Arc<Mutex<Option<std::process::ChildStdin>>>,
        request_id_arc: Arc<Mutex<i64>>,
    ) -> Result<()> {
        // Normalize the workspace path for Windows
        let workspace_str = workspace_root.to_string_lossy().replace("\\", "/");
        let uri = if workspace_str.starts_with("C:/") || workspace_str.starts_with("c:/") {
            format!("file:///{}", workspace_str)
        } else {
            format!("file://{}", workspace_str)
        };

        tracing::info!("  Using workspace URI: {}", uri);

        let mut req_id = request_id_arc.lock().map_err(|e| anyhow!("Lock error: {}", e))?;
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
                        "workspaceFolders": true
                    },
                    "textDocument": {
                        "completion": {
                            "completionItem": {
                                "snippetSupport": true,
                                "resolveSupport": {
                                    "properties": ["documentation", "detail", "additionalTextEdits"]
                                }
                            }
                        },
                        "hover": {
                            "contentFormat": ["plaintext", "markdown"]
                        }
                    },
                    "window": {
                        "workDoneProgress": true
                    },
                    "experimental": {
                        "serverStatusNotification": true
                    }
                },
                "initializationOptions": {
                    "checkOnSave": {
                        "command": "clippy"
                    },
                    "cargo": {
                        "loadOutDirsFromCheck": true
                    },
                    "procMacro": {
                        "enable": true
                    }
                }
            }
        });

        let mut stdin_lock = stdin_arc.lock().map_err(|e| anyhow!("Lock error: {}", e))?;
        if let Some(stdin) = stdin_lock.as_mut() {
            let content = serde_json::to_string(&init_request)?;
            let message = format!("Content-Length: {}\r\n\r\n{}", content.len(), content);

            stdin.write_all(message.as_bytes())?;
            stdin.flush()?;

            tracing::info!("✓ Sent initialize request to rust-analyzer");

            // Send initialized notification
            let initialized_notification = json!({
                "jsonrpc": "2.0",
                "method": "initialized",
                "params": {}
            });

            let content = serde_json::to_string(&initialized_notification)?;
            let message = format!("Content-Length: {}\r\n\r\n{}", content.len(), content);

            stdin.write_all(message.as_bytes())?;
            stdin.flush()?;

            tracing::info!("✓ Sent initialized notification");
        } else {
            return Err(anyhow!("stdin not available"));
        }

        Ok(())
    }

    fn handle_lsp_message(content: &str, progress_tx: &Sender<ProgressUpdate>) {
        // Try to parse as JSON
        if let Ok(msg) = serde_json::from_str::<Value>(content) {
            // Check if it's a response to a request
            if let Some(id) = msg.get("id").and_then(|id| id.as_i64()) {
                // This is a response - we handle it via the pending_requests mechanism
                // The send_request method handles receiving responses via channels
                return;
            }
            
            if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
                match method {
                    "$/progress" => {
                        // Handle progress notifications
                        if let Some(params) = msg.get("params") {
                            // Get the progress token to identify what type of progress this is
                            let token = params.get("token").and_then(|t| t.as_str()).unwrap_or("");
                            
                            if let Some(value) = params.get("value") {
                                if let Some(kind) = value.get("kind").and_then(|k| k.as_str()) {
                                    match kind {
                                        "begin" => {
                                            let title = value.get("title").and_then(|t| t.as_str()).unwrap_or("Processing");
                                            
                                            // Extract meaningful information from the title
                                            let message = if title.contains("Fetching") || title.contains("Loading") {
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
                                            let message = value.get("message").and_then(|m| m.as_str()).unwrap_or("");
                                            let percentage = value.get("percentage").and_then(|p| p.as_u64()).unwrap_or(0);
                                            
                                            // Extract crate name from the message if present
                                            let display_message = if !message.is_empty() {
                                                // Parse messages like "12/45" or crate names
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
                                        "end" => {
                                            let message = value.get("message").and_then(|m| m.as_str()).unwrap_or("");
                                            
                                            // DON'T mark as ready here - cachePriming can complete and restart multiple times!
                                            // Instead, wait for the rust-analyzer/serverStatus notification with quiescent: true
                                            // That's the ONLY reliable way to know when ALL analysis is complete
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    "textDocument/publishDiagnostics" => {
                        // Handle diagnostic notifications
                        if let Some(params) = msg.get("params") {
                            if let Some(diagnostics_array) = params.get("diagnostics").and_then(|d| d.as_array()) {
                                if let Some(uri) = params.get("uri").and_then(|u| u.as_str()) {
                                    let mut diagnostics = Vec::new();
                                    
                                    for diag in diagnostics_array {
                                        if let (Some(range), Some(message)) = (
                                            diag.get("range"),
                                            diag.get("message").and_then(|m| m.as_str())
                                        ) {
                                            if let (Some(start), Some(severity_num)) = (
                                                range.get("start"),
                                                diag.get("severity").and_then(|s| s.as_u64())
                                            ) {
                                                let line = start.get("line").and_then(|l| l.as_u64()).unwrap_or(0) as usize + 1;
                                                let column = start.get("character").and_then(|c| c.as_u64()).unwrap_or(0) as usize + 1;
                                                
                                                let severity = match severity_num {
                                                    1 => DiagnosticSeverity::Error,
                                                    2 => DiagnosticSeverity::Warning,
                                                    3 => DiagnosticSeverity::Information,
                                                    4 => DiagnosticSeverity::Hint,
                                                    _ => DiagnosticSeverity::Information,
                                                };
                                                
                                                let file_path = uri.trim_start_matches("file:///").replace("%20", " ");
                                                
                                                diagnostics.push(Diagnostic {
                                                    file_path,
                                                    line,
                                                    column,
                                                    severity,
                                                    message: message.to_string(),
                                                    code: None,
                                                    source: Some("rust-analyzer".to_string()),
                                                });
                                            }
                                        }
                                    }
                                    
                                    if !diagnostics.is_empty() {
                                        let _ = progress_tx.send(ProgressUpdate::Diagnostics(diagnostics));
                                    }
                                }
                            }
                        }
                    }
                    "window/workDoneProgress/create" => {
                        tracing::info!("📊 Work done progress created");
                    }
                    "rust-analyzer/serverStatus" => {
                        // rust-analyzer sends this when its status changes
                        // params: { health: "ok" | "warning" | "error", quiescent: bool, message?: string }
                        // quiescent = true means the server is idle (all background work done)
                        if let Some(params) = msg.get("params") {
                            tracing::info!("🔔 rust-analyzer/serverStatus: {:?}", params);
                            
                            // Check if the server is quiescent (idle, all indexing done)
                            if let Some(quiescent) = params.get("quiescent").and_then(|q| q.as_bool()) {
                                if quiescent {
                                    tracing::info!("✅ rust-analyzer is quiescent (all indexing complete)");
                                    let _ = progress_tx.send(ProgressUpdate::Ready);
                                }
                            }
                        }
                    }
                    _ => {
                        // Log unhandled notifications to help with debugging
                        tracing::info!("📨 Unhandled LSP notification: {}", method);
                    }
                }
            }
        }
    }

    /// Stop rust-analyzer
    pub fn stop(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!("🛑 Stopping rust-analyzer");
        self.stop_internal();
        self.status = AnalyzerStatus::Stopped;
        cx.emit(AnalyzerEvent::StatusChanged(AnalyzerStatus::Stopped));
        cx.notify();
    }

    fn stop_internal(&mut self) {
        // Close stdin first
        {
            let mut stdin_lock = self.stdin.lock().unwrap();
            *stdin_lock = None;
        }

        // Clear pending requests
        {
            let mut pending = self.pending_requests.lock().unwrap();
            pending.clear();
        }

        // Then kill the process
        let mut process_lock = self.process.lock().unwrap();
        if let Some(mut child) = process_lock.take() {
            let _ = child.kill();
            let _ = child.wait();
            tracing::info!("✓ rust-analyzer process terminated");
        }
        self.initialized = false;
        self.progress_rx = None;
    }

    /// Restart rust-analyzer
    pub fn restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!("🔄 Restarting rust-analyzer");
        if let Some(workspace) = self.workspace_root.clone() {
            self.stop(window, cx);
            // Give it a moment to clean up
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
            AnalyzerStatus::Starting
                | AnalyzerStatus::Indexing { .. }
                | AnalyzerStatus::Ready
        )
    }

    /// Send didOpen notification for a file
    pub fn did_open_file(&self, file_path: &PathBuf, content: &str, language_id: &str) -> Result<()> {
        if !self.is_running() {
            return Ok(()); // Silently ignore if not running
        }

        let uri = self.path_to_uri(file_path);
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
            return Ok(()); // Silently ignore if not running
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
            return Ok(()); // Silently ignore if not running
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

        tracing::info!("💾 Notifying rust-analyzer of file save: {:?}", file_path);
        self.send_notification(notification)
    }

    /// Send didClose notification for a file
    pub fn did_close_file(&self, file_path: &PathBuf) -> Result<()> {
        if !self.is_running() {
            return Ok(()); // Silently ignore if not running
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

    /// Convert a file path to a URI
    fn path_to_uri(&self, path: &PathBuf) -> String {
        let path_str = path.to_string_lossy().replace("\\", "/");
        if path_str.starts_with("C:/") || path_str.starts_with("c:/") {
            format!("file:///{}", path_str)
        } else {
            format!("file://{}", path_str)
        }
    }

    /// Send a notification to rust-analyzer
    fn send_notification(&self, notification: Value) -> Result<()> {
        let mut stdin_lock = self.stdin.lock().map_err(|e| anyhow!("Lock error: {}", e))?;
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

    /// Send a request to rust-analyzer and wait for response asynchronously
    /// Returns a Task that resolves when the response is received
    pub fn send_request_async(&self, method: &str, params: Value) -> Result<flume::Receiver<Value>> {
        if !self.is_running() {
            return Err(anyhow!("rust-analyzer is not running"));
        }

        // Generate request ID
        let mut req_id = self.request_id.lock().map_err(|e| anyhow!("Lock error: {}", e))?;
        *req_id += 1;
        let id = *req_id;
        drop(req_id);

        // Create channel for this request's response (flume for async)
        let (response_tx, response_rx) = flume::unbounded();

        // Register the pending request
        {
            let mut pending = self.pending_requests.lock().map_err(|e| anyhow!("Lock error: {}", e))?;
            pending.insert(id, response_tx);
        }

        // Send the request
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        let mut stdin_lock = self.stdin.lock().map_err(|e| anyhow!("Lock error: {}", e))?;
        if let Some(stdin) = stdin_lock.as_mut() {
            let content = serde_json::to_string(&request)?;
            let message = format!("Content-Length: {}\r\n\r\n{}", content.len(), content);

            stdin.write_all(message.as_bytes())?;
            stdin.flush()?;
        } else {
            // Remove from pending since we failed
            let mut pending = self.pending_requests.lock().map_err(|e| anyhow!("Lock error: {}", e))?;
            pending.remove(&id);
            return Err(anyhow!("stdin not available"));
        }
        drop(stdin_lock);

        // Return the receiver for async awaiting
        Ok(response_rx)
    }

    /// Send a request to rust-analyzer and wait for response (blocking, for legacy compatibility)
    /// DEPRECATED: Use send_request_async for better performance
    pub fn send_request(&self, method: &str, params: Value) -> Result<Value> {
        let rx = self.send_request_async(method, params)?;
        
        // Wait for response with timeout (blocking!)
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(response) => Ok(response),
            Err(e) => {
                Err(anyhow!("Request timeout: {}", e))
            }
        }
    }

    /// Update progress from the background thread (called from UI thread periodically)
    pub fn update_progress_from_thread(&mut self, cx: &mut Context<Self>) {
        // Collect all updates first to avoid borrow issues
        let mut updates = Vec::new();
        if let Some(rx) = &self.progress_rx {
            // Drain all available messages
            while let Ok(update) = rx.try_recv() {
                updates.push(update);
            }
        }
        
        // Handle all collected updates
        for update in updates {
            self.handle_progress_update(update, cx);
        }
        
        // Additional check: if we've been indexing for a while and receiving updates,
        // but haven't marked as complete, check if we should timeout to ready
        if !self.initial_analysis_complete {
            if let Some(last_update) = self.last_update {
                // If no updates for 3 seconds and we're in indexing state, mark as ready
                // This handles cases where rust-analyzer doesn't send an explicit "end" progress event
                if last_update.elapsed() > Duration::from_secs(3) && matches!(self.status, AnalyzerStatus::Indexing { .. }) {
                    self.initial_analysis_complete = true;
                    self.status = AnalyzerStatus::Ready;
                    tracing::info!("✅ Initial analysis complete (timeout - no updates for 3s)");
                    cx.emit(AnalyzerEvent::Ready);
                    cx.notify();
                }
            }
        }
    }
    
    /// Handle a progress update from the background thread
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
                    tracing::info!("✅ Initial analysis marked as complete");
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
                tracing::info!("❌ {}", error_msg);
                self.status = AnalyzerStatus::Error(error_msg.clone());
                self.initialized = false;
                cx.emit(AnalyzerEvent::Error(error_msg));
                cx.notify();
            }
            ProgressUpdate::Diagnostics(diagnostics) => {
                // Track when we first receive diagnostics - this indicates the analyzer is working
                if self.first_diagnostics_time.is_none() {
                    self.first_diagnostics_time = Some(Instant::now());
                    tracing::info!("📊 First diagnostics received - analyzer is working");
                }
                
                // If we've been receiving diagnostics for more than 2 seconds and haven't marked as ready,
                // consider the initial analysis complete
                if let Some(first_time) = self.first_diagnostics_time {
                    if !self.initial_analysis_complete 
                        && first_time.elapsed() > Duration::from_secs(2) {
                        self.initial_analysis_complete = true;
                        self.status = AnalyzerStatus::Ready;
                        tracing::info!("✅ Initial analysis complete based on diagnostics (received for 2s)");
                        cx.emit(AnalyzerEvent::Ready);
                        cx.notify();
                    }
                }
                
                cx.emit(AnalyzerEvent::Diagnostics(diagnostics));
                // Don't notify here, let the app handle it
            }
        }
    }

    /// Request hover information at a specific position
    pub fn hover(
        &self,
        file_path: &PathBuf,
        line: usize,
        column: usize,
    ) -> Result<Value> {
        if !self.is_running() {
            return Err(anyhow!("rust-analyzer is not running"));
        }

        let uri = self.path_to_uri(file_path);
        let params = json!({
            "textDocument": {
                "uri": uri
            },
            "position": {
                "line": line.saturating_sub(1), // LSP uses 0-based lines
                "character": column.saturating_sub(1) // LSP uses 0-based columns
            }
        });

        self.send_request("textDocument/hover", params)
    }

    /// Request go-to-definition at a specific position
    pub fn definition(
        &self,
        file_path: &PathBuf,
        line: usize,
        column: usize,
    ) -> Result<Value> {
        if !self.is_running() {
            return Err(anyhow!("rust-analyzer is not running"));
        }

        let uri = self.path_to_uri(file_path);
        let params = json!({
            "textDocument": {
                "uri": uri
            },
            "position": {
                "line": line.saturating_sub(1), // LSP uses 0-based lines
                "character": column.saturating_sub(1) // LSP uses 0-based columns
            }
        });

        self.send_request("textDocument/definition", params)
    }
}

impl Drop for RustAnalyzerManager {
    fn drop(&mut self) {
        self.stop_internal();
    }
}

