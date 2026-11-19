//! JSON-RPC over a byte stream: framing, the two id spaces, and the reader
//! loop that routes what a server sends.
//!
//! A message with a `method` is a request (it has an `id`) or a notification
//! (it has none); a message without a `method` is a response. The ids of the
//! requests *we* send live in [`Connection`]'s counter and its pending table;
//! the ids of requests the *server* sends are its own, are never looked up in
//! that table, and are only ever echoed back in our reply. (The old client
//! treated any message with an id as a response, so a server request whose id
//! equalled a pending one was delivered as that request's answer.)

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use pulsar_language::settings::TraceLevel;

/// What happens with the answer to a request we sent.
type ResponseCallback = Box<dyn FnOnce(Value) + Send>;

/// The write side of one server connection and the table of requests we are
/// waiting on.
pub struct Connection {
    writer: Mutex<Box<dyn Write + Send>>,
    next_id: AtomicI64,
    pending: Mutex<HashMap<i64, ResponseCallback>>,
    closed: AtomicBool,
    trace: AtomicU8,
    label: String,
}

impl Connection {
    pub fn new(writer: impl Write + Send + 'static, label: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            writer: Mutex::new(Box::new(writer)),
            next_id: AtomicI64::new(1),
            pending: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            trace: AtomicU8::new(0),
            label: label.into(),
        })
    }

    pub fn set_trace(&self, level: TraceLevel) {
        self.trace.store(level as u8, Ordering::Relaxed);
    }

    fn trace_level(&self) -> TraceLevel {
        match self.trace.load(Ordering::Relaxed) {
            1 => TraceLevel::Messages,
            2 => TraceLevel::Verbose,
            _ => TraceLevel::Off,
        }
    }

    fn trace(&self, direction: &str, message: &Value) {
        match self.trace_level() {
            TraceLevel::Off => {}
            TraceLevel::Messages => {
                let method = message.get("method").and_then(Value::as_str).unwrap_or("(response)");
                let id = message.get("id").map(Value::to_string).unwrap_or_default();
                tracing::info!("[{}] {direction} {method} {id}", self.label);
            }
            TraceLevel::Verbose => {
                tracing::info!("[{}] {direction} {message}", self.label);
            }
        }
    }

    /// Whether the connection was closed (by us or because the server went).
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Close the connection: later writes fail and every request still
    /// waiting is dropped, so its receiver sees a disconnect rather than
    /// waiting for an answer that cannot come.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        if let Ok(mut pending) = self.pending.lock() {
            pending.clear();
        }
    }

    fn write(&self, message: &Value) -> io::Result<()> {
        if self.is_closed() {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "the language server is not running"));
        }
        self.trace("->", message);
        let body = serde_json::to_string(message)?;
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("the connection's writer is poisoned"))?;
        write!(writer, "Content-Length: {}\r\n\r\n{body}", body.len())?;
        writer.flush()
    }

    /// Send a notification.
    pub fn notify(&self, method: &str, params: Value) -> io::Result<()> {
        self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    /// Send a request; `on_response` runs on the reader thread with the whole
    /// response message (`result` or `error`) when it arrives.
    pub fn request_with(
        &self,
        method: &str,
        params: Value,
        on_response: impl FnOnce(Value) + Send + 'static,
    ) -> io::Result<i64> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending
            .lock()
            .map_err(|_| io::Error::other("the pending table is poisoned"))?
            .insert(id, Box::new(on_response));
        let sent = self.write(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }));
        if let Err(error) = sent {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            return Err(error);
        }
        Ok(id)
    }

    /// Send a request and get a receiver for its response message. The
    /// receiver disconnects if the connection closes first.
    pub fn request(&self, method: &str, params: Value) -> io::Result<flume::Receiver<Value>> {
        let (tx, rx) = flume::bounded(1);
        self.request_with(method, params, move |response| {
            let _ = tx.send(response);
        })?;
        Ok(rx)
    }

    /// Answer a request the server sent (`id` is the server's, echoed).
    pub fn respond(&self, id: &Value, outcome: Result<Value, (i64, String)>) -> io::Result<()> {
        let message = match outcome {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => json!({
                "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message }
            }),
        };
        self.write(&message)
    }

    /// Route a response we received to whoever asked. `false`: nobody was
    /// waiting on that id.
    fn deliver(&self, id: &Value, response: Value) -> bool {
        let Some(id) = id
            .as_i64()
            .or_else(|| id.as_str().and_then(|text| text.parse().ok()))
        else {
            return false;
        };
        let callback = self.pending.lock().ok().and_then(|mut pending| pending.remove(&id));
        match callback {
            Some(callback) => {
                callback(response);
                true
            }
            None => false,
        }
    }

    /// How many requests are waiting for an answer.
    pub fn pending_len(&self) -> usize {
        self.pending.lock().map(|pending| pending.len()).unwrap_or(0)
    }
}

