//! What the reader thread does with what the server sends, and decoding of the
//! server's answers.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::diagnostics::parse_publish;
use crate::path_utils::{path_to_uri, uri_to_path};
use crate::protocol::{Connection, Handler, RequestOutcome, METHOD_NOT_FOUND};
use pulsar_language::spec::{section_of, LanguageServerSpec, ProgressKind, ServerContext};

use super::*;

/// What the reader thread does with what the server sends.
pub(super) struct ServerHandler {
    pub(super) spec: &'static LanguageServerSpec,
    pub(super) context: Arc<ServerContext>,
    pub(super) tx: flume::Sender<Msg>,
    pub(super) connection: Arc<Connection>,
    pub(super) task: editor_task_queue::TaskContext,
    pub(super) label: String,
}

impl Handler for ServerHandler {
    fn request(&mut self, id: &Value, method: &str, params: &Value) -> RequestOutcome {
        self.task.set_idle(false);
        self.task.heartbeat();
        let outcome = match method {
            // Nothing to do but acknowledge: tsserver and others wait for it.
            "client/registerCapability"
            | "client/unregisterCapability"
            | "window/workDoneProgress/create"
            | "window/showMessageRequest"
            | "workspace/semanticTokens/refresh"
            | "workspace/inlayHint/refresh"
            | "workspace/codeLens/refresh"
            | "workspace/diagnostic/refresh" => RequestOutcome::Reply(Value::Null),
            "workspace/configuration" => {
                let sections: Vec<Value> = params
                    .get("items")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .map(|item| {
                                let section =
                                    item.get("section").and_then(Value::as_str).unwrap_or("");
                                (self.spec.configuration)(&self.context, section)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                RequestOutcome::Reply(Value::Array(sections))
            }
            "workspace/workspaceFolders" => {
                let uri = path_to_uri(&self.context.root);
                let name = self
                    .context
                    .root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                RequestOutcome::Reply(json!([{ "uri": uri, "name": name }]))
            }
            "window/showDocument" => RequestOutcome::Reply(json!({ "success": false })),
            "workspace/applyEdit" => {
                let _ = self.tx.send(Msg::ApplyEdit {
                    id: id.clone(),
                    params: params.clone(),
                });
                RequestOutcome::Deferred
            }
            other => RequestOutcome::Error(
                METHOD_NOT_FOUND,
                format!("Pulsar does not handle `{other}`"),
            ),
        };
        self.task.set_idle(true);
        outcome
    }

    fn notification(&mut self, method: &str, params: Value) {
        self.task.set_idle(false);
        self.task.heartbeat();
        match method {
            "$/progress" => {
                if let Some(message) = parse_progress(&params) {
                    let _ = self.tx.send(message);
                }
            }
            "textDocument/publishDiagnostics" => {
                if let Some((path, diagnostics)) =
                    parse_publish(&params, self.spec.id, &self.spec.hooks)
                {
                    let _ = self.tx.send(Msg::Diagnostics { path, diagnostics });
                }
            }
            "window/logMessage" | "window/showMessage" => {
                let text = params.get("message").and_then(Value::as_str).unwrap_or("");
                match params.get("type").and_then(Value::as_u64) {
                    Some(1) => tracing::error!("[{}] {text}", self.label),
                    Some(2) => tracing::warn!("[{}] {text}", self.label),
                    _ => tracing::info!("[{}] {text}", self.label),
                }
            }
            other => {
                if let Some(signal) = (self.spec.hooks.notification)(other, &params) {
                    let _ = self.tx.send(Msg::Signal(signal));
                } else {
                    tracing::debug!("[{}] unhandled notification {other}", self.label);
                }
            }
        }
        let _ = &self.connection;
        self.task.set_idle(true);
    }
}

fn parse_progress(params: &Value) -> Option<Msg> {
    let token = match params.get("token")? {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let value = params.get("value")?;
    let kind = match value.get("kind")?.as_str()? {
        "begin" => ProgressKind::Begin,
        "report" => ProgressKind::Report,
        "end" => ProgressKind::End,
        _ => return None,
    };
    Some(Msg::Progress {
        token,
        kind,
        title: value
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned),
        message: value
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned),
        percentage: value
            .get("percentage")
            .and_then(Value::as_u64)
            .map(|p| p as u32),
    })
}

/// Resolve the file a `Location`-shaped answer points at (shared by the
/// editor's command handling).
pub fn location_target(result: &Value) -> Option<(PathBuf, u32, u32)> {
    let item = match result {
        Value::Array(items) => items.first()?,
        Value::Object(_) => result,
        _ => return None,
    };
    let uri = item
        .get("targetUri")
        .or_else(|| item.get("uri"))?
        .as_str()?;
    let range = item
        .get("targetSelectionRange")
        .or_else(|| item.get("range"))?;
    let start = range.get("start")?;
    Some((
        uri_to_path(uri),
        start.get("line")?.as_u64()? as u32,
        start.get("character")?.as_u64()? as u32,
    ))
}

/// The configuration answer for a section when the spec has no opinion:
/// the section of its effective initialization options.
pub fn default_configuration(
    spec: &LanguageServerSpec,
    context: &ServerContext,
    section: &str,
) -> Value {
    section_of(&spec.effective_initialization_options(context), section)
}
