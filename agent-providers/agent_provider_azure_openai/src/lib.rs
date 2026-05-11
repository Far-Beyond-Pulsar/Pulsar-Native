use agent_chat_core::{
    AuthHost, AuthMethod, AuthResult, ChatProvider, ChatRequest, ChatResponse, ModelDescriptor,
    PromptTokenRequest, ProviderAvailability, ProviderEnvironment, ProviderKind, ProviderMetadata,
};

const BASE_URL: &str = "https://YOUR_RESOURCE.openai.azure.com";

pub struct AzureOpenAIProvider;

impl AzureOpenAIProvider {
    pub fn new() -> Self {
        Self
    }

    fn static_models() -> Vec<ModelDescriptor> {
        vec![
            ModelDescriptor {
                id: "gpt-4.1",
                label: "GPT-4.1",
                supports_tools: true,
                context_tokens: 1047576, compact_model: None,
            },
            ModelDescriptor {
                id: "gpt-4.1-mini",
                label: "GPT-4.1 Mini",
                supports_tools: true,
                context_tokens: 1047576, compact_model: None,
            },
            ModelDescriptor {
                id: "gpt-4o",
                label: "GPT-4o",
                supports_tools: true,
                context_tokens: 128000, compact_model: None,
            },
        ]
    }

    fn auth_token_from_env(env: &dyn ProviderEnvironment) -> Option<String> {
        env.get_env("AZURE_OPENAI_API_KEY")
    }
}

impl Default for AzureOpenAIProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatProvider for AzureOpenAIProvider {
    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            id: "azure_openai",
            display_name: "Azure OpenAI",
            endpoint: BASE_URL,
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
        vec![AuthMethod::PromptToken]
    }

    fn authenticate(
        &self,
        method: AuthMethod,
        host: &mut dyn AuthHost,
    ) -> anyhow::Result<AuthResult> {
        match method {
            AuthMethod::PromptToken => {
                let token = host.prompt_for_token(PromptTokenRequest {
                    title: "Azure OpenAI Authentication".to_string(),
                    prompt: "Paste your Azure OpenAI API key.".to_string(),
                    placeholder: None,
                    env_var_hint: Some("AZURE_OPENAI_API_KEY".to_string()),
                })?;

                Ok(match token {
                    Some(token) => AuthResult::Authenticated { token },
                    None => AuthResult::Cancelled,
                })
            }
            AuthMethod::BrowserDeviceCode => Ok(AuthResult::Cancelled),
        }
    }

    fn list_models_api(&self, _token: &str) -> anyhow::Result<Vec<ModelDescriptor>> {
        Ok(Self::static_models())
    }

    fn chat_completion(
        &self,
        _token: &str,
        _request: &ChatRequest,
    ) -> anyhow::Result<ChatResponse> {
        anyhow::bail!("Azure OpenAI not yet implemented — contribution welcome")
    }

    fn chat_completion_streaming(
        &self,
        _token: &str,
        _request: &ChatRequest,
        _on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        anyhow::bail!("Azure OpenAI not yet implemented — contribution welcome")
    }
}
