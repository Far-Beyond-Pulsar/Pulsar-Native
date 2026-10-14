//! A minimal MCP client: newline-delimited JSON-RPC 2.0 over a child
//! process's stdio. Implements `initialize`, `tools/list` and `tools/call`
//! with per-request timeouts.
//!
//! One reader thread owns the child's stdout and routes responses to the
//! waiting caller by id; a second thread drains stderr into a small tail used
//! in error messages. A server that dies, hangs or sends garbage only ever
//! produces an `Err` for the calls waiting on it.

use std::{
    collections::{HashMap, VecDeque},
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};

use serde_json::{json, Value};

use crate::config::ServerConfig;

pub const PROTOCOL_VERSION: &str = "2024-11-05";
const STDERR_TAIL_LINES: usize = 20;

#[derive(Debug, Clone)]
pub enum McpError {
    /// Could not start the server process.
    Spawn(String),
    /// The server process exited or closed its pipes.
    Disconnected(String),
    Timeout(String),
    /// The server answered with a JSON-RPC error.
    Rpc { code: i64, message: String },
    Protocol(String),
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpError::Spawn(m) => write!(f, "could not start MCP server: {m}"),
            McpError::Disconnected(m) => write!(f, "MCP server disconnected: {m}"),
            McpError::Timeout(m) => write!(f, "MCP request timed out: {m}"),
            McpError::Rpc { code, message } => write!(f, "MCP error {code}: {message}"),
            McpError::Protocol(m) => write!(f, "MCP protocol error: {m}"),
        }
    }
}

impl std::error::Error for McpError {}

/// One tool as the server describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

type Pending = Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value, McpError>>>>>;
type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

pub struct McpClient {
    name: String,
    writer: SharedWriter,
    child: Mutex<Option<Child>>,
    pending: Pending,
    next_id: AtomicU64,
    alive: Arc<AtomicBool>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    timeout: Duration,
}

impl McpClient {
    /// Launch `config.command` and complete the `initialize` handshake.
    pub fn spawn(
        name: &str,
        config: &ServerConfig,
        workspace_root: &std::path::Path,
    ) -> Result<Arc<Self>, McpError> {
        let command = config
            .command
            .as_deref()
            .filter(|c| !c.trim().is_empty())
            .ok_or_else(|| {
                if config.url.is_some() {
                    McpError::Spawn("the streamable-HTTP transport is not supported yet; use a stdio command".into())
                } else {
                    McpError::Spawn("no \"command\" configured".into())
                }
            })?;

        let mut cmd = build_command(command, &config.args);
        cmd.current_dir(
            config
                .cwd
                .as_deref()
                .map(|c| workspace_root.join(c))
                .unwrap_or_else(|| workspace_root.to_path_buf()),
        )
        .envs(&config.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| McpError::Spawn(format!("{command}: {e}")))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let client = Self::from_io(name, Box::new(stdin), Box::new(stdout), Some(child), config.timeout());
        client.drain_stderr(stderr);
        match client.initialize() {
            Ok(()) => Ok(client),
            Err(err) => {
                client.close();
                Err(err)
            }
        }
    }