/// What the host does about a request the server sent.
#[derive(Clone, Debug, PartialEq)]
pub enum RequestOutcome {
    /// Reply now with this result.
    Reply(Value),
    /// Reply now with this error (`code`, `message`).
    Error(i64, String),
    /// The handler answers later through [`Connection::respond`].
    Deferred,
}

/// JSON-RPC's "method not found".
pub const METHOD_NOT_FOUND: i64 = -32601;

/// What the reader loop reports to its owner.
pub trait Handler {
    /// A request from the server. The loop sends the reply unless the
    /// outcome is [`RequestOutcome::Deferred`].
    fn request(&mut self, id: &Value, method: &str, params: &Value) -> RequestOutcome;
    /// A notification from the server.
    fn notification(&mut self, method: &str, params: Value);
}

/// Read one framed message. `Ok(None)` is the end of the stream. Header
/// lines other than `Content-Length` are ignored; a body that is not JSON
/// is skipped with a warning.
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    loop {
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                return Ok(None);
            }
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if content_length.is_some() {
                    break;
                }
                continue;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("content-length") {
                    content_length = value.trim().parse().ok();
                }
            }
        }
        let mut body = vec![0u8; content_length.unwrap_or(0)];
        reader.read_exact(&mut body)?;
        match serde_json::from_slice::<Value>(&body) {
            Ok(message) => return Ok(Some(message)),
            Err(error) => tracing::warn!("a language server sent a message that is not JSON: {error}"),
        }
    }
}

/// Read until the stream ends, routing each message: responses to the
/// requests we sent, requests and notifications to `handler`. Returns when
/// the stream ends or fails; the connection is closed on the way out.
pub fn run_reader(mut reader: impl BufRead, connection: &Connection, handler: &mut dyn Handler) {
    loop {
        let message = match read_message(&mut reader) {
            Ok(Some(message)) => message,
            Ok(None) => break,
            Err(error) => {
                tracing::warn!("[{}] read failed: {error}", connection.label);
                break;
            }
        };
        connection.trace("<-", &message);
        let method = message.get("method").and_then(Value::as_str).map(str::to_owned);
        let id = message.get("id").cloned();
        match (method, id) {
            // A request: the id is the server's, never matched against ours.
            (Some(method), Some(id)) => {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                let reply = match handler.request(&id, &method, &params) {
                    RequestOutcome::Reply(result) => Some(Ok(result)),
                    RequestOutcome::Error(code, text) => Some(Err((code, text))),
                    RequestOutcome::Deferred => None,
                };
                if let Some(reply) = reply {
                    if let Err(error) = connection.respond(&id, reply) {
                        tracing::warn!("[{}] could not answer {method}: {error}", connection.label);
                    }
                }
            }
            (Some(method), None) => {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                handler.notification(&method, params);
            }
            // A response to one of our requests.
            (None, Some(id)) => {
                if !connection.deliver(&id, message) {
                    tracing::debug!("[{}] a response nobody waits for (id {id})", connection.label);
                }
            }
            (None, None) => tracing::debug!("[{}] a message with neither method nor id", connection.label),
        }
    }
    connection.close();
}

