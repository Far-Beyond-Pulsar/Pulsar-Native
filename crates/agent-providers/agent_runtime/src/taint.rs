//! Tool classification and taint tracking.
//!
//! A run is *tainted* once untrusted content from the web, from a file outside
//! the workspace, or from a sub-agent has entered its context. While tainted,
//! write tools need the user's confirmation (see [`crate::confirm`]).

use std::{
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use serde_json::Value;

use crate::ledger::{EntryKind, Ledger, LedgerEntry, Source};

/// What a tool can do to the outside world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolClass {
    /// Looks at state, changes nothing.
    Read,
    /// Edits files, runs commands, writes the scene, or has unknown side
    /// effects.
    Write,
    /// Talks to the network; its results are untrusted web content.
    External,
}

/// When tool calls need the user's approval. This is the user setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfirmPolicy {
    /// Every write and external call asks.
    AlwaysAsk,
    /// Writes ask once untrusted content is in the run's context.
    #[default]
    AskWhenTainted,
    /// Never ask. Checkpoints still protect file edits.
    Auto,
}

impl ConfirmPolicy {
    pub const ALL: [ConfirmPolicy; 3] = [
        ConfirmPolicy::AlwaysAsk,
        ConfirmPolicy::AskWhenTainted,
        ConfirmPolicy::Auto,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ConfirmPolicy::AlwaysAsk => "always-ask",
            ConfirmPolicy::AskWhenTainted => "ask-when-tainted",
            ConfirmPolicy::Auto => "auto",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ConfirmPolicy::AlwaysAsk => "Always ask",
            ConfirmPolicy::AskWhenTainted => "Ask when tainted",
            ConfirmPolicy::Auto => "Auto",
        }
    }

    /// Unknown strings fall back to the default (the safe middle setting).
    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "always-ask" => ConfirmPolicy::AlwaysAsk,
            "auto" => ConfirmPolicy::Auto,
            _ => ConfirmPolicy::AskWhenTainted,
        }
    }
}

/// Read / write / external table. Unknown tools are `Write`: a new tool is
/// gated until someone classifies it.
#[derive(Clone, Debug, Default)]
pub struct ToolClassifier {
    overrides: Vec<(String, ToolClass)>,
}

const READ_TOOLS: &[&str] = &[
    "cd",
    "list_files",
    "tree",
    "task_list_read",
    "task_list_update",
    "open_file_in_default_editor",
    "query_open_editors",
    "activate_open_editor",
    "query_available_file_types",
    "query_file_editors",
    "query_plugin_tools",
    "query_tools_for_plugin",
    "spawn_subagent",
    "await_subagent",
    "query_running_subagents",
    "get_subagent_result",
    "cancel_subagent",
    "read_run",
];

const EXTERNAL_TOOLS: &[&str] = &["web_search", "fetch_url"];

/// Prefix of every tool that comes from an external MCP server
/// (`mcp__<server>__<tool>`). Such a tool is `Write`-class unless the project
/// config or an override says otherwise, and its result is always untrusted.
pub const MCP_TOOL_PREFIX: &str = "mcp__";

/// Is this tool served by an external MCP server?
pub fn is_mcp_tool(name: &str) -> bool {
    name.starts_with(MCP_TOOL_PREFIX)
}

/// Tools whose result is a sub-agent's output, which is untrusted.
const SUBAGENT_RESULT_TOOLS: &[&str] = &["await_subagent", "get_subagent_result", "read_run"];

/// Read tools that fetch file contents, and the argument keys that name the
/// file. Reading outside the workspace taints.
const FILE_READ_TOOLS: &[&str] = &["read_file", "view_file", "cat", "open_file"];

pub(crate) const PATH_KEYS: &[&str] = &[
    "path",
    "file_path",
    "filepath",
    "file",
    "filename",
    "target",
    "target_path",
    "source",
    "destination",
    "dest",
    "from",
    "to",
    "new_path",
    "old_path",
];
pub(crate) const PATH_LIST_KEYS: &[&str] = &["paths", "files"];