    /// Wire a client to any pair of pipes (used by tests and by `spawn`).
    /// The caller still has to run [`McpClient::initialize`].
    pub fn from_io(
        name: &str,
        writer: Box<dyn Write + Send>,
        reader: Box<dyn Read + Send>,
        child: Option<Child>,
        timeout: Duration,
    ) -> Arc<Self> {
        let client = Arc::new(Self {
            name: name.to_string(),
            writer: Arc::new(Mutex::new(writer)),
            child: Mutex::new(child),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            alive: Arc::new(AtomicBool::new(true)),
            stderr_tail: Arc::new(Mutex::new(VecDeque::new())),
            timeout,
        });
        client.start_reader(reader);
        client
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn drain_stderr(&self, stderr: impl Read + Send + 'static) {
        let tail = self.stderr_tail.clone();
        let name = self.name.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                tracing::debug!("[agent_mcp:{name}] {line}");
                let mut tail = tail.lock().unwrap_or_else(|e| e.into_inner());
                if tail.len() >= STDERR_TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        });
    }

    fn stderr_summary(&self) -> String {
        let tail = self.stderr_tail.lock().unwrap_or_else(|e| e.into_inner());
        let last: Vec<&str> = tail.iter().rev().take(3).map(String::as_str).collect();
        last.into_iter().rev().collect::<Vec<_>>().join(" | ")
    }

    fn start_reader(&self, reader: impl Read + Send + 'static) {
        let pending = self.pending.clone();
        let alive = self.alive.clone();
        let writer = self.writer.clone();
        let name = self.name.clone();
        thread::spawn(move || {
            for line in BufReader::new(reader).lines().map_while(Result::ok) {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Ok(msg) = serde_json::from_str::<Value>(line) else {
                    tracing::debug!("[agent_mcp:{name}] non-JSON stdout line ignored");
                    continue;
                };
                route_message(&msg, &pending, &writer);
            }
            alive.store(false, Ordering::SeqCst);
            let waiting: Vec<_> = pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .drain()
                .collect();
            for (_, tx) in waiting {
                let _ = tx.send(Err(McpError::Disconnected("stdout closed".into())));
            }
        });
    }

    fn write_message(&self, msg: &Value) -> Result<(), McpError> {
        let mut line = msg.to_string();
        line.push('\n');
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        writer
            .write_all(line.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|e| McpError::Disconnected(format!("write failed: {e}")))
    }

    /// Send a request and wait for its response, at most `timeout`.
    pub fn request_with(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, McpError> {
        if !self.is_alive() {
            return Err(self.disconnected());
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);
        // The reader may have exited between the check above and the insert.
        if !self.is_alive() {
            self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            return Err(self.disconnected());
        }
        if let Err(err) = self.write_message(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        })) {
            self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            return Err(err);
        }
        match rx.recv_timeout(timeout) {
            Ok(result) => result.map_err(|e| match e {
                McpError::Disconnected(_) => self.disconnected(),
                other => other,
            }),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                let _ = self.write_message(&json!({
                    "jsonrpc": "2.0", "method": "notifications/cancelled",
                    "params": {"requestId": id, "reason": "timeout"}
                }));
                Err(McpError::Timeout(format!(
                    "{method} on '{}' after {}s",
                    self.name,
                    timeout.as_secs()
                )))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(self.disconnected()),
        }
    }

    pub fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        self.request_with(method, params, self.timeout)
    }

    fn disconnected(&self) -> McpError {
        let tail = self.stderr_summary();
        McpError::Disconnected(if tail.is_empty() {
            format!("server '{}' exited", self.name)
        } else {
            format!("server '{}' exited ({tail})", self.name)
        })
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        self.write_message(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    pub fn initialize(&self) -> Result<(), McpError> {
        let result = self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "pulsar-engine", "version": env!("CARGO_PKG_VERSION")}
            }),
        )?;
        if !result.is_object() {
            return Err(McpError::Protocol("initialize result is not an object".into()));
        }
        self.notify("notifications/initialized", json!({}))
    }

    /// All tools, following `nextCursor` pagination (bounded).
    pub fn list_tools(&self) -> Result<Vec<RemoteTool>, McpError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..20 {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let result = self.request("tools/list", params)?;
            let items = result
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(|| McpError::Protocol("tools/list result has no \"tools\" array".into()))?;
            for item in items {
                let Some(name) = item.get("name").and_then(Value::as_str) else {
                    continue;
                };
                tools.push(RemoteTool {
                    name: name.to_string(),
                    description: item
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    input_schema: item
                        .get("inputSchema")
                        .cloned()
                        .filter(Value::is_object)
                        .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                });
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    /// Raw `tools/call` result (`content`, `isError`, ...).
    pub fn call_tool(&self, tool: &str, arguments: Value) -> Result<Value, McpError> {
        let arguments = if arguments.is_object() { arguments } else { json!({}) };
        self.request("tools/call", json!({"name": tool, "arguments": arguments}))
    }

    /// Stop the server: close its stdin, give it a moment, then kill it.
    pub fn close(&self) {
        self.alive.store(false, Ordering::SeqCst);
        // Dropping the writer closes the child's stdin.
        *self.writer.lock().unwrap_or_else(|e| e.into_inner()) = Box::new(std::io::sink());
        let child = self.child.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut child) = child {
            for _ in 0..10 {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.close();
    }
}

/// Route one incoming message: a response goes to its waiting caller; a
/// request from the server gets a polite answer so it never blocks on us.
fn route_message(msg: &Value, pending: &Pending, writer: &SharedWriter) {
    let id = msg.get("id");
    let method = msg.get("method").and_then(Value::as_str);
    match (method, id) {
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else { return };
            let Some(tx) = pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) else {
                return;
            };
            let outcome = match msg.get("error") {
                Some(err) => Err(McpError::Rpc {
                    code: err.get("code").and_then(Value::as_i64).unwrap_or(-32000),
                    message: err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error")
                        .to_string(),
                }),
                None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = tx.send(outcome);
        }
        (Some(method), Some(id)) => {
            let reply = if method == "ping" {
                json!({"jsonrpc": "2.0", "id": id, "result": {}})
            } else {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not supported by this client"}})
            };
            let mut line = reply.to_string();
            line.push('\n');
            let mut w = writer.lock().unwrap_or_else(|e| e.into_inner());
            let _ = w.write_all(line.as_bytes()).and_then(|_| w.flush());
        }
        _ => {} // notifications are ignored
    }
}

/// `npx` and friends are `.cmd` shims on Windows; run them through `cmd /c`.
fn build_command(command: &str, args: &[String]) -> Command {
    #[cfg(windows)]
    {
        let stem = command.to_ascii_lowercase();
        if matches!(stem.as_str(), "npx" | "npm" | "yarn" | "pnpm" | "uvx" | "bunx") {
            let mut cmd = Command::new("cmd");
            cmd.arg("/c").arg(command).args(args);
            return cmd;
        }
    }
    let mut cmd = Command::new(command);
    cmd.args(args);
    cmd
}
