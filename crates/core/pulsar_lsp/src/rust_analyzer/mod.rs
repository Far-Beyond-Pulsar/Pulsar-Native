//! Rust Analyzer LSP process manager.
//!
//! Handles process lifecycle, the LSP JSON-RPC protocol loop, progress
//! reporting, and all `textDocument/*` request helpers.  The GPUI
//! `EventEmitter` implementation emits [`AnalyzerEvent`] so that UI
//! components can subscribe to indexing progress and diagnostics.

mod discovery;
mod document_sync;
mod notifications;
mod path_utils;
mod process;
mod progress;
mod requests;
mod transport;
pub use path_utils::{path_key, path_to_uri, uri_key, uri_to_path};

use gpui::{Context, EventEmitter, Window};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, ExitStatus};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use ui::diagnostics::Diagnostic;

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
    IndexingProgress {
        progress: f32,
        message: String,
    },
    Ready,
    Error(String),
    Diagnostics(Vec<Diagnostic>),
    /// The complete diagnostic set for one file, published whenever the
    /// server re-analyses it. Unlike [`AnalyzerEvent::Diagnostics`] this is
    /// also sent when the set becomes empty, so a fixed error clears.
    /// `key` identifies the file for comparison with [`path_utils::path_key`].
    FileDiagnostics {
        key: String,
        diagnostics: Vec<Diagnostic>,
    },
}

#[derive(Debug)]
enum ProgressUpdate {
    Progress {
        progress: f32,
        message: String,
    },
    Ready,
    Error(String),
    ProcessExited(ExitStatus),
    Diagnostics(Vec<Diagnostic>),
    FileDiagnostics {
        key: String,
        diagnostics: Vec<Diagnostic>,
    },
    /// The server's own `completionProvider.triggerCharacters`.
    CompletionTriggers(Vec<char>),
}

/// Minimum wall time between emitted `IndexingProgress` events. Raw updates
/// arrive far faster than a subscriber can usefully repaint, and each emission
/// costs a full notify up the ancestor chain.
const PROGRESS_EMIT_MIN_INTERVAL: Duration = Duration::from_millis(100);

/// An emission bypasses the time window whenever progress crosses a multiple
/// of this percentage, so the reported value stays within one visible step of
/// reality even during slow indexing.
const PROGRESS_EMIT_MILESTONE_STEP: f32 = 5.0;

/// The completion trigger characters in an `initialize` reply.
fn trigger_characters_in(reply: &Value) -> Vec<char> {
    reply
        .pointer("/result/capabilities/completionProvider/triggerCharacters")
        .and_then(|value| value.as_array())
        .map(|chars| {
            chars
                .iter()
                .filter_map(|c| c.as_str()?.chars().next())
                .collect()
        })
        .unwrap_or_default()
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
    /// Time of the last emitted `IndexingProgress`, for coalescing.
    last_progress_emit: Option<Instant>,
    /// Progress percentage last emitted to subscribers; milestone crossings
    /// are detected against this so suppressed updates can still end at an
    /// observable final value.
    last_emitted_progress: Option<f32>,
    /// Editors that currently hold each document open. The server sees one
    /// `didOpen` for the first and one `didClose` after the last, however many
    /// editors show the file.
    open_documents: Arc<Mutex<HashMap<String, usize>>>,
    /// Latest diagnostics per file key, so an editor opened later shows them
    /// without waiting for the next analysis.
    file_diagnostics: HashMap<String, Vec<Diagnostic>>,
    /// Characters the server asked to be offered completion on (from its
    /// initialize reply). Empty until that reply arrives.
    completion_triggers: Vec<char>,
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
            last_progress_emit: None,
            last_emitted_progress: None,
            open_documents: Arc::new(Mutex::new(HashMap::new())),
            file_diagnostics: HashMap::new(),
            completion_triggers: Vec::new(),
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

    /// Check if analyzer is initialized: the `initialize` request (and the
    /// `initialized` notification) are on the wire ahead of anything else, so
    /// document operations the server receives from now on follow them.
    /// That is the transition to Indexing status, which `start` makes only
    /// after the spawn thread has written the request.
    pub fn is_initialized(&self) -> bool {
        matches!(
            self.status,
            AnalyzerStatus::Indexing { .. } | AnalyzerStatus::Ready
        )
    }
}

impl Drop for RustAnalyzerManager {
    fn drop(&mut self) {
        self.stop_internal();
    }
}

#[cfg(test)]
mod trigger_tests {
    use super::*;

    #[test]
    fn the_servers_registered_trigger_characters_are_read_from_initialize() {
        let reply = json!({ "id": 17, "result": { "capabilities": { "completionProvider": {
            "triggerCharacters": [":", ".", "'", "("]
        } } } });
        assert_eq!(trigger_characters_in(&reply), vec![':', '.', '\x27', '(']);
        assert!(trigger_characters_in(&json!({ "result": { "capabilities": {} } })).is_empty());
        assert!(trigger_characters_in(&json!({ "error": {} })).is_empty());
    }
}
