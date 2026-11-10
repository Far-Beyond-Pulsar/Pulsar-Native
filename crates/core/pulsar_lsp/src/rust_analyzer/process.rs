//! Process lifecycle: spawning rust-analyzer, the `initialize` handshake,
//! the stdout reader thread, stop and restart. Entry points:
//! [`RustAnalyzerManager::start`], `stop`, `restart`.
use anyhow::{anyhow, Result};
use gpui::{Context, Window};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::{
    trigger_characters_in, AnalyzerEvent, AnalyzerStatus, ProgressUpdate, RustAnalyzerManager,
};

impl RustAnalyzerManager {
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
                        manager.last_progress_emit = Some(Instant::now());
                        manager.last_emitted_progress = Some(0.0);
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

    pub(super) fn spawn_process_blocking(
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
                                            let triggers = trigger_characters_in(&msg);
                                            let _ = progress_tx_stdout
                                                .send(ProgressUpdate::CompletionTriggers(triggers));
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

    pub(super) fn send_initialize_request(
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

    /// Stop rust-analyzer
    pub fn stop(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        tracing::debug!("🛑 Stopping rust-analyzer");
        self.stop_internal();
        self.status = AnalyzerStatus::Stopped;
        cx.emit(AnalyzerEvent::StatusChanged(AnalyzerStatus::Stopped));
        cx.notify();
    }

    pub(super) fn stop_internal(&mut self) {
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
        // A new process knows no documents and has published nothing yet.
        if let Ok(mut open) = self.open_documents.lock() {
            open.clear();
        }
        self.file_diagnostics.clear();
        self.completion_triggers.clear();
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
}
