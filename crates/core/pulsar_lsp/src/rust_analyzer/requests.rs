//! `textDocument/*` request helpers: hover, definition and code actions
//! (request, parse, resolve).
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use ui::diagnostics::CodeAction;

use super::RustAnalyzerManager;

impl RustAnalyzerManager {
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
    pub(super) fn uri_to_path(uri: &str) -> String {
        let path = uri.trim_start_matches("file:///");
        path.replace("%20", " ")
            .replace("%3A", ":")
            .replace("%5C", "\\")
            .replace("%2F", "/")
            .replace("%23", "#")
            .replace("%25", "%")
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
