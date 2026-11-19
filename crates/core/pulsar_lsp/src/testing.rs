//! A language server scripted on a pipe, for tests of what talks to one.
//!
//! [`scripted_server`] gives the two ends a client needs
//! ([`LanguageServer::attach`](crate::LanguageServer::attach)): a reader for
//! what the "server" writes and a writer for what the client sends. A thread
//! plays the server: every message the client sends goes to the script along
//! with a [`Reply`] it answers on, and to the `received` channel for the
//! test's own assertions. No process, no Node, no network.

use std::io::{BufReader, Write};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::protocol::read_message;

/// How a script talks back: framed JSON-RPC messages to the client.
#[derive(Clone)]
pub struct Reply {
    writer: Arc<Mutex<std::io::PipeWriter>>,
}

impl Reply {
    /// Send any message to the client.
    pub fn send(&self, message: Value) {
        let body = message.to_string();
        if let Ok(mut writer) = self.writer.lock() {
            let _ = write!(writer, "Content-Length: {}\r\n\r\n{body}", body.len());
            let _ = writer.flush();
        }
    }

    /// Answer the request `id` with `result`.
    pub fn result(&self, id: &Value, result: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    /// Send a notification.
    pub fn notify(&self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Send a request of the server's own (the client must answer it).
    pub fn request(&self, id: Value, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
    }
}

/// The client's ends of a scripted server, and what the script has been sent.
pub struct ScriptedServer {
    /// What the server writes (give it to `attach`).
    pub reader: std::io::PipeReader,
    /// What the client writes to the server (give it to `attach`).
    pub writer: std::io::PipeWriter,
    /// Every message the client sent, in order.
    pub received: flume::Receiver<Value>,
}

/// Start a server that runs `script` on every message the client sends. The
/// server ends (and the client's reader sees the end of the stream) when the
/// client's writer is dropped.
pub fn scripted_server(
    mut script: impl FnMut(&Value, &Reply) + Send + 'static,
) -> ScriptedServer {
    let (server_stdin, client_writer) = std::io::pipe().expect("a pipe");
    let (client_reader, server_stdout) = std::io::pipe().expect("a pipe");
    let (received_tx, received) = flume::unbounded();
    let reply = Reply {
        writer: Arc::new(Mutex::new(server_stdout)),
    };
    let _ = editor_task_queue::spawn_thread(
        editor_task_queue::ThreadSpec::new(
            "scripted language server",
            editor_task_queue::ThreadRole::Io,
        )
        .owner("pulsar_lsp"),
        move || {
            let mut input = BufReader::new(server_stdin);
            while let Ok(Some(message)) = read_message(&mut input) {
                script(&message, &reply);
                let _ = received_tx.send(message);
            }
            // The stdout writer is dropped with `reply` here: the client's
            // reader sees the end of the stream.
        },
    );
    ScriptedServer {
        reader: client_reader,
        writer: client_writer,
        received,
    }
}
