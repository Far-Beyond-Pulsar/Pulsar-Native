//! Owns the configured MCP servers: starts them (off the caller's thread),
//! tracks status, registers their tools in a [`ToolRegistry`] as
//! `mcp__<server>__<tool>`, and shuts the processes down.
//!
//! Classification and taint (see `agent_runtime::taint`):
//! * every MCP tool is `Write`-class (asks when the run is tainted) unless the
//!   server's `read_only_tools` lists it -> [`McpManager::apply_classification`];
//! * every MCP result is untrusted, read-only or not (prefix rule in the
//!   runtime);
//! * the Lite router files them under the `mcp` category behind `more_tools`.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};

use agent_chat_tools::{ChatTool, ToolContext, ToolRegistry};
use agent_runtime::trust::{mcp_item, PendingItem, PendingKind, TrustStore};
use agent_runtime::{ToolClass, ToolClassifier, MCP_TOOL_PREFIX};
use anyhow::anyhow;
use serde_json::{json, Value};

use crate::{
    client::{McpClient, McpError, RemoteTool},
    config::{state_path, McpConfig, ServerConfig},
};

/// Category string tools are registered under.
pub const MCP_CATEGORY: &str = "mcp";
/// Longest tool name most providers accept.
const MAX_TOOL_NAME: usize = 64;
const MAX_DESCRIPTION_CHARS: usize = 400;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerStatus {
    /// Configured and enabled, not started yet (or starting).
    Pending,
    Running,
    /// `"enabled": false` in the config.
    Disabled,
    /// Could not start or handshake; carries the reason.
    Failed(String),
    /// Was running, then the process went away.
    Crashed(String),
    /// Defined by the project's `.pulsar/mcp.json` and not yet approved by the
    /// user: nothing has been launched. See [`McpManager::approve`].
    AwaitingApproval,
}

