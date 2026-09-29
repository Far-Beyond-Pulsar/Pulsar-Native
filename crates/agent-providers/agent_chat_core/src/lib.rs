use std::collections::HashMap;
use std::sync::Arc;

// ── Provider registration layer ─────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    Cloud,
    Local,
}

#[derive(Clone, Debug)]
pub struct ConfigField {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub sensitive: bool,
    pub required: bool,
    pub placeholder: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub struct ProviderConfig {
    pub values: HashMap<String, String>,
}

impl ProviderConfig {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(|s| s.as_str())
    }
    pub fn require(&self, key: &str) -> anyhow::Result<&str> {
        self.values
            .get(key)
            .map(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing required config field: {key}"))
    }
}

#[derive(Clone, Debug)]
pub struct ProviderEntry {
    pub id: &'static str,
    pub display_name: &'static str,
    pub kind: ProviderKind,
    pub default_endpoint: Option<&'static str>,
    pub config_fields: Vec<ConfigField>,
    /// A template is not a provider itself: every configuration the user
    /// saves from it becomes its own named provider (a [`ProviderInstance`],
    /// persisted as a [`ProviderInstanceConfig`]), so one backend kind --
    /// several LM Studio servers, say -- can be connected many times.
    pub template: bool,
}

pub trait ProviderCrate: Send + Sync {
    fn entries(&self) -> Vec<ProviderEntry>;
    fn create(&self, id: &str, config: ProviderConfig) -> anyhow::Result<Box<dyn ChatProvider>>;
}

// ── Runtime chat interface ──────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct ModelDescriptor {
    pub id: String,
    pub label: String,
    pub supports_tools: bool,
    pub context_tokens: u32,
    pub compact_model: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
    AgentEvent,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    pub tool_call_id: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Debug)]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub parameters_json_schema: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments_json: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub enable_tool_calls: bool,
    pub tools: Vec<ToolDefinition>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct ChatResponse {
    pub assistant_message: Option<String>,
    pub streamed_text_chunks: Vec<String>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
    pub raw_response: serde_json::Value,
}

pub trait ChatProvider: Send + Sync {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn config_fields(&self) -> &[ConfigField] {
        &[]
    }

    /// Validate the provider's configuration (e.g. check API key is valid).
    /// Return `Ok(())` on success, or an error with a human-readable message.
    /// The provider decides how to test — lightweight API call, key format check, etc.
    /// Default impl calls `models()`. Override for custom validation.
    fn validate_config(&self) -> anyhow::Result<()> {
        self.models()?;
        Ok(())
    }

    fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>>;
    fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse>;

    fn chat_streaming(
        &self,
        request: ChatRequest,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        let response = self.chat(request)?;
        if response.streamed_text_chunks.is_empty() {
            if let Some(text) = &response.assistant_message {
                on_chunk(text.clone());
            }
        } else {
            for chunk in &response.streamed_text_chunks {
                on_chunk(chunk.clone());
            }
        }
        Ok(response)
    }
}

// ── Provider registry ───────────────────────────────────────────────────────

#[derive(Default)]
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn ChatProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, provider: Arc<dyn ChatProvider>) {
        self.providers.insert(provider.id().to_string(), provider);
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn ChatProvider>> {
        self.providers.get(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.providers.contains_key(id)
    }

    pub fn remove(&mut self, id: &str) {
        self.providers.remove(id);
    }

    pub fn all(&self) -> impl Iterator<Item = (&String, &Arc<dyn ChatProvider>)> {
        self.providers.iter()
    }
}

// ── Provider instances (configured templates) ───────────────────────────────

/// One saved configuration of a template provider: what the user named it,
/// which template it came from, and the config values it was created with.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderInstanceConfig {
    /// Unique provider id, `<template_id>:<slug of name>`.
    pub id: String,
    pub name: String,
    pub template_id: String,
    #[serde(default)]
    pub values: HashMap<String, String>,
}

