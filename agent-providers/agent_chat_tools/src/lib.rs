use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};
use reqwest::blocking::Client;

#[derive(Clone)]
pub struct ToolContext {
    pub workspace_root: PathBuf,
    /// Optional plugin tool bridge for accessing plugin tools
    pub plugin_bridge: Option<Arc<RwLock<plugin_manager::PluginToolBridge>>>,
    /// Current file being edited (if any)
    pub current_file: Option<PathBuf>,
    /// Optional callback to open a file through the app's default editor flow.
    pub open_file_request: Option<Arc<dyn Fn(PathBuf) -> Result<(), String> + Send + Sync>>,
    /// Optional callback to query active/inactive open editors tracked by the engine.
    pub query_open_editors: Option<Arc<dyn Fn() -> Result<Value, String> + Send + Sync>>,
    /// Optional callback to activate one of the already-open editor tabs by index.
    pub activate_open_editor_request:
        Option<Arc<dyn Fn(usize) -> Result<(), String> + Send + Sync>>,
}

pub trait ChatTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value>;
}

#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ChatTool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_default_tools() -> Self {
        let mut this = Self::new();
        this.register(Arc::new(OpenFileInDefaultEditorTool));
        this.register(Arc::new(QueryOpenEditorsTool));
        this.register(Arc::new(ActivateOpenEditorTool));
        this.register(Arc::new(QueryAvailableFileTypesTool));
        this.register(Arc::new(QueryFileEditorsTool));
        this.register(Arc::new(QueryPluginToolsTool));
        this.register(Arc::new(QueryToolsForPluginTool));
        this.register(Arc::new(ExecutePluginToolTool));
        this.register(Arc::new(WebSearchTool));
        this.register(Arc::new(FetchUrlTool));
        this
    }

    pub fn register(&mut self, tool: Arc<dyn ChatTool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn execute(&self, name: &str, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let Some(tool) = self.tools.get(name) else {
            return Err(anyhow!("Unknown tool: {name}"));
        };
        tool.execute(args, ctx)
    }

    pub fn available_tools_schema(&self) -> Vec<Value> {
        vec![
            json!({
                "name": "open_file_in_default_editor",
                "description": "Open a file in its default editor tab. Call this before plugin edit tools so edits happen in editor state, not direct file access.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" }
                    },
                    "required": ["file_path"]
                }
            }),
            json!({
                "name": "query_open_editors",
                "description": "List already open editors and indicate which one is active versus inactive.",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }),
            json!({
                "name": "activate_open_editor",
                "description": "Activate one of the already-open editors by its index from query_open_editors.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "index": { "type": "integer", "minimum": 0 }
                    },
                    "required": ["index"]
                }
            }),
            json!({
                "name": "query_available_file_types",
                "description": "List all file types currently registered by plugins/editors.",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }),
            json!({
                "name": "query_file_editors",
                "description": "Query which editors/plugins can handle a specific file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" }
                    },
                    "required": ["file_path"]
                }
            }),
            json!({
                "name": "query_plugin_tools",
                "description": "Query available AI tools from plugins for a specific file. Call query_open_editors first to get the file_path for open editors.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string", "description": "The file path to query tools for. Use the file_path from query_open_editors output." }
                    },
                    "required": ["file_path"]
                }
            }),
            json!({
                "name": "query_tools_for_plugin",
                "description": "Query AI tools provided by a specific plugin, optionally scoped to a file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "plugin_id": { "type": "string", "description": "Plugin id to inspect" },
                        "file_path": { "type": "string", "description": "Optional file path to filter tools by capability for that file" }
                    },
                    "required": ["plugin_id"]
                }
            }),
            json!({
                "name": "execute_plugin_tool",
                "description": "Execute an AI tool provided by a plugin.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "plugin_id": { "type": "string", "description": "Plugin id returned by query_plugin_tools. Recommended when tool names overlap." },
                        "tool_name": { "type": "string", "description": "Name of the tool to execute" },
                        "file_path": { "type": "string", "description": "File to operate on. If not provided, uses current_file from context." },
                        "tool_args": { "type": "object", "description": "Arguments to pass to the tool", "additionalProperties": true }
                    },
                    "required": ["tool_name", "tool_args"]
                }
            }),
            json!({
                "name": "web_search",
                "description": "Search the web using a search engine. Returns up to 10 detailed results with title, summary, and URL.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "The search query (e.g., 'Rust programming language', 'latest AI research 2024')" }
                    },
                    "required": ["query"]
                }
            }),
            json!({
                "name": "fetch_url",
                "description": "Fetch and parse the text content of a URL. Returns the HTML/text content, cleaned of markup.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "The URL to fetch (must start with http:// or https://)" },
                        "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 30, "description": "Timeout in seconds (default 10)" }
                    },
                    "required": ["url"]
                }
            }),
        ]
    }
}