impl ServerStatus {
    pub fn label(&self) -> &'static str {
        match self {
            ServerStatus::Pending => "starting",
            ServerStatus::Running => "running",
            ServerStatus::Disabled => "disabled",
            ServerStatus::Failed(_) => "failed",
            ServerStatus::Crashed(_) => "crashed",
            ServerStatus::AwaitingApproval => "needs approval",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolSnapshot {
    /// Registered name: `mcp__<server>__<tool>`.
    pub full_name: String,
    /// The server's own name for it.
    pub name: String,
    pub description: String,
    pub read_only: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct ServerSnapshot {
    pub name: String,
    pub command: String,
    pub status: ServerStatus,
    pub tools: Vec<ToolSnapshot>,
}

struct ServerEntry {
    config: ServerConfig,
    status: ServerStatus,
    client: Option<Arc<McpClient>>,
    tools: Vec<RemoteTool>,
    /// Bumped on every (re)start so a slow old start cannot overwrite a newer one.
    generation: u64,
    /// For a project-defined server under a trust record: what approving it
    /// means (`ServerConfig::launch_description`).
    approval: Option<String>,
}

struct Inner {
    workspace_root: PathBuf,
    servers: BTreeMap<String, ServerEntry>,
    disabled_tools: BTreeSet<String>,
    trust: Option<Arc<TrustStore>>,
}

pub struct McpManager {
    inner: Mutex<Inner>,
}

impl McpManager {
    /// Read the config files. Nothing is launched until [`start_all`](Self::start_all).
    ///
    /// Servers the project's `.pulsar/mcp.json` defines launch processes, and
    /// a cloned repository can ship that file, so each needs the user's
    /// approval (recorded in `trust`, keyed by project and by what would be
    /// launched). Until then it is `AwaitingApproval` and nothing runs;
    /// servers from the user-global file are the user's own.
    pub fn load(workspace_root: &Path, trust: Arc<TrustStore>) -> Arc<Self> {
        let (config, project_names) = McpConfig::load_with_origins(workspace_root);
        let manager = Self::with_config(workspace_root, config);
        {
            let mut inner = manager.lock();
            inner.trust = Some(trust.clone());
            for (name, entry) in inner.servers.iter_mut() {
                if !project_names.contains(name) || !entry.config.enabled {
                    continue;
                }
                let description = entry.config.launch_description();
                if !trust.is_trusted(workspace_root, &mcp_item(name), &description) {
                    entry.status = ServerStatus::AwaitingApproval;
                }
                entry.approval = Some(description);
            }
        }
        manager
    }

    /// Servers waiting for the user, with the exact command line they would run.
    pub fn pending_approvals(&self) -> Vec<PendingItem> {
        let inner = self.lock();
        let Some(trust) = &inner.trust else {
            return Vec::new();
        };
        inner
            .servers
            .iter()
            .filter(|(_, entry)| entry.status == ServerStatus::AwaitingApproval)
            .filter_map(|(name, entry)| {
                PendingItem::new(
                    trust,
                    &inner.workspace_root,
                    PendingKind::Mcp,
                    mcp_item(name),
                    name.clone(),
                    entry.approval.clone()?,
                )
            })
            .collect()
    }

    /// The user accepted this server: record it and start it.
    pub fn approve(self: &Arc<Self>, name: &str) {
        {
            let mut inner = self.lock();
            let root = inner.workspace_root.clone();
            let Some(trust) = inner.trust.clone() else {
                return;
            };
            let Some(entry) = inner.servers.get_mut(name) else {
                return;
            };
            let Some(description) = entry.approval.clone() else {
                return;
            };
            trust.trust(&root, &mcp_item(name), &description);
            if entry.status == ServerStatus::AwaitingApproval {
                entry.status = ServerStatus::Pending;
            }
        }
        self.restart_server(name);
    }

    pub fn with_config(workspace_root: &Path, config: McpConfig) -> Arc<Self> {
        let servers = config
            .servers
            .into_iter()
            .map(|(name, config)| {
                let status = if config.enabled {
                    ServerStatus::Pending
                } else {
                    ServerStatus::Disabled
                };
                (
                    name,
                    ServerEntry {
                        config,
                        status,
                        client: None,
                        tools: Vec::new(),
                        generation: 0,
                        approval: None,
                    },
                )
            })
            .collect();
        Arc::new(Self {
            inner: Mutex::new(Inner {
                workspace_root: workspace_root.to_path_buf(),
                servers,
                disabled_tools: load_disabled(workspace_root),
                trust: None,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_empty(&self) -> bool {
        self.lock().servers.is_empty()
    }

    /// Start every enabled server, each on its own thread.
    pub fn start_all(self: &Arc<Self>) {
        let names: Vec<String> = self
            .lock()
            .servers
            .iter()
            .filter(|(_, e)| e.config.enabled && e.status != ServerStatus::AwaitingApproval)
            .map(|(n, _)| n.clone())
            .collect();
        for name in names {
            self.restart_server(&name);
        }
    }

    /// (Re)start one server in the background.
    pub fn restart_server(self: &Arc<Self>, name: &str) {
        let (config, root, generation, old) = {
            let mut inner = self.lock();
            let root = inner.workspace_root.clone();
            let Some(entry) = inner.servers.get_mut(name) else {
                return;
            };
            // Never launch what the user has not approved.
            if entry.status == ServerStatus::AwaitingApproval {
                return;
            }
            entry.generation += 1;
            entry.status = ServerStatus::Pending;
            entry.tools.clear();
            (entry.config.clone(), root, entry.generation, entry.client.take())
        };
        let this = self.clone();
        let name = name.to_string();
        thread::spawn(move || {
            if let Some(old) = old {
                old.close();
            }
            let outcome = McpClient::spawn(&name, &config, &root).and_then(|client| {
                client.list_tools().map(|tools| (client, tools))
            });
            let mut inner = this.lock();
            let Some(entry) = inner.servers.get_mut(&name) else {
                return;
            };
            if entry.generation != generation {
                // Superseded by a newer start; discard this one.
                if let Ok((client, _)) = outcome {
                    drop(inner);
                    client.close();
                }
                return;
            }
            match outcome {
                Ok((client, tools)) => {
                    tracing::info!("[agent_mcp] '{name}' ready with {} tool(s)", tools.len());
                    entry.client = Some(client);
                    entry.tools = tools;
                    entry.status = ServerStatus::Running;
                }
                Err(err) => {
                    tracing::warn!("[agent_mcp] '{name}' failed to start: {err}");
                    entry.status = ServerStatus::Failed(err.to_string());
                }
            }
        });
    }

    /// Block until no server is `Pending` or `timeout` passes. For tests and
    /// for callers that want tools in the very first request.
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if !self
                .lock()
                .servers
                .values()
                .any(|e| e.status == ServerStatus::Pending)
            {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Stop every server process. Idempotent; also runs on drop.
    pub fn shutdown(&self) {
        let clients: Vec<Arc<McpClient>> = {
            let mut inner = self.lock();
            inner
                .servers
                .values_mut()
                .filter_map(|e| {
                    e.generation += 1;
                    e.status = ServerStatus::Disabled;
                    e.tools.clear();
                    e.client.take()
                })
                .collect()
        };
        for client in clients {
            client.close();
        }
    }

    /// Mark servers whose process died as `Crashed` (cheap; call before
    /// rendering or registering).
    fn refresh_liveness(inner: &mut Inner) {
        for entry in inner.servers.values_mut() {
            if entry.status == ServerStatus::Running
                && entry.client.as_ref().is_some_and(|c| !c.is_alive())
            {
                entry.status = ServerStatus::Crashed("process exited".into());
            }
        }
    }

    pub fn snapshot(&self) -> Vec<ServerSnapshot> {
        let mut inner = self.lock();
        Self::refresh_liveness(&mut inner);
        inner
            .servers
            .iter()
            .map(|(name, entry)| ServerSnapshot {
                name: name.clone(),
                command: entry
                    .config
                    .command
                    .clone()
                    .or_else(|| entry.config.url.clone())
                    .unwrap_or_default(),
                status: entry.status.clone(),
                tools: tool_names(name, entry)
                    .into_iter()
                    .map(|(full_name, tool)| ToolSnapshot {
                        enabled: !inner.disabled_tools.contains(&full_name),
                        read_only: entry.config.is_read_only(&tool.name),
                        description: tool.description.clone(),
                        name: tool.name.clone(),
                        full_name,
                    })
                    .collect(),
            })
            .collect()
    }

    /// Switch one tool on or off. Persisted to `.pulsar/mcp_state.json`.
    pub fn set_tool_enabled(&self, full_name: &str, enabled: bool) {
        let mut inner = self.lock();
        if enabled {
            inner.disabled_tools.remove(full_name);
        } else {
            inner.disabled_tools.insert(full_name.to_string());
        }
        save_disabled(&inner.workspace_root, &inner.disabled_tools);
    }

    pub fn is_tool_enabled(&self, full_name: &str) -> bool {
        !self.lock().disabled_tools.contains(full_name)
    }

    /// Register every enabled tool of every running server. Tools that are
    /// already registered under the same name are replaced; tools of servers
    /// that are not running are removed, so a registry can be refreshed by
    /// calling this again.
    pub fn register_into(self: &Arc<Self>, registry: &mut ToolRegistry) {
        let stale: Vec<String> = registry
            .names()
            .into_iter()
            .filter(|n| n.starts_with(MCP_TOOL_PREFIX))
            .map(str::to_string)
            .collect();
        for name in stale {
            registry.unregister(&name);
        }
        let mut inner = self.lock();
        Self::refresh_liveness(&mut inner);
        for (server, entry) in &inner.servers {
            if entry.status != ServerStatus::Running {
                continue;
            }
            for (full_name, tool) in tool_names(server, entry) {
                if inner.disabled_tools.contains(&full_name) {
                    continue;
                }
                registry.register(Arc::new(McpTool {
                    full_name,
                    server: server.clone(),
                    remote_name: tool.name.clone(),
                    description: describe(server, tool),
                    schema: tool.input_schema.clone(),
                    manager: self.clone(),
                }));
            }
        }
    }

    /// Give config-vouched read-only tools the `Read` class. Everything else
    /// stays at the classifier's default for `mcp__*`: `Write`.
    pub fn apply_classification(&self, classifier: &mut ToolClassifier) {
        let inner = self.lock();
        for (server, entry) in &inner.servers {
            for (full_name, tool) in tool_names(server, entry) {
                if entry.config.is_read_only(&tool.name) {
                    classifier.set(full_name, ToolClass::Read);
                }
            }
        }
    }

    fn call(&self, server: &str, full_name: &str, tool: &str, args: Value) -> anyhow::Result<Value> {
        let client = {
            let mut inner = self.lock();
            Self::refresh_liveness(&mut inner);
            if inner.disabled_tools.contains(full_name) {
                return Err(anyhow!("tool '{full_name}' is switched off in the MCP settings"));
            }
            let entry = inner
                .servers
                .get_mut(server)
                .ok_or_else(|| anyhow!("MCP server '{server}' is not configured"))?;
            match (&entry.status, &entry.client) {
                (ServerStatus::Running, Some(c)) => c.clone(),
                (status, _) => {
                    return Err(anyhow!(
                        "MCP server '{server}' is not available ({}). The rest of the run is unaffected.",
                        status.label()
                    ))
                }
            }
        };
        // The lock is released: a slow server never blocks other tools or the UI.
        let outcome = client.call_tool(tool, args);
        if let Err(McpError::Disconnected(reason)) = &outcome {
            let mut inner = self.lock();
            if let Some(entry) = inner.servers.get_mut(server) {
                if entry.status == ServerStatus::Running {
                    entry.status = ServerStatus::Crashed(reason.clone());
                }
            }
        }
        convert_result(outcome.map_err(|e| anyhow!("{e}"))?)
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct McpTool {
    full_name: String,
    server: String,
    remote_name: String,
    description: String,
    schema: Value,
    manager: Arc<McpManager>,
}

impl ChatTool for McpTool {
    fn name(&self) -> &str {
        &self.full_name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn parameters_schema(&self) -> Value {
        self.schema.clone()
    }
    fn category(&self) -> Option<&str> {
        Some(MCP_CATEGORY)
    }
    fn execute(&self, args: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        self.manager
            .call(&self.server, &self.full_name, &self.remote_name, args)
    }
}

/// `tools/call` result -> the JSON the model sees. `isError` becomes an `Err`.
fn convert_result(result: Value) -> anyhow::Result<Value> {
    let mut texts = Vec::new();
    if let Some(items) = result.get("content").and_then(Value::as_array) {
        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(t) = item.get("text").and_then(Value::as_str) {
                        texts.push(t.to_string());
                    }
                }
                Some("image") | Some("audio") => texts.push(format!(
                    "[{} content omitted]",
                    item.get("type").and_then(Value::as_str).unwrap_or("binary")
                )),
                Some("resource") => {
                    let res = item.get("resource").cloned().unwrap_or(Value::Null);
                    match res.get("text").and_then(Value::as_str) {
                        Some(t) => texts.push(t.to_string()),
                        None => texts.push(format!(
                            "[resource {}]",
                            res.get("uri").and_then(Value::as_str).unwrap_or("?")
                        )),
                    }
                }
                _ => texts.push(item.to_string()),
            }
        }
    }
    let text = texts.join("\n");
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(anyhow!(
            "{}",
            if text.is_empty() { "the MCP tool reported an error".to_string() } else { text }
        ));
    }
    let mut out = json!({"ok": true, "content": text});
    if let Some(structured) = result.get("structuredContent") {
        out["structured"] = structured.clone();
    }
    Ok(out)
}

fn describe(server: &str, tool: &RemoteTool) -> String {
    let desc: String = tool.description.chars().take(MAX_DESCRIPTION_CHARS).collect();
    format!("[MCP server '{server}'] {desc}").trim_end().to_string()
}

/// Lowercase-safe identifier: `[A-Za-z0-9_-]`, everything else becomes `_`.
pub fn sanitize(raw: &str) -> String {
    let s: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    if s.is_empty() { "_".into() } else { s }
}

/// `mcp__<server>__<tool>`, at most 64 characters.
pub fn full_tool_name(server: &str, tool: &str) -> String {
    let server = sanitize(server);
    let tool = sanitize(tool);
    let mut name = format!("{MCP_TOOL_PREFIX}{server}__{tool}");
    if name.len() > MAX_TOOL_NAME {
        name.truncate(MAX_TOOL_NAME);
    }
    name
}

/// Registered names for one server's tools, de-duplicated after sanitising.
fn tool_names<'a>(server: &str, entry: &'a ServerEntry) -> Vec<(String, &'a RemoteTool)> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for tool in &entry.tools {
        let mut name = full_tool_name(server, &tool.name);
        let mut n = 2;
        while !seen.insert(name.clone()) {
            let suffix = format!("_{n}");
            let mut base = full_tool_name(server, &tool.name);
            base.truncate(MAX_TOOL_NAME - suffix.len());
            name = format!("{base}{suffix}");
            n += 1;
        }
        out.push((name, tool));
    }
    out
}

fn load_disabled(root: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(state_path(root))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| {
            v.get("disabled_tools").and_then(Value::as_array).map(|a| {
                a.iter().filter_map(Value::as_str).map(str::to_string).collect()
            })
        })
        .unwrap_or_default()
}

fn save_disabled(root: &Path, disabled: &BTreeSet<String>) {
    let path = state_path(root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let body = json!({"disabled_tools": disabled});
    if let Err(err) = std::fs::write(&path, body.to_string()) {
        tracing::warn!("[agent_mcp] could not save {}: {err}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_namespaced_sanitised_and_bounded() {
        assert_eq!(full_tool_name("my server", "read.file"), "mcp__my_server__read_file");
        let long = full_tool_name("s", &"x".repeat(200));
        assert_eq!(long.len(), 64);
        assert!(long.starts_with("mcp__s__"));
    }

    #[test]
    fn error_results_become_errors_and_text_is_joined() {
        let ok = convert_result(json!({"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]})).unwrap();
        assert_eq!(ok["content"], "a\nb");
        assert!(convert_result(json!({"isError":true,"content":[{"type":"text","text":"boom"}]}))
            .unwrap_err()
            .to_string()
            .contains("boom"));
    }
}
