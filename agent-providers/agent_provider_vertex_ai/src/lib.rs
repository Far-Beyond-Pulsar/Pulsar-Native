use agent_chat_core::{
    AuthHost, AuthMethod, AuthResult, ChatProvider, ChatRequest, ChatResponse, ModelDescriptor,
    ProviderAvailability, ProviderEnvironment, ProviderKind, ProviderMetadata,
};

const ENDPOINT: &str = "https://aiplatform.googleapis.com";

pub struct VertexAiProvider;

impl VertexAiProvider {
    pub fn new() -> Self {
        Self
    }

    fn static_models() -> Vec<ModelDescriptor> {
        vec![
            ModelDescriptor {
                id: "anthropic/claude-opus-4-1",
                label: "Claude Opus 4.1",
                supports_tools: true,
                context_tokens: 200000,
            },
            ModelDescriptor {
                id: "anthropic/claude-3-7-sonnet",
                label: "Claude 3.7 Sonnet",
                supports_tools: true,
                context_tokens: 200000,
            },
            ModelDescriptor {
                id: "google/gemini-2.5-pro",
                label: "Gemini 2.5 Pro",
                supports_tools: true,
                context_tokens: 1048576,
            },
            ModelDescriptor {
                id: "google/gemini-2.5-flash",
                label: "Gemini 2.5 Flash",
                supports_tools: true,
                context_tokens: 1048576,
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
    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            id: "vertex_ai",
            display_name: "Vertex AI",
            endpoint: ENDPOINT,
            kind: ProviderKind::Cloud,
        }
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        Self::static_models()
    }

    fn availability(&self, _env: &dyn ProviderEnvironment) -> ProviderAvailability {
        ProviderAvailability::requires_auth("Google service account credentials")
    }

    fn auth_methods(&self) -> Vec<AuthMethod> {
        vec![]
    }

    fn authenticate(
        &self,
        _method: AuthMethod,
        _host: &mut dyn AuthHost,
    ) -> anyhow::Result<AuthResult> {
        Ok(AuthResult::Cancelled)
    }

    fn list_models_api(&self, _token: &str) -> anyhow::Result<Vec<ModelDescriptor>> {
        Ok(Self::static_models())
    }

    fn chat_completion(
        &self,
        _token: &str,
        _request: &ChatRequest,
    ) -> anyhow::Result<ChatResponse> {
        anyhow::bail!(
            "Vertex AI not yet implemented — requires Google OAuth 2.0. Contribution welcome!"
        )
    }

    fn chat_completion_streaming(
        &self,
        _token: &str,
        _request: &ChatRequest,
        _on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        anyhow::bail!(
            "Vertex AI not yet implemented — requires Google OAuth 2.0. Contribution welcome!"
        )
    }
}
