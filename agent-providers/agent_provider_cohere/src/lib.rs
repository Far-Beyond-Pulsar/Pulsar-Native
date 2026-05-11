use agent_chat_core::{
    AuthHost, AuthMethod, AuthResult, ChatProvider, ChatRequest, ChatResponse, ModelDescriptor,
    ProviderAvailability, ProviderEnvironment, ProviderKind, ProviderMetadata,
};

const ENDPOINT: &str = "https://api.cohere.com/v2";

pub struct CohereProvider;

impl CohereProvider {
    pub fn new() -> Self {
        Self
    }

    fn static_models() -> Vec<ModelDescriptor> {
        vec![
            ModelDescriptor {
                id: "command-a-03-2025",
                label: "Command A (Mar 2025)",
                supports_tools: true,
                context_tokens: 256000,
            },
            ModelDescriptor {
                id: "command-r-plus",
                label: "Command R+",
                supports_tools: true,
                context_tokens: 128000,
            },
            ModelDescriptor {
                id: "command-r",
                label: "Command R",
                supports_tools: true,
                context_tokens: 128000,
            },
        ]
    }
}

impl Default for CohereProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatProvider for CohereProvider {
    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            id: "cohere",
            display_name: "Cohere",
            endpoint: ENDPOINT,
            kind: ProviderKind::Cloud,
        }
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        Self::static_models()
    }

    fn availability(&self, _env: &dyn ProviderEnvironment) -> ProviderAvailability {
        ProviderAvailability::wip("Not yet implemented — contribution welcome")
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
            "Cohere not yet implemented — uses Cohere's v2 chat API format. Contribution welcome!"
        )
    }

    fn chat_completion_streaming(
        &self,
        _token: &str,
        _request: &ChatRequest,
        _on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        anyhow::bail!(
            "Cohere not yet implemented — uses Cohere's v2 chat API format. Contribution welcome!"
        )
    }
}