struct OpenFileInDefaultEditorTool;
impl ChatTool for OpenFileInDefaultEditorTool {
    fn name(&self) -> &'static str {
        "open_file_in_default_editor"
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("open_file_in_default_editor.file_path is required"))?;
        let full = resolve_workspace_path(&ctx.workspace_root, file_path)?;

        let callback = ctx
            .open_file_request
            .as_ref()
            .ok_or_else(|| anyhow!("Open-file callback unavailable in this context"))?;
        callback(full.clone()).map_err(|err| anyhow!(err))?;

        Ok(json!({
            "ok": true,
            "file_path": full.display().to_string(),
            "opened": true,
        }))
    }
}

struct QueryOpenEditorsTool;
impl ChatTool for QueryOpenEditorsTool {
    fn name(&self) -> &'static str {
        "query_open_editors"
    }

    fn execute(&self, _args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let callback = ctx
            .query_open_editors
            .as_ref()
            .ok_or_else(|| anyhow!("Open-editors callback unavailable in this context"))?;
        callback().map_err(|err| anyhow!(err))
    }
}

struct ActivateOpenEditorTool;
impl ChatTool for ActivateOpenEditorTool {
    fn name(&self) -> &'static str {
        "activate_open_editor"
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let index = args
            .get("index")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| anyhow!("activate_open_editor.index is required"))?
            as usize;

        let callback = ctx
            .activate_open_editor_request
            .as_ref()
            .ok_or_else(|| anyhow!("Activate-open-editor callback unavailable in this context"))?;
        callback(index).map_err(|err| anyhow!(err))?;

        Ok(json!({
            "ok": true,
            "index": index,
            "activated": true,
        }))
    }
}

struct QueryAvailableFileTypesTool;
impl ChatTool for QueryAvailableFileTypesTool {
    fn name(&self) -> &'static str {
        "query_available_file_types"
    }

    fn execute(&self, _args: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        let manager_lock = plugin_manager::global()
            .ok_or_else(|| anyhow!("Global plugin manager not available"))?;
        let manager = manager_lock
            .read()
            .map_err(|_| anyhow!("Failed to lock plugin manager"))?;

        let file_types = manager
            .file_type_registry()
            .get_all_file_types()
            .into_iter()
            .map(|ft| {
                json!({
                    "id": ft.id.to_string(),
                    "extension": ft.extension,
                    "display_name": ft.display_name,
                    "structure": format!("{:?}", ft.structure),
                })
            })
            .collect::<Vec<_>>();

        Ok(json!({
            "count": file_types.len(),
            "file_types": file_types,
        }))
    }
}

struct QueryFileEditorsTool;
impl ChatTool for QueryFileEditorsTool {
    fn name(&self) -> &'static str {
        "query_file_editors"
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("query_file_editors.file_path is required"))?;
        let full = resolve_workspace_path(&ctx.workspace_root, file_path)?;

        let manager_lock = plugin_manager::global()
            .ok_or_else(|| anyhow!("Global plugin manager not available"))?;
        let manager = manager_lock
            .read()
            .map_err(|_| anyhow!("Failed to lock plugin manager"))?;

        let Some(file_type_id) = manager.file_type_registry().get_file_type_for_path(&full) else {
            return Ok(json!({
                "file_path": full.display().to_string(),
                "file_type": null,
                "editors": [],
            }));
        };

        let editors = manager
            .editor_registry()
            .get_editors_for_file_type(&file_type_id)
            .into_iter()
            .map(|editor_id| {
                let plugin_id = manager
                    .editor_registry()
                    .get_plugin_for_editor(&editor_id)
                    .map(|pid| pid.to_string());
                json!({
                    "editor_id": editor_id.to_string(),
                    "plugin_id": plugin_id,
                })
            })
            .collect::<Vec<_>>();

        Ok(json!({
            "file_path": full.display().to_string(),
            "file_type": file_type_id.to_string(),
            "editors": editors,
        }))
    }
}

