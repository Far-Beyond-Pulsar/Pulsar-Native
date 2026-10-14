//! `.pulsar/mcp.json` (project) and `~/.pulsar/mcp.json` (user-global).
//!
//! ```json
//! { "mcpServers": { "files": {
//!     "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "."],
//!     "env": {}, "enabled": true, "timeout_secs": 30,
//!     "read_only_tools": ["read_file", "list_directory"]
//! } } }
//! ```
//!
//! A project entry replaces a global entry of the same name.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};

pub const CONFIG_FILE: &str = "mcp.json";
/// Per-project record of the tools the user switched off in the panel.
pub const STATE_FILE: &str = "mcp_state.json";

pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Executable to launch (stdio transport).
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// Working directory; defaults to the workspace root.
    pub cwd: Option<String>,
    /// Streamable-HTTP endpoint. Recognised but not supported yet: such a
    /// server is listed as failed instead of being silently dropped.
    pub url: Option<String>,
    pub enabled: bool,
    /// Per request (initialize, tools/list, each tools/call).
    pub timeout_secs: u64,
    /// Tools the user vouches for as read-only. Everything else is write-class.
    pub read_only_tools: Vec<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            url: None,
            enabled: true,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            read_only_tools: Vec::new(),
        }
    }
}

impl ServerConfig {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.max(1))
    }

    pub fn is_read_only(&self, tool: &str) -> bool {
        self.read_only_tools.iter().any(|t| t == tool)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default, rename = "mcpServers", alias = "servers")]
    pub servers: BTreeMap<String, ServerConfig>,
}

impl McpConfig {
    pub fn parse(raw: &str) -> Result<Self, String> {
        serde_json::from_str(raw).map_err(|e| e.to_string())
    }

    fn read(path: &Path) -> Self {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match Self::parse(&raw) {
            Ok(cfg) => cfg,
            Err(err) => {
                tracing::warn!("[agent_mcp] ignoring {}: {err}", path.display());
                Self::default()
            }
        }
    }

    /// Global file first, project entries override. A missing or broken file
    /// is skipped, never fatal.
    pub fn load(workspace_root: &Path) -> Self {
        let mut merged = global_config_path()
            .map(|p| Self::read(&p))
            .unwrap_or_default();
        let project = Self::read(&project_config_path(workspace_root));
        merged.servers.extend(project.servers);
        merged
    }
}

pub fn project_config_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".pulsar").join(CONFIG_FILE)
}

pub fn global_config_path() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".pulsar").join(CONFIG_FILE))
}

pub fn state_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".pulsar").join(STATE_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_style_config_with_defaults() {
        let cfg = McpConfig::parse(
            r#"{"mcpServers":{"fs":{"command":"npx","args":["-y","x"],"read_only_tools":["read"]}}}"#,
        )
        .unwrap();
        let fs = &cfg.servers["fs"];
        assert_eq!(fs.command.as_deref(), Some("npx"));
        assert!(fs.enabled);
        assert_eq!(fs.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert!(fs.is_read_only("read"));
        assert!(!fs.is_read_only("write"));
    }

    #[test]
    fn accepts_servers_alias_and_rejects_garbage() {
        assert!(McpConfig::parse(r#"{"servers":{"a":{"command":"x"}}}"#)
            .unwrap()
            .servers
            .contains_key("a"));
        assert!(McpConfig::parse("not json").is_err());
    }

    #[test]
    fn project_overrides_global_by_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".pulsar")).unwrap();
        std::fs::write(
            project_config_path(dir.path()),
            r#"{"mcpServers":{"a":{"command":"proj"}}}"#,
        )
        .unwrap();
        let cfg = McpConfig::load(dir.path());
        assert_eq!(cfg.servers["a"].command.as_deref(), Some("proj"));
    }
}
