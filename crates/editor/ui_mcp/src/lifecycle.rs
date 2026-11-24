//! Keeping the server in step with the settings.
//!
//! A persistent editor task on the UI executor reads `editor / mcp` every
//! couple of seconds (an in-memory read) and starts, stops or restarts the
//! server. The server itself runs as services of its own (`pulsar_mcp`); this is
//! only the decision.

use std::time::Duration;

use gpui::{App, AsyncApp};
use parking_lot::Mutex;
use pulsar_mcp::{McpConfig, McpServer};
use pulsar_settings::editor::mcp::{DEFAULT_PORT, ENABLED, NS, OWNER, PORT};

/// How often the settings are looked at.
const POLL: Duration = Duration::from_secs(2);

/// Where the running server is.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub url: String,
    pub port: u16,
    /// The file a client reads the token from.
    pub endpoint_file: std::path::PathBuf,
}

struct Running {
    server: McpServer,
    port_wanted: u16,
}

static SERVER: Mutex<Option<Running>> = Mutex::new(None);
static APPLYING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The running server's endpoint, `None` when it is off.
pub fn current_endpoint() -> Option<Endpoint> {
    SERVER.lock().as_ref().map(|running| Endpoint {
        url: running.server.url(),
        port: running.server.local_addr().port(),
        endpoint_file: pulsar_mcp::default_endpoint_file(),
    })
}

/// `(enabled, port)` now: the settings, with the environment's override for one
/// session (`PULSAR_MCP=1`, `PULSAR_MCP_PORT`).
fn wanted() -> (bool, u16) {
    let config = engine_state::global_config();
    let setting = |key: &str| config.get(NS, OWNER, key).ok();
    let enabled = std::env::var("PULSAR_MCP").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        || setting(ENABLED)
            .and_then(|value| value.as_bool().ok())
            .unwrap_or(false);
    let port = std::env::var("PULSAR_MCP_PORT")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .or_else(|| setting(PORT).and_then(|value| value.as_int().ok()))
        .unwrap_or(DEFAULT_PORT);
    (enabled, u16::try_from(port).unwrap_or(DEFAULT_PORT as u16))
}

/// Whether the server is not as the settings ask (a cheap read: the UI thread
/// decides, a task does the work).
fn out_of_step() -> Option<(bool, u16)> {
    let (enabled, port) = wanted();
    let slot = SERVER.lock();
    let differs = match slot.as_ref() {
        Some(running) => !enabled || running.port_wanted != port,
        None => enabled && LAST_FAILED.lock().is_none_or(|failed| failed != port),
    };
    differs.then_some((enabled, port))
}

static LAST_FAILED: Mutex<Option<u16>> = Mutex::new(None);

/// Start, stop or restart the server to match `(enabled, port)`. Runs in a task: it
/// binds a socket and writes the endpoint file.
fn apply(enabled: bool, port: u16) {
    let mut slot = SERVER.lock();
    let restart = match slot.as_ref() {
        Some(running) => !enabled || running.port_wanted != port,
        None => false,
    };
    if restart {
        if let Some(running) = slot.take() {
            tracing::info!("Stopping the MCP server");
            running.server.stop();
        }
    }
    if enabled && slot.is_none() {
        match McpServer::start(McpConfig {
            port,
            token: None,
            endpoint_file: Some(pulsar_mcp::default_endpoint_file()),
        }) {
            Ok(server) => {
                tracing::info!(url = %server.url(), "MCP server started");
                *LAST_FAILED.lock() = None;
                *slot = Some(Running {
                    server,
                    port_wanted: port,
                });
            }
            Err(error) => {
                // Not retried every poll: say it once per port and wait for a change.
                tracing::error!(port, "Could not start the MCP server: {error}");
                *LAST_FAILED.lock() = Some(port);
            }
        }
    }
}

pub(crate) fn install(cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let _task = editor_task_queue::global().persistent_task(
            editor_task_queue::TaskDescription::new(
                "MCP server settings watcher",
                "mcp",
                editor_task_queue::TaskDuration::Long,
            ),
            None,
        );
        loop {
            if let Some((enabled, port)) = out_of_step() {
                if !APPLYING.swap(true, std::sync::atomic::Ordering::AcqRel) {
                    editor_task_queue::global().submit(
                        editor_task_queue::TaskDescription::new(
                            "Apply MCP server settings",
                            "mcp",
                            editor_task_queue::TaskDuration::Short,
                        ),
                        move |_| {
                            apply(enabled, port);
                            APPLYING.store(false, std::sync::atomic::Ordering::Release);
                            Ok::<(), String>(())
                        },
                    );
                }
            }
            cx.background_executor().timer(POLL).await;
        }
    })
    .detach();
}