struct QueryPluginToolsTool;
impl ChatTool for QueryPluginToolsTool {
    fn name(&self) -> &'static str {
        "query_plugin_tools"
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let file_path_raw = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| p.to_string())
            .or_else(|| ctx.current_file.as_ref().map(|p| p.display().to_string()));

        let Some(file_path_raw) = file_path_raw else {
            return Ok(json!({
                "ok": false,
                "error": "file_path is required. Provide the path of the file you want to query tools for.",
                "tools_available": 0,
                "tools": [],
                "plugins": [],
            }));
        };

        // Use soft resolution so tool discovery works even if the file hasn't been
        // saved to disk yet (e.g. a new unsaved level file open in the editor).
        let full = resolve_workspace_path_soft(&ctx.workspace_root, &file_path_raw)?;
        let file_path_str = full.display().to_string();

        let manager_lock = plugin_manager::global()
            .ok_or_else(|| anyhow!("Global plugin manager not available"))?;
        let manager = manager_lock
            .read()
            .map_err(|_| anyhow!("Failed to lock plugin manager"))?;

        let tools = manager.build_tool_bridge_for_file(&full).all_tools();

        let tool_schemas: Vec<Value> = tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.definition.name,
                    "description": tool.definition.description,
                    "category": tool.definition.category,
                    "parameters": tool.definition.parameters_json_schema,
                    "plugin_id": tool.plugin_id.to_string(),
                })
            })
            .collect();

        let mut tools_by_plugin: HashMap<String, Vec<Value>> = HashMap::new();
        for tool in &tool_schemas {
            if let Some(plugin_id) = tool.get("plugin_id").and_then(|v| v.as_str()) {
                tools_by_plugin
                    .entry(plugin_id.to_string())
                    .or_default()
                    .push(tool.clone());
            }
        }

        let plugins = tools_by_plugin
            .into_iter()
            .map(|(plugin_id, tools)| {
                json!({
                    "plugin_id": plugin_id,
                    "tool_count": tools.len(),
                    "tools": tools,
                })
            })
            .collect::<Vec<_>>();

        Ok(json!({
            "ok": true,
            "file_path": file_path_str,
            "tools_available": tool_schemas.len(),
            "note": if tool_schemas.is_empty() {
                format!(
                    "No tools are registered for '{}'. The file type may not be supported, or the required editor is not loaded. \
                     Check that the file is open in an editor first.",
                    file_path_str
                )
            } else {
                format!("Found {} tool(s) for this file.", tool_schemas.len())
            },
            "tools": tool_schemas,
            "plugins": plugins,
        }))
    }
}

struct QueryToolsForPluginTool;
impl ChatTool for QueryToolsForPluginTool {
    fn name(&self) -> &'static str {
        "query_tools_for_plugin"
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let plugin_id = args
            .get("plugin_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("query_tools_for_plugin.plugin_id is required"))?;

        let manager_lock = plugin_manager::global()
            .ok_or_else(|| anyhow!("Global plugin manager not available"))?;
        let manager = manager_lock
            .read()
            .map_err(|_| anyhow!("Failed to lock plugin manager"))?;

        let tools = if let Some(file_path) = args.get("file_path").and_then(|v| v.as_str()) {
            let full = resolve_workspace_path(&ctx.workspace_root, file_path)?;
            manager
                .build_tool_bridge_for_file(&full)
                .all_tools()
                .into_iter()
                .filter(|tool| tool.plugin_id.to_string() == plugin_id)
                .map(|tool| tool.definition)
                .collect::<Vec<_>>()
        } else {
            manager
                .build_tool_bridge()
                .all_tools()
                .into_iter()
                .filter(|tool| tool.plugin_id.to_string() == plugin_id)
                .map(|tool| tool.definition)
                .collect::<Vec<_>>()
        };

        let tool_schemas = tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "category": tool.category,
                    "parameters": tool.parameters_json_schema,
                    "plugin_id": plugin_id,
                })
            })
            .collect::<Vec<_>>();

        Ok(json!({
            "plugin_id": plugin_id,
            "tools_available": tool_schemas.len(),
            "tools": tool_schemas,
        }))
    }
}

