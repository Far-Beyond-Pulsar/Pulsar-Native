//! What the UI sends to a running server: document sync and requests.

use std::path::Path;
use std::time::Instant;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::code_actions;
use crate::documents::Notifications;
use crate::path_utils::path_to_uri;

use super::*;

impl LanguageServer {
    pub(super) fn notify(&self, method: &str, params: Value) -> Result<()> {
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| anyhow!("{} is not running", self.spec.display_name))?;
        connection.notify(method, params)?;
        Ok(())
    }

    pub(super) fn send_all(&self, notifications: Notifications) {
        for (method, params) in notifications.0 {
            if let Err(error) = self.notify(method, params) {
                tracing::debug!("[{}] {method} not sent: {error}", self.id.server);
            }
        }
    }

    /// An editor opened `path`. The server sees one `didOpen` however many
    /// editors show the file; before it has answered `initialize` the open
    /// is held and sent when it has.
    pub fn did_open_file(&self, path: &Path, content: &str, language_id: &str) -> Result<()> {
        if !self.is_running() {
            return Err(anyhow!("{} is not running", self.spec.display_name));
        }
        let notifications = self
            .documents
            .lock()
            .map_err(|_| anyhow!("documents lock poisoned"))?
            .open(path, content, language_id, self.is_initialized());
        self.send_all(notifications);
        Ok(())
    }

    /// The `languageId` this server uses for `path`.
    pub fn language_id_of(&self, path: &Path) -> &'static str {
        self.language.language_id_of(path)
    }

    pub fn did_change_file(&self, path: &Path, content: &str, version: i32) -> Result<()> {
        if !self.is_running() {
            return Ok(());
        }
        let notifications = self
            .documents
            .lock()
            .map_err(|_| anyhow!("documents lock poisoned"))?
            .change_full(path, content, version);
        self.send_all(notifications);
        Ok(())
    }

    /// An incremental change (`range` + `text`, in order). `Err` means the
    /// server cannot take it (it only accepts full text, or has not been told
    /// of the document yet): send the whole text with
    /// [`LanguageServer::did_change_file`].
    pub fn did_change_file_incremental(
        &self,
        path: &Path,
        version: i32,
        changes: Vec<Value>,
    ) -> Result<()> {
        if !self.is_running() {
            return Ok(());
        }
        if !self.supports_incremental_sync() {
            return Err(anyhow!("the server takes full-text changes only"));
        }
        let notifications = self
            .documents
            .lock()
            .map_err(|_| anyhow!("documents lock poisoned"))?
            .change_incremental(path, version, changes)
            .ok_or_else(|| anyhow!("the server has not been told of this document yet"))?;
        self.send_all(notifications);
        Ok(())
    }

    pub fn did_save_file(&self, path: &Path, content: &str) -> Result<()> {
        if !self.is_running() {
            return Ok(());
        }
        let notifications = self
            .documents
            .lock()
            .map_err(|_| anyhow!("documents lock poisoned"))?
            .saved(path, content);
        self.send_all(notifications);
        Ok(())
    }

    pub fn did_close_file(&self, path: &Path) -> Result<()> {
        let notifications = self
            .documents
            .lock()
            .map_err(|_| anyhow!("documents lock poisoned"))?
            .close(path);
        self.send_all(notifications);
        Ok(())
    }

    /// Whether an editor has `path` open here.
    pub fn is_document_open(&self, path: &Path) -> bool {
        self.documents
            .lock()
            .map(|docs| docs.is_open(path))
            .unwrap_or(false)
    }

    /// Whether the server has been told of `path` (so requests about it are
    /// meaningful).
    pub fn is_document_announced(&self, path: &Path) -> bool {
        self.documents
            .lock()
            .map(|docs| docs.is_announced(path))
            .unwrap_or(false)
    }

    /// Record that editors are active (resets the idle clock).
    pub fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    // ---- requests ---------------------------------------------------------

    /// Send a request; the receiver yields the whole response message
    /// (`result` or `error`) or disconnects if the server goes away first.
    pub fn send_request_async(
        &self,
        method: &str,
        params: Value,
    ) -> Result<flume::Receiver<Value>> {
        if !self.is_initialized() {
            return Err(anyhow!("{} is not running", self.spec.display_name));
        }
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| anyhow!("{} is not running", self.spec.display_name))?;
        Ok(connection.request(method, params)?)
    }

    /// Code actions for a diagnostic, using the raw diagnostic the server
    /// published (it carries the server's own `data`).
    pub fn request_code_actions_with_diagnostic(
        &self,
        file_path: &Path,
        raw_diagnostic: &Value,
    ) -> Result<flume::Receiver<Value>> {
        let range = raw_diagnostic.get("range").cloned().unwrap_or_else(|| {
            json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } })
        });
        self.send_request_async(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": path_to_uri(file_path) },
                "range": range,
                "context": {
                    "diagnostics": [raw_diagnostic],
                    "only": ["quickfix"],
                    "triggerKind": 1
                }
            }),
        )
    }

    /// Resolve a code action (fetch its edits).
    pub fn resolve_code_action_async(&self, action: &Value) -> Result<flume::Receiver<Value>> {
        self.send_request_async("codeAction/resolve", action.clone())
    }

    /// The code actions with edits in a `codeAction` response, and those that
    /// still need resolving.
    pub fn split_code_actions(response: &Value) -> (Vec<ui::diagnostics::CodeAction>, Vec<Value>) {
        let result = response.get("result").unwrap_or(&Value::Null);
        (
            code_actions::parse_code_actions(result),
            code_actions::get_unresolved_actions(result),
        )
    }
}
