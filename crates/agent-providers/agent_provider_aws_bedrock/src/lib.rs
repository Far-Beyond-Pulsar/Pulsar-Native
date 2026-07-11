use agent_chat_core::{ChatProvider, ChatRequest, ChatResponse, ModelDescriptor};

pub struct AwsBedrockProvider {
    models: Vec<ModelDescriptor>,
}

impl AwsBedrockProvider {
    pub fn new() -> Self {
        Self {
            models: Self::static_models(),
        }
    }

    fn static_models() -> Vec<ModelDescriptor> {
        vec![
            ModelDescriptor {
                id: "anthropic.claude-opus-4-1".to_string(),
                label: "Claude Opus 4.1".to_string(),
                supports_tools: true,
                context_tokens: 200000,
                compact_model: None,
            },
            ModelDescriptor {
                id: "anthropic.claude-3-7-sonnet".to_string(),
                label: "Claude 3.7 Sonnet".to_string(),
                supports_tools: true,
                context_tokens: 200000,
                compact_model: None,
            },
            ModelDescriptor {
                id: "anthropic.claude-3-5-sonnet-v2".to_string(),
                label: "Claude 3.5 Sonnet v2".to_string(),
                supports_tools: true,
                context_tokens: 200000,
                compact_model: None,
            },
            ModelDescriptor {
                id: "amazon.nova-pro-v1".to_string(),
                label: "Amazon Nova Pro".to_string(),
                supports_tools: true,
                context_tokens: 300000,
                compact_model: None,
            },
            ModelDescriptor {
                id: "meta.llama3-1-70b-instruct-v1".to_string(),
                label: "Llama 3.1 70B Instruct".to_string(),
                supports_tools: false,
                context_tokens: 131072,
                compact_model: None,
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
    fn id(&self) -> &str {
        "aws_bedrock"
    }

    fn display_name(&self) -> &str {
        "AWS Bedrock"
    }

    fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        Ok(self.models.clone())
    }

    fn chat(&self, _request: ChatRequest) -> anyhow::Result<ChatResponse> {
        anyhow::bail!(
            "AWS Bedrock not yet implemented — requires AWS SigV4 auth. Contribution welcome!"
        )
    }

    fn chat_streaming(
        &self,
        _request: ChatRequest,
        _on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        anyhow::bail!(
            "AWS Bedrock not yet implemented — requires AWS SigV4 auth. Contribution welcome!"
        )
    }
}

use agent_chat_core::{ConfigField, ProviderConfig, ProviderCrate, ProviderEntry, ProviderKind};

pub struct AwsBedrockProviderCrate;

impl ProviderCrate for AwsBedrockProviderCrate {
    fn entries(&self) -> Vec<ProviderEntry> {
        vec![ProviderEntry {
            id: "aws_bedrock",
            display_name: "AWS Bedrock",
            kind: ProviderKind::Cloud,
            default_endpoint: None,
            config_fields: vec![ConfigField {
                key: "info",
                label: "AWS Configuration",
                description: "AWS Bedrock uses standard AWS credential chain (env vars, ~/.aws, IAM roles)",
                sensitive: false,
                required: false,
                placeholder: None,
            }],
        }]
    }

    fn create(&self, id: &str, _config: ProviderConfig) -> anyhow::Result<Box<dyn ChatProvider>> {
        anyhow::ensure!(id == "aws_bedrock", "unknown provider: {id}");
        Ok(Box::new(AwsBedrockProvider::new()))
    }
}