struct ExecutePluginToolTool;
impl ChatTool for ExecutePluginToolTool {
    fn name(&self) -> &'static str {
        "execute_plugin_tool"
    }

    fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let explicit_plugin_id = args.get("plugin_id").and_then(|v| v.as_str());
        let tool_name = args
            .get("tool_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("execute_plugin_tool.tool_name is required"))?;

        let tool_args = args.get("tool_args").cloned().unwrap_or_else(|| json!({}));

        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| PathBuf::from(p))
            .or_else(|| ctx.current_file.clone())
            .ok_or_else(|| anyhow!("No file path provided or available in context"))?;

        // Use soft resolution: the file path from query_open_editors is absolute and canonical,
        // but even if it's relative we don't want to fail just because we can't canonicalize it.
        let full_file_path =
            resolve_workspace_path_soft(&ctx.workspace_root, &file_path.display().to_string())?;

        let manager_lock = plugin_manager::global()
            .ok_or_else(|| anyhow!("Global plugin manager not available"))?;
        let manager = manager_lock
            .read()
            .map_err(|_| anyhow!("Failed to lock plugin manager"))?;

        let bridge = manager.build_tool_bridge();
        let plugin_id = if let Some(plugin_id) = explicit_plugin_id {
            bridge
                .all_tools()
                .into_iter()
                .find(|tool| tool.plugin_id.to_string() == plugin_id)
                .map(|tool| tool.plugin_id)
                .ok_or_else(|| anyhow!("Plugin id not found: {}", plugin_id))?
        } else {
            bridge
                .plugin_for_tool(tool_name)
                .ok_or_else(|| anyhow!("Tool not found or plugin not resolvable: {}", tool_name))?
        };

        let result = manager
            .execute_plugin_ai_tool(&plugin_id, &full_file_path, tool_name, tool_args)
            .map_err(|err| anyhow!(err.to_string()))?;

        Ok(json!({
            "status": "ok",
            "plugin_id": plugin_id.to_string(),
            "tool_name": tool_name,
            "file_path": full_file_path.display().to_string(),
            "result": result,
        }))
    }
}

