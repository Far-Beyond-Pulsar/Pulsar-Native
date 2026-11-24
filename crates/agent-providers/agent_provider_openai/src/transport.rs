//! The blocking HTTP client and the stream-level error both wire formats share.

use anyhow::Context;
use reqwest::blocking::Client;
use serde_json::Value;
use std::time::Duration;

/// The blocking client. `Client::new()` gives every wait 30 s, which the
/// first byte of a long generation exceeds. The blocking client applies
/// `timeout` to each wait for the server (the response, then each body
/// read), not to the whole request, so this keeps a slow stream open as long
/// as the server keeps sending, and still fails a stalled one.
pub(crate) fn http_client() -> anyhow::Result<Client> {
    Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()
        .context("building HTTP client")
}

/// The message of a top-level `error` a server sends as a stream event
/// instead of an HTTP error status.
pub(crate) fn stream_error(event: &Value) -> Option<String> {
    let error = event.get("error")?;
    Some(match error {
        Value::Null => return None,
        Value::String(message) => message.clone(),
        Value::Object(fields) => fields
            .get("message")
            .and_then(|m| m.as_str())
            .map_or_else(|| error.to_string(), str::to_string),
        _ => error.to_string(),
    })
}