impl ProviderInstanceConfig {
    /// A new instance of `template_id` named `name`, with an id that is not in
    /// `taken`.
    pub fn new(
        template_id: &str,
        name: &str,
        values: HashMap<String, String>,
        taken: &[&str],
    ) -> Self {
        let slug: String = name
            .trim()
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect::<String>()
            .split('_')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("_");
        let base = format!(
            "{template_id}:{}",
            if slug.is_empty() { "connection" } else { &slug }
        );
        let mut id = base.clone();
        let mut n = 2;
        while taken.contains(&id.as_str()) {
            id = format!("{base}_{n}");
            n += 1;
        }
        Self {
            id,
            name: name.trim().to_string(),
            template_id: template_id.to_string(),
            values,
        }
    }

    pub fn provider_config(&self) -> ProviderConfig {
        ProviderConfig {
            values: self.values.clone(),
        }
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct ProviderInstanceFile {
    #[serde(default)]
    providers: Vec<ProviderInstanceConfig>,
}

/// Read saved instances from `path`. A missing file is an empty list; an
/// unreadable one is an error, so callers never overwrite it blindly.
pub fn load_provider_instances(
    path: &std::path::Path,
) -> anyhow::Result<Vec<ProviderInstanceConfig>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(serde_json::from_str::<ProviderInstanceFile>(&text)?.providers),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

/// Write `instances` to `path`, creating its directory.
pub fn save_provider_instances(
    path: &std::path::Path,
    instances: &[ProviderInstanceConfig],
) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = ProviderInstanceFile {
        providers: instances.to_vec(),
    };
    std::fs::write(path, serde_json::to_string_pretty(&file)?)?;
    Ok(())
}

/// A provider created from a template, running under the instance's own id
/// and name. Everything else is the template's implementation.
pub struct ProviderInstance {
    id: String,
    name: String,
    inner: Box<dyn ChatProvider>,
}

impl ProviderInstance {
    pub fn new(config: &ProviderInstanceConfig, inner: Box<dyn ChatProvider>) -> Self {
        Self {
            id: config.id.clone(),
            name: config.name.clone(),
            inner,
        }
    }
}

impl ChatProvider for ProviderInstance {
    fn id(&self) -> &str {
        &self.id
    }
    fn display_name(&self) -> &str {
        &self.name
    }
    fn config_fields(&self) -> &[ConfigField] {
        self.inner.config_fields()
    }
    fn validate_config(&self) -> anyhow::Result<()> {
        self.inner.validate_config()
    }
    fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        self.inner.models()
    }
    fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        self.inner.chat(request)
    }
    fn chat_streaming(
        &self,
        request: ChatRequest,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        self.inner.chat_streaming(request, on_chunk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_ids_are_slugged_and_unique() {
        let a = ProviderInstanceConfig::new("lm_studio", " Desk GPU #1 ", HashMap::new(), &[]);
        assert_eq!(a.id, "lm_studio:desk_gpu_1");
        assert_eq!(a.name, "Desk GPU #1");
        let b = ProviderInstanceConfig::new("lm_studio", "desk-gpu 1", HashMap::new(), &[&a.id]);
        assert_eq!(b.id, "lm_studio:desk_gpu_1_2");
        let c = ProviderInstanceConfig::new("ollama", "!!!", HashMap::new(), &[]);
        assert_eq!(c.id, "ollama:connection");
    }

    #[test]
    fn instances_round_trip_through_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("configs").join("ai_providers.json");
        assert!(load_provider_instances(&path).unwrap().is_empty());

        let values = HashMap::from([("endpoint_url".to_string(), "http://box:1234/v1".to_string())]);
        let saved = vec![ProviderInstanceConfig::new("lm_studio", "Box", values, &[])];
        save_provider_instances(&path, &saved).unwrap();
        assert_eq!(load_provider_instances(&path).unwrap(), saved);

        std::fs::write(&path, "not json").unwrap();
        assert!(load_provider_instances(&path).is_err());
    }
}