struct WebSearchTool;
impl ChatTool for WebSearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn execute(&self, args: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("web_search.query is required"))?;

        let max_results = 10;  // Fixed at 10 results

        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("Failed to build HTTP client")?;

        // Use DuckDuckGo API which doesn't require authentication
        let url = format!(
            "https://duckduckgo.com/?q={}&format=json&t=pulsar&no_redirect=1&d_l=en-us",
            urlencoding::encode(query)
        );

        let response = client
            .get(&url)
            .header("User-Agent", "Pulsar-Engine-AI/1.0")
            .send()
            .context("Failed to perform web search")?;

        if !response.status().is_success() {
            return Ok(json!({
                "ok": false,
                "query": query,
                "error": format!("Search API returned status {}", response.status()),
                "results": []
            }));
        }

        let body = response
            .text()
            .context("Failed to read search response")?;

        let json: Value = serde_json::from_str(&body)
            .unwrap_or_else(|_| json!({}));

        // Try multiple sources: RelatedTopics, Results, or AbstractText
        let mut results: Vec<Value> = Vec::new();

        // First, try RelatedTopics (usually for broader queries)
        if let Some(topics) = json.get("RelatedTopics").and_then(|v| v.as_array()) {
            results.extend(
                topics
                    .iter()
                    .take(max_results - results.len())
                    .filter_map(|topic| parse_topic_result(topic))
            );
        }

        // If we still need more results, try Results array (for specific queries)
        if results.len() < max_results {
            if let Some(res_array) = json.get("Results").and_then(|v| v.as_array()) {
                results.extend(
                    res_array
                        .iter()
                        .take(max_results - results.len())
                        .filter_map(|result| parse_result_entry(result))
                );
            }
        }

        // If we still have no results, try to use AbstractText as a fallback
        if results.is_empty() {
            if let Some(abstract_text) = json.get("AbstractText").and_then(|v| v.as_str()) {
                if !abstract_text.is_empty() {
                    let abstract_url = json.get("AbstractURL")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "https://duckduckgo.com".to_string());
                    
                    let summary = if abstract_text.len() > 300 {
                        format!("{}...", &abstract_text[..300])
                    } else {
                        abstract_text.to_string()
                    };
                    
                    results.push(json!({
                        "title": "Direct Answer",
                        "summary": summary,
                        "url": abstract_url,
                        "source": "DuckDuckGo"
                    }));
                }
            }
        }

        Ok(json!({
            "ok": true,
            "query": query,
            "result_count": results.len(),
            "max_results": max_results,
            "results": results,
            "source": "DuckDuckGo"
        }))
    }
}

// Helper function to parse RelatedTopics entries
fn parse_topic_result(topic: &Value) -> Option<Value> {
    let text_content = topic.get("Text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    
    if text_content.is_empty() {
        return None;
    }

    // Split text into title and summary
    // Format is typically "Title - Details" or just details
    let (title, summary) = if let Some(dash_pos) = text_content.find(" - ") {
        let (t, s) = text_content.split_at(dash_pos);
        (t.to_string(), s[3..].to_string()) // Skip " - "
    } else if text_content.len() > 100 {
        let title = text_content[..100].to_string();
        (title, text_content.clone())
    } else {
        (text_content.clone(), text_content.clone())
    };

    let url = topic.get("FirstURL")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();

    // Truncate summary to ~300 chars for detailed but concise info
    let summary = if summary.len() > 300 {
        format!("{}...", &summary[..300])
    } else {
        summary
    };

    Some(json!({
        "title": title,
        "summary": summary,
        "url": url,
        "source": "DuckDuckGo"
    }))
}

// Helper function to parse Results array entries (used when RelatedTopics is empty)
fn parse_result_entry(result: &Value) -> Option<Value> {
    let title = result.get("Title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            result.get("Text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })?;

    if title.is_empty() {
        return None;
    }

    let summary = result.get("Content")
        .or_else(|| result.get("Text"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| title.clone());

    // Truncate summary
    let summary = if summary.len() > 300 {
        format!("{}...", &summary[..300])
    } else {
        summary
    };

    let url = result.get("FirstURL")
        .or_else(|| result.get("URL"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();

    Some(json!({
        "title": title,
        "summary": summary,
        "url": url,
        "source": "DuckDuckGo"
    }))
}

struct FetchUrlTool;
impl ChatTool for FetchUrlTool {
    fn name(&self) -> &'static str {
        "fetch_url"
    }

    fn execute(&self, args: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("fetch_url.url is required"))?;

        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Ok(json!({
                "ok": false,
                "url": url,
                "error": "URL must start with http:// or https://",
                "content": null
            }));
        }

        let timeout_secs = args
            .get("timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .max(1)
            .min(30);

        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .context("Failed to build HTTP client")?;

        let response = match client
            .get(url)
            .header("User-Agent", "Pulsar-Engine-AI/1.0")
            .send()
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(json!({
                    "ok": false,
                    "url": url,
                    "error": format!("Failed to fetch URL: {}", e),
                    "content": null
                }));
            }
        };

        if !response.status().is_success() {
            return Ok(json!({
                "ok": false,
                "url": url,
                "status_code": response.status().as_u16(),
                "error": format!("HTTP {}", response.status()),
                "content": null
            }));
        }

        let content = match response.text() {
            Ok(text) => text,
            Err(e) => {
                return Ok(json!({
                    "ok": false,
                    "url": url,
                    "error": format!("Failed to read response: {}", e),
                    "content": null
                }));
            }
        };

        // Basic HTML stripping: remove script/style tags and common HTML markup
        let cleaned = strip_html_tags(&content);
        let truncated = if cleaned.len() > 8000 {
            format!("{}... [truncated]", &cleaned[..8000])
        } else {
            cleaned
        };

        Ok(json!({
            "ok": true,
            "url": url,
            "status_code": 200,
            "content_length": truncated.len(),
            "content": truncated
        }))
    }
}

