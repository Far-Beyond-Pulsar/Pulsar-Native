//! Incoming server messages: progress, diagnostics and server-status
//! notifications parsed into [`ProgressUpdate`]s, plus the diagnostic and
//! quick-fix parsing they need. Entry point: `handle_lsp_message`.
use serde_json::Value;
use std::sync::mpsc::Sender;
use ui::diagnostics::{CodeAction, Diagnostic, DiagnosticSeverity, TextEdit};

use super::path_utils;
use super::{ProgressUpdate, RustAnalyzerManager};

impl RustAnalyzerManager {
    pub(super) fn handle_lsp_message(content: &str, progress_tx: &Sender<ProgressUpdate>) {
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

    pub(super) fn handle_progress_notification(msg: &Value, progress_tx: &Sender<ProgressUpdate>) {
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

    pub(super) fn handle_diagnostics_notification(
        msg: &Value,
        progress_tx: &Sender<ProgressUpdate>,
    ) {
        if let Some(params) = msg.get("params") {
            if let Some(diagnostics_array) = params.get("diagnostics").and_then(|d| d.as_array()) {
                if let Some(uri) = params.get("uri").and_then(|u| u.as_str()) {
                    let file_path = uri.trim_start_matches("file:///").replace("%20", " ");
                    let diagnostics: Vec<Diagnostic> = diagnostics_array
                        .iter()
                        .filter_map(|diag| Self::parse_diagnostic(diag, &file_path))
                        .collect();

                    let _ = progress_tx.send(ProgressUpdate::FileDiagnostics {
                        key: path_utils::uri_key(uri),
                        diagnostics: diagnostics.clone(),
                    });

                    if !diagnostics.is_empty() {
                        let _ = progress_tx.send(ProgressUpdate::Diagnostics(diagnostics));
                    }
                }
            }
        }
    }

    pub(super) fn parse_diagnostic(diag: &Value, file_path: &str) -> Option<Diagnostic> {
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

    pub(super) fn extract_code_actions_from_data(diag: &Value, code_actions: &mut Vec<CodeAction>) {
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

    pub(super) fn extract_text_edits_from_changes(edit: &Value, edits: &mut Vec<TextEdit>) {
        if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
            for (edit_uri, edit_array) in changes {
                if let Some(edit_list) = edit_array.as_array() {
                    let edit_file = edit_uri.trim_start_matches("file:///").replace("%20", " ");
                    for text_edit in edit_list {
                        if let Some(te) = Self::parse_text_edit(text_edit, &edit_file) {
                            edits.push(te);
                        }
                    }
                }
            }
        }
    }

    pub(super) fn extract_text_edits_from_doc_changes(edit: &Value, edits: &mut Vec<TextEdit>) {
        if let Some(doc_changes) = edit.get("documentChanges").and_then(|c| c.as_array()) {
            for doc_change in doc_changes {
                if let Some(text_doc) = doc_change.get("textDocument") {
                    let edit_file = text_doc
                        .get("uri")
                        .and_then(|u| u.as_str())
                        .map(|u| u.trim_start_matches("file:///").replace("%20", " "))
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

    pub(super) fn parse_text_edit(text_edit: &Value, file_path: &str) -> Option<TextEdit> {
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

    pub(super) fn extract_code_actions_from_related_info(
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
                                        .map(|u| {
                                            u.trim_start_matches("file:///").replace("%20", " ")
                                        })
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

    pub(super) fn handle_server_status(msg: &Value, progress_tx: &Sender<ProgressUpdate>) {
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
}
