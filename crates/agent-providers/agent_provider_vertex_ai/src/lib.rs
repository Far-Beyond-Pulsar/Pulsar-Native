use agent_chat_core::{ChatProvider, ChatRequest, ChatResponse, ModelDescriptor};

pub struct VertexAiProvider {
    models: Vec<ModelDescriptor>,
}

impl VertexAiProvider {
    pub fn new() -> Self {
        Self {
            models: Self::static_models(),
        }
    }

    fn static_models() -> Vec<ModelDescriptor> {
        vec![
            ModelDescriptor {
                id: "anthropic/claude-opus-4-1".to_string(),
                label: "Claude Opus 4.1".to_string(),
                supports_tools: true,
                context_tokens: 200000,
                compact_model: None,
            },
            ModelDescriptor {
                id: "anthropic/claude-3-7-sonnet".to_string(),
                label: "Claude 3.7 Sonnet".to_string(),
                supports_tools: true,
                context_tokens: 200000,
                compact_model: None,
            },
            ModelDescriptor {
                id: "google/gemini-2.5-pro".to_string(),
                label: "Gemini 2.5 Pro".to_string(),
                supports_tools: true,
                context_tokens: 1048576,
                compact_model: None,
            },
            ModelDescriptor {
                id: "google/gemini-2.5-flash".to_string(),
                label: "Gemini 2.5 Flash".to_string(),
                supports_tools: true,
                context_tokens: 1048576,
                compact_model: None,
            },
        ]
    }
}

impl Default for VertexAiProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatProvider for VertexAiProvider {
    fn id(&self) -> &str {
        "vertex_ai"
    }

    fn display_name(&self) -> &str {
        "Vertex AI"
    }

    fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        Ok(self.models.clone())
    }

    fn chat(&self, _request: ChatRequest) -> anyhow::Result<ChatResponse> {
        anyhow::bail!(
            "Vertex AI not yet implemented — requires Google OAuth 2.0. Contribution welcome!"
        )
    }

    fn chat_streaming(
        &self,
        _request: ChatRequest,
        _on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        anyhow::bail!(
            "Vertex AI not yet implemented — requires Google OAuth 2.0. Contribution welcome!"
        )
    }
}

use agent_chat_core::{ConfigField, ProviderConfig, ProviderCrate, ProviderEntry, ProviderKind};

pub struct VertexAiProviderCrate;

impl ProviderCrate for VertexAiProviderCrate {
    fn entries(&self) -> Vec<ProviderEntry> {
        vec![ProviderEntry {
            id: "vertex_ai",
            display_name: "Vertex AI",
            kind: ProviderKind::Cloud,
            default_endpoint: None,
            config_fields: vec![ConfigField {
                key: "info",
                label: "Google Cloud Configuration",
                description: "Vertex AI uses standard GCP credential chain (gcloud auth, GOOGLE_APPLICATION_CREDENTIALS)",
                sensitive: false,
                required: false,
                placeholder: None,
            }],
        }]
    }

    fn create(&self, id: &str, _config: ProviderConfig) -> anyhow::Result<Box<dyn ChatProvider>> {
        anyhow::ensure!(id == "vertex_ai", "unknown provider: {id}");
        Ok(Box::new(VertexAiProvider::new()))
    }
}