#[cfg(test)]
pub(crate) mod fake {
    //! A scripted server on the other end of a pair of in-memory pipes.

    use super::*;
    use std::io::Read;

    /// The server's stdin as the client writes it: bytes into a channel.
    pub struct PipeWriter(pub flume::Sender<Vec<u8>>);

    impl Write for PipeWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .send(buf.to_vec())
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "closed"))?;
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The other end of a pipe: bytes out of a channel.
    pub struct PipeReader {
        rx: flume::Receiver<Vec<u8>>,
        buffer: Vec<u8>,
        at: usize,
    }

    impl PipeReader {
        pub fn new(rx: flume::Receiver<Vec<u8>>) -> Self {
            Self { rx, buffer: Vec::new(), at: 0 }
        }
    }

    impl Read for PipeReader {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.at >= self.buffer.len() {
                match self.rx.recv() {
                    Ok(bytes) => {
                        self.buffer = bytes;
                        self.at = 0;
                    }
                    Err(_) => return Ok(0),
                }
            }
            let n = out.len().min(self.buffer.len() - self.at);
            out[..n].copy_from_slice(&self.buffer[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
    }

    /// Frame a message as a server would send it.
    pub fn frame(message: &Value) -> Vec<u8> {
        let body = message.to_string();
        format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
    }

    /// A client connection wired to a fake server: the client's writes come
    /// out of `from_client`, and what is sent into `to_client` is what the
    /// reader loop reads.
    pub struct Wire {
        pub connection: Arc<Connection>,
        pub from_client: std::io::BufReader<PipeReader>,
        pub to_client: flume::Sender<Vec<u8>>,
        pub client_reader: Option<std::io::BufReader<PipeReader>>,
    }

    pub fn wire() -> Wire {
        let (client_out, server_in) = flume::unbounded();
        let (server_out, client_in) = flume::unbounded();
        Wire {
            connection: Connection::new(PipeWriter(client_out), "fake"),
            from_client: std::io::BufReader::new(PipeReader::new(server_in)),
            to_client: server_out,
            client_reader: Some(std::io::BufReader::new(PipeReader::new(client_in))),
        }
    }

    pub fn send(to_client: &flume::Sender<Vec<u8>>, message: Value) {
        to_client.send(frame(&message)).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[derive(Default)]
    struct Recorder {
        requests: Vec<(Value, String, Value)>,
        notifications: Vec<(String, Value)>,
        outcome: Option<RequestOutcome>,
    }

    impl Handler for Recorder {
        fn request(&mut self, id: &Value, method: &str, params: &Value) -> RequestOutcome {
            self.requests.push((id.clone(), method.to_string(), params.clone()));
            self.outcome.clone().unwrap_or(RequestOutcome::Reply(Value::Null))
        }

        fn notification(&mut self, method: &str, params: Value) {
            self.notifications.push((method.to_string(), params));
        }
    }

    fn run(wire: &mut Wire, handler: Recorder) -> thread::JoinHandle<Recorder> {
        let reader = wire.client_reader.take().unwrap();
        let connection = wire.connection.clone();
        thread::spawn(move || {
            let mut handler = handler;
            run_reader(reader, &connection, &mut handler);
            handler
        })
    }

    /// The server speaks the other id space on purpose: its request has the
    /// same id as our pending one.
    #[test]
    fn a_server_request_with_a_pending_clients_id_is_not_taken_for_its_response() {
        let mut wire = wire();
        let reader = run(&mut wire, Recorder::default());
        let answer = wire.connection.request("textDocument/hover", json!({})).unwrap();
        // Our request got id 1. The server now sends ITS request id 1.
        let sent = read_message(&mut wire.from_client).unwrap().unwrap();
        assert_eq!(sent["id"], 1);
        send(&wire.to_client, json!({
            "jsonrpc": "2.0", "id": 1, "method": "workspace/configuration",
            "params": { "items": [{ "section": "x" }] }
        }));
        // The client answers the server's request id 1 ...
        let reply = read_message(&mut wire.from_client).unwrap().unwrap();
        assert_eq!(reply["id"], 1);
        assert!(reply.get("method").is_none(), "a reply, not a request");
        assert_eq!(reply["result"], Value::Null);
        // ... and our own request is still waiting, not answered by it.
        assert!(answer.try_recv().is_err());
        assert_eq!(wire.connection.pending_len(), 1);
        // The real response arrives and is delivered.
        send(&wire.to_client, json!({ "jsonrpc": "2.0", "id": 1, "result": { "contents": "hi" } }));
        let response = answer.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(response["result"]["contents"], "hi");
        drop(wire.to_client);
        let handler = reader.join().unwrap();
        assert_eq!(handler.requests.len(), 1);
        assert_eq!(handler.requests[0].1, "workspace/configuration");
    }

    #[test]
    fn server_requests_may_carry_string_ids_and_are_echoed_back_verbatim() {
        let mut wire = wire();
        let handler = Recorder {
            outcome: Some(RequestOutcome::Error(METHOD_NOT_FOUND, "nope".into())),
            ..Recorder::default()
        };
        let reader = run(&mut wire, handler);
        send(&wire.to_client, json!({ "jsonrpc": "2.0", "id": "abc-1", "method": "x/unknown" }));
        let reply = read_message(&mut wire.from_client).unwrap().unwrap();
        assert_eq!(reply["id"], "abc-1");
        assert_eq!(reply["error"]["code"], METHOD_NOT_FOUND);
        drop(wire.to_client);
        reader.join().unwrap();
    }

    #[test]
    fn notifications_reach_the_handler_and_deferred_requests_are_not_answered() {
        let mut wire = wire();
        let handler = Recorder {
            outcome: Some(RequestOutcome::Deferred),
            ..Recorder::default()
        };
        let reader = run(&mut wire, handler);
        send(&wire.to_client, json!({ "jsonrpc": "2.0", "method": "$/progress", "params": { "token": 1 } }));
        send(&wire.to_client, json!({ "jsonrpc": "2.0", "id": 9, "method": "workspace/applyEdit", "params": {} }));
        drop(wire.to_client);
        let handler = reader.join().unwrap();
        assert_eq!(handler.notifications.len(), 1);
        assert_eq!(handler.requests.len(), 1);
        let mut from_client = wire.from_client;
        drop(wire.connection);
        assert!(
            from_client.fill_buf().map(|b| b.is_empty()).unwrap_or(true),
            "nothing was written back for a deferred request"
        );
    }

    #[test]
    fn closing_disconnects_waiting_requests() {
        let mut wire = wire();
        let reader = run(&mut wire, Recorder::default());
        let answer = wire.connection.request("a", json!({})).unwrap();
        drop(wire.to_client);
        reader.join().unwrap();
        assert!(wire.connection.is_closed());
        assert!(answer.recv_timeout(Duration::from_secs(5)).is_err());
        assert!(wire.connection.notify("b", json!({})).is_err());
    }

    #[test]
    fn framing_tolerates_extra_headers_and_split_reads() {
        let (tx, rx) = flume::unbounded();
        let body = json!({ "jsonrpc": "2.0", "method": "m" }).to_string();
        let framed = format!(
            "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes();
        for chunk in framed.chunks(7) {
            tx.send(chunk.to_vec()).unwrap();
        }
        drop(tx);
        let mut reader = std::io::BufReader::new(PipeReader::new(rx));
        let message = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(message["method"], "m");
        assert!(read_message(&mut reader).unwrap().is_none());
    }
}
