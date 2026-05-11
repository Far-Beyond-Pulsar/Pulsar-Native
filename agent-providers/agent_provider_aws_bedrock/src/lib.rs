use agent_chat_core::{
    AuthHost, AuthMethod, AuthResult, ChatProvider, ChatRequest, ChatResponse, ModelDescriptor,
    ProviderAvailability, ProviderEnvironment, ProviderKind, ProviderMetadata,
};

const ENDPOINT: &str = "https://bedrock-runtime.us-east-1.amazonaws.com";

pub struct AwsBedrockProvider;

impl AwsBedrockProvider {
    pub fn new() -> Self {
        Self
    }

    fn static_models() -> Vec<ModelDescriptor> {
        vec![
            ModelDescriptor {
                id: "anthropic.claude-opus-4-1",
                label: "Claude Opus 4.1",
                supports_tools: true,
                context_tokens: 200000, compact_model: None,
            },
            ModelDescriptor {
                id: "anthropic.claude-3-7-sonnet",
                label: "Claude 3.7 Sonnet",
                supports_tools: true,
                context_tokens: 200000, compact_model: None,
            },
            ModelDescriptor {
                id: "anthropic.claude-3-5-sonnet-v2",
                label: "Claude 3.5 Sonnet v2",
                supports_tools: true,
                context_tokens: 200000, compact_model: None,
            },
            ModelDescriptor {
                id: "amazon.nova-pro-v1",
                label: "Amazon Nova Pro",
                supports_tools: true,
                context_tokens: 300000, compact_model: None,
            },
            ModelDescriptor {
                id: "meta.llama3-1-70b-instruct-v1",
                label: "Llama 3.1 70B Instruct",
                supports_tools: false,
                context_tokens: 131072, compact_model: None,
            },
        ]
    }
}

impl Default for AwsBedrockProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatProvider for AwsBedrockProvider {
    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            id: "aws_bedrock",
            display_name: "AWS Bedrock",
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
            "AWS Bedrock not yet implemented — requires AWS SigV4 auth. Contribution welcome!"
        )
    }

    fn chat_completion_streaming(
        &self,
        _token: &str,
        _request: &ChatRequest,
        _on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        anyhow::bail!(
            "AWS Bedrock not yet implemented — requires AWS SigV4 auth. Contribution welcome!"
        )
    }
}