impl ToolClassifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Classify `name` explicitly, e.g. for a plugin tool known to be pure.
    pub fn with_override(mut self, name: impl Into<String>, class: ToolClass) -> Self {
        self.set(name, class);
        self
    }

    pub fn set(&mut self, name: impl Into<String>, class: ToolClass) {
        let name = name.into();
        self.overrides.retain(|(n, _)| *n != name);
        self.overrides.push((name, class));
    }

    pub fn classify(&self, name: &str) -> ToolClass {
        if let Some((_, class)) = self.overrides.iter().find(|(n, _)| n == name) {
            return *class;
        }
        if EXTERNAL_TOOLS.contains(&name) {
            return ToolClass::External;
        }
        if READ_TOOLS.contains(&name) || FILE_READ_TOOLS.contains(&name) {
            return ToolClass::Read;
        }
        // MCP tools fall through to Write like every unknown tool: a tool
        // marked read-only in the config gets an explicit `Read` override.
        // `call_plugin_tool` / `execute_plugin_tool` run arbitrary plugin
        // code, so they fall through to Write like every unknown tool.
        ToolClass::Write
    }

    /// Does the result of this call put untrusted content in the context?
    pub fn taints_on_result(&self, name: &str, args: &Value, workspace_root: &Path) -> bool {
        // An MCP server is outside our trust boundary, read-only or not.
        if is_mcp_tool(name) {
            return true;
        }
        match self.classify(name) {
            ToolClass::External => true,
            _ if SUBAGENT_RESULT_TOOLS.contains(&name) => true,
            ToolClass::Read if FILE_READ_TOOLS.contains(&name) => {
                path_args(args)
                    .iter()
                    .any(|p| is_untrusted_path(p, workspace_root))
            }
            _ => false,
        }
    }
}

/// Every path-looking string argument of a call.
pub(crate) fn path_args(args: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(obj) = args.as_object() else {
        return out;
    };
    for key in PATH_KEYS {
        if let Some(s) = obj.get(*key).and_then(Value::as_str) {
            if !s.is_empty() {
                out.push(s.to_string());
            }
        }
    }
    for key in PATH_LIST_KEYS {
        if let Some(items) = obj.get(*key).and_then(Value::as_array) {
            out.extend(
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
            );
        }
    }
    out
}

/// A path is untrusted when it leaves the workspace (including by `..`).
/// Cloud paths are treated as inside the project they name.
pub fn is_untrusted_path(raw: &str, workspace_root: &Path) -> bool {
    if raw.starts_with("cloud+pulsar://") {
        return false;
    }
    let path = Path::new(raw);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    };
    !lexical_normalize(&joined).starts_with(lexical_normalize(workspace_root))
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The per-run taint flag. Clones share it; it only ever goes from clean to
/// tainted.
#[derive(Clone, Debug, Default)]
pub struct TaintTracker(Arc<AtomicBool>);

impl TaintTracker {
    pub fn new(initially_tainted: bool) -> Self {
        Self(Arc::new(AtomicBool::new(initially_tainted)))
    }

    pub fn mark(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_tainted(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Does this entry carry untrusted web/file/sub-agent content?
pub fn entry_taints(entry: &LedgerEntry, classifier: &ToolClassifier, workspace_root: &Path) -> bool {
    match &entry.kind {
        EntryKind::ToolExchange(exchange) => classifier.taints_on_result(
            &exchange.call.name,
            &exchange.call.arguments_json,
            workspace_root,
        ),
        EntryKind::SubagentReport(_) => true,
        // Entries reloaded from disk with an untrusted provenance tag.
        _ => matches!(entry.source, Source::Tool { .. } | Source::Subagent { .. }),
    }
}

impl Ledger {
    /// True when any entry in the window carries untrusted content. Seeds the
    /// taint flag of a run that continues an existing chat.
    pub fn is_tainted(&self, classifier: &ToolClassifier, workspace_root: &Path) -> bool {
        self.entries
            .iter()
            .any(|entry| entry_taints(entry, classifier, workspace_root))
    }
}
