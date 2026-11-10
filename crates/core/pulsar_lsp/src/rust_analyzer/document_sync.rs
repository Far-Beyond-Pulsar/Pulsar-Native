//! Document synchronisation: didOpen / didChange / didSave / didClose and
//! the open-document bookkeeping behind them.
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use ui::diagnostics::Diagnostic;

use super::path_utils;
use super::RustAnalyzerManager;

impl RustAnalyzerManager {
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

        let key = path_utils::path_key(file_path);
        {
            let mut open = self
                .open_documents
                .lock()
                .map_err(|e| anyhow!("Lock error: {}", e))?;
            let holders = open.entry(key).or_insert(0);
            *holders += 1;
            if *holders > 1 {
                // Already open for another editor: the server has the text.
                return Ok(());
            }
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

        let sent = self.send_notification(notification);
        if sent.is_err() {
            // The server never saw it, so this editor does not hold it open.
            if let Ok(mut open) = self.open_documents.lock() {
                let key = path_utils::path_key(file_path);
                if let Some(holders) = open.get_mut(&key) {
                    *holders = holders.saturating_sub(1);
                    if *holders == 0 {
                        open.remove(&key);
                    }
                }
            }
        }
        sent
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

    /// Send an incremental didChange: `changes` are LSP content-change
    /// objects (`range` + `text`) applied in order. Costs only the edit, not
    /// the document.
    pub fn did_change_file_incremental(
        &self,
        file_path: &PathBuf,
        version: i32,
        changes: Vec<Value>,
    ) -> Result<()> {
        if !self.is_running() || changes.is_empty() {
            return Ok(());
        }

        let uri = self.path_to_uri(file_path);
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": changes
            }
        });

        self.send_notification(notification)
    }

    /// Whether the server has `file_path` open (an editor called
    /// [`Self::did_open_file`] and none has closed it since).
    pub fn is_document_open(&self, file_path: &PathBuf) -> bool {
        let key = path_utils::path_key(file_path);
        self.open_documents
            .lock()
            .map(|open| open.contains_key(&key))
            .unwrap_or(false)
    }

    /// The characters the server registered to open completion (empty until it
    /// has answered `initialize`).
    pub fn completion_trigger_characters(&self) -> Vec<char> {
        self.completion_triggers.clone()
    }

    /// The latest diagnostics the server published for `file_path`.
    pub fn diagnostics_for(&self, file_path: &PathBuf) -> Vec<Diagnostic> {
        self.file_diagnostics
            .get(&path_utils::path_key(file_path))
            .cloned()
            .unwrap_or_default()
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

        let key = path_utils::path_key(file_path);
        {
            let mut open = self
                .open_documents
                .lock()
                .map_err(|e| anyhow!("Lock error: {}", e))?;
            let remaining = match open.get_mut(&key) {
                Some(holders) => {
                    *holders = holders.saturating_sub(1);
                    *holders
                }
                None => return Ok(()),
            };
            if remaining > 0 {
                // Another editor still shows it: keep it open.
                return Ok(());
            }
            open.remove(&key);
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
}
