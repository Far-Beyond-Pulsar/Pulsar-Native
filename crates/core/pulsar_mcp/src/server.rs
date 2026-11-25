//! The server: a listener and a few workers, each an editor service with a
//! heartbeat (so the Tasks window lists them, a stall is reported, and no raw
//! thread exists), bound to loopback only.
//!
//! The token and the address are written to an endpoint file in the per-user
//! app data (not the project: a project is often under version control), which
//! is how a client on the same machine finds the server. The file is removed when
//! the server stops.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use editor_task_queue::{ServiceDescription, ServiceHandle, ThreadRole, spawn_service};

use crate::http::{self, Policy};

/// Workers answering requests. A tool that waits (`logs_wait`) holds one for up
/// to its timeout, so there are several.
const WORKERS: usize = 4;

/// How to start the server.
#[derive(Clone, Debug, Default)]
pub struct McpConfig {
    /// The loopback port; `0` lets the system pick one.
    pub port: u16,
    /// The bearer token; a fresh random one when `None`.
    pub token: Option<String>,
    /// Where to write the endpoint file (`url`, `token`); none when `None`.
    pub endpoint_file: Option<PathBuf>,
}

/// The per-user endpoint file a client reads to find the running server:
/// `<app data>/mcp/endpoint.json`.
pub fn default_endpoint_file() -> PathBuf {
    pulsar_install_layout::appdata_dir()
        .join("mcp")
        .join("endpoint.json")
}

/// A fresh random token: 256 bits, hex.
pub fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// A running server.
pub struct McpServer {
    addr: SocketAddr,
    token: String,
    stop: Arc<AtomicBool>,
    services: Vec<ServiceHandle<()>>,
    endpoint_file: Option<PathBuf>,
}

impl McpServer {
    /// Bind `127.0.0.1:<port>` and start the acceptor and the workers.
    pub fn start(config: McpConfig) -> std::io::Result<Self> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, config.port))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let token = config.token.unwrap_or_else(new_token);
        let policy = Policy {
            port: addr.port(),
            token: token.clone(),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let (send, receive) = mpsc::channel::<std::net::TcpStream>();
        let receive = Arc::new(Mutex::new(receive));
        let mut services = Vec::new();

        for index in 0..WORKERS {
            let policy = policy.clone();
            let receive = Arc::clone(&receive);
            let stop = Arc::clone(&stop);
            services.push(spawn_service(
                ServiceDescription::new(
                    format!("MCP worker {}", index + 1),
                    "mcp",
                    format!("mcp-worker-{}", index + 1),
                    ThreadRole::Service,
                )
                .owner("pulsar_mcp")
                .heartbeat(Duration::from_secs(5)),
                move |ctx| {
                    ctx.set_phase("Waiting for a request");
                    while !stop.load(Ordering::Acquire) && !ctx.is_cancelled() {
                        ctx.heartbeat();
                        let next = receive
                            .lock()
                            .map(|receiver| receiver.recv_timeout(Duration::from_millis(250)))
                            .unwrap_or(Err(RecvTimeoutError::Disconnected));
                        match next {
                            Ok(stream) => {
                                ctx.set_phase("Serving a request");
                                http::serve(stream, &policy);
                                ctx.set_phase("Waiting for a request");
                            }
                            Err(RecvTimeoutError::Timeout) => {}
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    }
                },
            )?);
        }

        let accept_stop = Arc::clone(&stop);
        services.push(spawn_service(
            ServiceDescription::new("MCP server", "mcp", "mcp-accept", ThreadRole::Service)
                .owner("pulsar_mcp")
                .heartbeat(Duration::from_secs(5)),
            move |ctx| {
                ctx.set_phase("Accepting connections");
                tracing::info!(%addr, "MCP server listening (loopback only, bearer token required)");
                while !accept_stop.load(Ordering::Acquire) && !ctx.is_cancelled() {
                    ctx.heartbeat();
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            if send.send(stream).is_err() {
                                break;
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(30));
                        }
                        Err(error) => {
                            tracing::warn!(%error, "MCP server: accept failed");
                            std::thread::sleep(Duration::from_millis(200));
                        }
                    }
                }
                tracing::info!("MCP server stopped");
            },
        )?);

        let server = Self {
            addr,
            token,
            stop,
            services,
            endpoint_file: config.endpoint_file,
        };
        if let Some(file) = &server.endpoint_file {
            server.write_endpoint(file);
        }
        Ok(server)
    }

    /// The bound address (`127.0.0.1:<port>`).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// The endpoint a client posts to.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.addr.port())
    }

    /// The bearer token.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// A `.mcp.json` a client can use: the URL and the `Authorization`
    /// header. It holds the token, so it is written to nothing the project
    /// shares.
    pub fn client_config(&self) -> serde_json::Value {
        serde_json::json!({
            "mcpServers": {
                "pulsar": {
                    "type": "http",
                    "url": self.url(),
                    "headers": { "Authorization": format!("Bearer {}", self.token) },
                }
            }
        })
    }

    fn write_endpoint(&self, file: &Path) {
        let body = serde_json::json!({
            "url": self.url(),
            "token": self.token,
            "pid": std::process::id(),
            "client_config": self.client_config(),
        });
        let write = || -> Result<(), String> {
            if let Some(dir) = file.parent() {
                engine_fs::virtual_fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            engine_fs::virtual_fs::write_file(
                file,
                serde_json::to_string_pretty(&body)
                    .map_err(|e| e.to_string())?
                    .as_bytes(),
            )
            .map_err(|e| e.to_string())
        };
        match write() {
            Ok(()) => tracing::info!(file = %file.display(), "MCP endpoint file written"),
            Err(error) => {
                tracing::warn!(file = %file.display(), "could not write the MCP endpoint file: {error}")
            }
        }
    }

    /// Stop the server: the services end, the endpoint file goes.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        for service in self.services.drain(..) {
            service.request_stop();
            let _ = service.join();
        }
        if let Some(file) = self.endpoint_file.take() {
            let _ = engine_fs::virtual_fs::delete_path(&file);
        }
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}
