//! JSON-RPC transport: framing and writing notifications and requests to the
//! server's stdin and delivering responses to waiting callers. Entry points:
//! `send_notification`, `send_request`, `send_request_async`.
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use super::path_utils;
use super::RustAnalyzerManager;

impl RustAnalyzerManager {
    pub(super) fn path_to_uri(&self, path: &PathBuf) -> String {
        path_utils::path_to_uri(path)
    }

    pub(super) fn send_notification(&self, notification: Value) -> Result<()> {
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
}