fn strip_html_tags(html: &str) -> String {
    // Simple HTML tag removal
    let mut result = String::new();
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;

    let lower = html.to_lowercase();
    let bytes = html.as_bytes();

    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b'<' {
            in_tag = true;
            // Check for script or style tags
            if lower[i..].starts_with("<script") {
                in_script = true;
            } else if lower[i..].starts_with("<style") {
                in_style = true;
            }
        } else if byte == b'>' {
            in_tag = false;
            if lower[i..].starts_with("</script>") {
                in_script = false;
            } else if lower[i..].starts_with("</style>") {
                in_style = false;
            }
            if !in_script && !in_style {
                result.push(' '); // Add space after closing tag for word separation
            }
        } else if !in_tag && !in_script && !in_style {
            result.push(byte as char);
        }
    }

    // Clean up whitespace
    result
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn resolve_workspace_path(root: &Path, rel_or_abs: &str) -> anyhow::Result<PathBuf> {
    let p = PathBuf::from(rel_or_abs);
    let joined = if p.is_absolute() { p } else { root.join(p) };
    let canonical = joined
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", joined.display()))?;

    let root_canonical = root
        .canonicalize()
        .with_context(|| format!("Workspace root missing: {}", root.display()))?;

    if !canonical.starts_with(&root_canonical) {
        return Err(anyhow!("Path escapes workspace root"));
    }
    Ok(canonical)
}

/// Resolve a workspace-relative path, canonicalizing it if the file exists on disk
/// or falling back to manual component normalization if it doesn't.
///
/// This is used for both tool discovery (extension check only) and tool execution
/// (session lookup), so it must return the canonical path when the file is on disk
/// — otherwise `ai_sessions::get_open_scene_state` won't find the registered entry.
fn resolve_workspace_path_soft(root: &Path, rel_or_abs: &str) -> anyhow::Result<PathBuf> {
    let p = PathBuf::from(rel_or_abs);
    let joined = if p.is_absolute() { p } else { root.join(&p) };

    // Happy path: if the file is on disk, canonicalize gives us the exact path
    // that was stored in ai_sessions (which also canonicalizes on registration).
    if let Ok(canonical) = joined.canonicalize() {
        // Security check: must still be inside the workspace root.
        // Use the canonical root if available, otherwise skip the check for dot-roots.
        if let Ok(root_canonical) = root.canonicalize() {
            if !canonical.starts_with(&root_canonical) {
                return Err(anyhow!("Path escapes workspace root"));
            }
        }
        return Ok(canonical);
    }

    // File doesn't exist yet on disk — normalize manually without requiring existence.
    let mut components = Vec::new();
    for part in joined.components() {
        use std::path::Component;
        match part {
            Component::ParentDir => {
                components.pop();
            }
            Component::CurDir => {}
            other => components.push(other),
        }
    }
    let normalized: PathBuf = components.iter().collect();

    let root_normalized: PathBuf = {
        let mut c = Vec::new();
        for part in root.components() {
            use std::path::Component;
            match part {
                Component::ParentDir => {
                    c.pop();
                }
                Component::CurDir => {}
                other => c.push(other),
            }
        }
        c.iter().collect()
    };

    // Only enforce root prefix when we have a meaningful absolute root.
    if root_normalized.is_absolute() && !normalized.starts_with(&root_normalized) {
        return Err(anyhow!("Path escapes workspace root"));
    }
    Ok(normalized)
}
