//! What a tool is: its registration, its result, and the hints that say what
//! it may do.
//!
//! A tool is registered, never wired to the server by hand. The engine's own
//! crates submit a [`McpToolRegistration`] with `inventory::submit!` (origin
//! `pulsar_plugins::engine_origin!()`); a plugin adds one from its root crate's
//! `build` (`p.add(McpToolRegistration { .. })`) and so it crosses the plugin
//! library boundary like any other contribution, gated by the plugin being
//! enabled. The server lists whatever is registered (`tools/list`) and calls it
//! (`tools/call`); no tool is named anywhere in the server.
//!
//! Everything that crosses the boundary is plain std data: the arguments arrive
//! as JSON text and the result is a [`ToolResult`] of strings, so a library
//! built with other `serde_json` features cannot disagree about a layout.

use base64::Engine as _;
use serde_json::{Value, json};

/// What a tool says about its effects (MCP tool annotations). A client may use
/// them to decide what to ask the person about; the server also refuses nothing
/// on their account: they describe, they do not enforce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Annotations {
    /// Reads state and changes nothing.
    pub read_only: bool,
    /// May destroy something a person made (delete, overwrite, stop a run).
    /// Meaningful only when not `read_only`.
    pub destructive: bool,
    /// Calling it twice with the same arguments has no further effect.
    pub idempotent: bool,
}

impl Annotations {
    /// Reads only.
    pub const READ_ONLY: Self = Self {
        read_only: true,
        destructive: false,
        idempotent: true,
    };
    /// Changes state, nothing is lost by it (selects, moves a camera, starts a build).
    pub const WRITES: Self = Self {
        read_only: false,
        destructive: false,
        idempotent: false,
    };
    /// Changes state and the same call again changes nothing more.
    pub const WRITES_IDEMPOTENT: Self = Self {
        read_only: false,
        destructive: false,
        idempotent: true,
    };
    /// May delete or overwrite work (deletes objects, saves over a file, stops the game).
    pub const DESTRUCTIVE: Self = Self {
        read_only: false,
        destructive: true,
        idempotent: false,
    };

    pub(crate) fn to_json(self, title: &str) -> Value {
        json!({
            "title": title,
            "readOnlyHint": self.read_only,
            "destructiveHint": self.destructive,
            "idempotentHint": self.idempotent,
            // Tools act on this editor only, never on the open world.
            "openWorldHint": false,
        })
    }
}

/// One piece of a tool's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Text(String),
    /// A picture: base64 data and its media type (`image/png`).
    Image { mime: String, base64: String },
}

/// What a tool returns: text for the model to read, optionally the same facts
/// as structured JSON, and pictures.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ToolResult {
    pub content: Vec<Content>,
    /// JSON text of the structured result, if the tool has one.
    pub structured: Option<String>,
    /// The call failed: the text says why, in terms the caller can act on.
    pub is_error: bool,
}

impl ToolResult {
    /// A text answer.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text(text.into())],
            ..Self::default()
        }
    }

    /// A structured answer: the JSON is both the structured result and, as
    /// text, what a client that reads only text sees.
    pub fn json(value: &Value) -> Self {
        let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
        Self {
            content: vec![Content::Text(text)],
            structured: Some(value.to_string()),
            is_error: false,
        }
    }

    /// A failed call. `message` should say what to do instead.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text(message.into())],
            structured: None,
            is_error: true,
        }
    }

    /// Add a picture.
    pub fn with_image(mut self, mime: impl Into<String>, bytes: &[u8]) -> Self {
        self.content.push(Content::Image {
            mime: mime.into(),
            base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        });
        self
    }

    pub(crate) fn to_json(&self) -> Value {
        let content: Vec<Value> = self
            .content
            .iter()
            .map(|part| match part {
                Content::Text(text) => json!({ "type": "text", "text": text }),
                Content::Image { mime, base64 } => {
                    json!({ "type": "image", "data": base64, "mimeType": mime })
                }
            })
            .collect();
        let mut out = json!({ "content": content, "isError": self.is_error });
        if let Some(structured) = self
            .structured
            .as_deref()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
        {
            out["structuredContent"] = structured;
        }
        out
    }
}

/// A tool a crate or plugin contributes. Add it with
/// `inventory::submit!` (the engine) or `PluginBuilder::add` (a plugin).
pub struct McpToolRegistration {
    /// The package that submits it (`pulsar_plugins::origin!()`, or
    /// `engine_origin!()` for the engine's own).
    pub origin: &'static str,
    /// The tool's name: lowercase words joined by `_`, unique among all tools
    /// (`build_start`, a plugin's own prefixed with its kind: `voxel_bake`).
    pub name: &'static str,
    /// A short title a person reads.
    pub title: &'static str,
    /// What it does, what its arguments mean (units, ranges) and what to call
    /// next: written for a model that has never seen the engine.
    pub description: &'static str,
    /// The JSON Schema of its arguments, as JSON text: an object schema with
    /// `properties` (each with a `description`) and `required`.
    pub input_schema: &'static str,
    pub annotations: Annotations,
    /// Runs the tool: the arguments as JSON text (an object), the result.
    /// Called on a server worker, never the UI thread; a tool that needs the UI
    /// thread hops to it itself.
    pub call: fn(&str) -> ToolResult,
}

impl pulsar_plugins::Contribution for McpToolRegistration {
    const CATEGORY: &'static str = "mcp.tools";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.name.to_owned()
    }

    fn label(&self) -> String {
        self.title.to_owned()
    }
}

pulsar_plugins::registration!(McpToolRegistration);

/// A tool as the protocol lists it.
#[derive(Clone, Debug)]
pub struct ToolInfo {
    pub name: String,
    pub title: String,
    pub description: String,
    pub input_schema: Value,
    pub annotations: Annotations,
    /// Who provides it: `engine`, a plugin's id, or a provider's name.
    pub source: String,
}

impl ToolInfo {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "title": self.title,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": self.annotations.to_json(&self.title),
        })
    }
}

/// Parse a registration's schema text; a schema that is not a JSON object is
/// replaced by the empty object schema (and logged), so one bad tool cannot
/// break `tools/list`.
pub(crate) fn schema_of(name: &str, text: &str) -> Value {
    match serde_json::from_str::<Value>(text) {
        Ok(value) if value.is_object() => value,
        _ => {
            tracing::warn!(tool = name, "the tool's input schema is not a JSON object");
            json!({ "type": "object", "properties": {} })
        }
    }
}
