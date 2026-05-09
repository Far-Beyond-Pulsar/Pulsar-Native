use super::*;
use crate::custom_providers::CustomProvider;
use agent_chat_core::ProviderRegistry;
use std::collections::HashMap;
use std::sync::Arc;

impl AgentChatPanel {
    pub(super) fn default_provider_catalog() -> Vec<ProviderDefinition> {
        vec![
            ProviderDefinition {
                id: "demo_random",
                label: "Demo Random",
                kind: ProviderKind::Local,
                endpoint: "local://demo-random",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "demo-breeze",
                        label: "Demo Breeze",
                        supports_tools: false,
                    },
                    ModelDefinition {
                        id: "demo-story",
                        label: "Demo Story",
                        supports_tools: false,
                    },
                    ModelDefinition {
                        id: "demo-chaos",
                        label: "Demo Chaos",
                        supports_tools: false,
                    },
                ]),
            },
            ProviderDefinition {
                id: "openai",
                label: "OpenAI",
                kind: ProviderKind::Cloud,
                endpoint: "https://api.openai.com/v1",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "gpt-4.1",
                        label: "GPT-4.1",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "gpt-4.1-mini",
                        label: "GPT-4.1 Mini",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "gpt-4o",
                        label: "GPT-4o",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "o4-mini",
                        label: "o4 Mini",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "o3",
                        label: "o3",
                        supports_tools: true,
                    },
                ]),
            },
            ProviderDefinition {
                id: "azure_openai",
                label: "Azure OpenAI",
                kind: ProviderKind::Cloud,
                endpoint: "https://YOUR_RESOURCE_NAME.openai.azure.com/openai/deployments",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "gpt-4.1",
                        label: "GPT-4.1",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "gpt-4.1-mini",
                        label: "GPT-4.1 Mini",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "gpt-4o",
                        label: "GPT-4o",
                        supports_tools: true,
                    },
                ]),
            },
            ProviderDefinition {
                id: "anthropic",
                label: "Anthropic",
                kind: ProviderKind::Cloud,
                endpoint: "https://api.anthropic.com/v1",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "claude-3-7-sonnet-latest",
                        label: "Claude 3.7 Sonnet",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "claude-3-5-sonnet-latest",
                        label: "Claude 3.5 Sonnet",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "claude-3-5-haiku-latest",
                        label: "Claude 3.5 Haiku",
                        supports_tools: true,
                    },
                ]),
            },
            ProviderDefinition {
                id: "aws_bedrock",
                label: "AWS Bedrock",
                kind: ProviderKind::Cloud,
                endpoint: "https://bedrock-runtime.REGION.amazonaws.com/model",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "anthropic.claude-opus-4-1",
                        label: "Claude Opus 4.1 (Bedrock)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "anthropic.claude-3-7-sonnet",
                        label: "Claude 3.7 Sonnet (Bedrock)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "anthropic.claude-3-5-sonnet-v2",
                        label: "Claude 3.5 Sonnet v2 (Bedrock)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "amazon.nova-pro-v1",
                        label: "Amazon Nova Pro",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "meta.llama3-1-70b-instruct-v1",
                        label: "Llama 3.1 70B Instruct (Bedrock)",
                        supports_tools: false,
                    },
                ]),
            },
            ProviderDefinition {
                id: "vertex_ai",
                label: "Vertex AI",
                kind: ProviderKind::Cloud,
                endpoint: "https://LOCATION-aiplatform.googleapis.com/v1/projects/PROJECT/locations/LOCATION/publishers",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "anthropic/claude-opus-4-1",
                        label: "Claude Opus 4.1 (Vertex)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "anthropic/claude-3-7-sonnet",
                        label: "Claude 3.7 Sonnet (Vertex)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "google/gemini-2.5-pro",
                        label: "Gemini 2.5 Pro (Vertex)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "google/gemini-2.5-flash",
                        label: "Gemini 2.5 Flash (Vertex)",
                        supports_tools: true,
                    },
                ]),
            },
            ProviderDefinition {
                id: "github_copilot",
                label: "GitHub Models",
                kind: ProviderKind::Cloud,
                endpoint: "https://models.github.ai/inference/chat/completions",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "openai/gpt-4.1",
                        label: "GPT-4.1 (GitHub Models)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "openai/gpt-5-mini",
                        label: "GPT-5 mini (GitHub Models)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "anthropic/claude-sonnet-4-6",
                        label: "Claude Sonnet 4.6 (GitHub Models)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "google/gemini-2.5-pro",
                        label: "Gemini 2.5 Pro (GitHub Models)",
                        supports_tools: true,
                    },
                ]),
            },
            ProviderDefinition {
                id: "github_models",
                label: "GitHub Models",
                kind: ProviderKind::Cloud,
                endpoint: "https://models.inference.ai.azure.com",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "openai/gpt-4o",
                        label: "OpenAI GPT-4o (GitHub Models)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "anthropic/claude-3.7-sonnet",
                        label: "Claude 3.7 Sonnet (GitHub Models)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "meta/llama-3.3-70b-instruct",
                        label: "Llama 3.3 70B (GitHub Models)",
                        supports_tools: false,
                    },
                    ModelDefinition {
                        id: "mistral/mistral-large-latest",
                        label: "Mistral Large (GitHub Models)",
                        supports_tools: true,
                    },
                ]),
            },
            ProviderDefinition {
                id: "google",
                label: "Google Gemini",
                kind: ProviderKind::Cloud,
                endpoint: "https://generativelanguage.googleapis.com/v1beta",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "gemini-2.5-pro",
                        label: "Gemini 2.5 Pro",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "gemini-2.5-flash",
                        label: "Gemini 2.5 Flash",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "gemini-2.0-flash",
                        label: "Gemini 2.0 Flash",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "gemini-2.0-flash-lite",
                        label: "Gemini 2.0 Flash Lite",
                        supports_tools: false,
                    },
                ]),
            },
            ProviderDefinition {
                id: "mistral",
                label: "Mistral",
                kind: ProviderKind::Cloud,
                endpoint: "https://api.mistral.ai/v1",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "mistral-large-latest",
                        label: "Mistral Large",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "mistral-medium-latest",
                        label: "Mistral Medium",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "codestral-latest",
                        label: "Codestral",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "ministral-8b-latest",
                        label: "Ministral 8B",
                        supports_tools: false,
                    },
                ]),
            },
            ProviderDefinition {
                id: "cohere",
                label: "Cohere",
                kind: ProviderKind::Cloud,
                endpoint: "https://api.cohere.com/v1",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "command-a-03-2025",
                        label: "Command A",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "command-r-plus",
                        label: "Command R+",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "command-r",
                        label: "Command R",
                        supports_tools: true,
                    },
                ]),
            },
            ProviderDefinition {
                id: "groq",
                label: "Groq",
                kind: ProviderKind::Cloud,
                endpoint: "https://api.groq.com/openai/v1",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "llama-3.3-70b-versatile",
                        label: "Llama 3.3 70B Versatile",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "llama-3.1-8b-instant",
                        label: "Llama 3.1 8B Instant",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "qwen-qwq-32b",
                        label: "Qwen QwQ 32B",
                        supports_tools: false,
                    },
                    ModelDefinition {
                        id: "mixtral-8x7b-32768",
                        label: "Mixtral 8x7B",
                        supports_tools: false,
                    },
                ]),
            },
            ProviderDefinition {
                id: "xai",
                label: "xAI",
                kind: ProviderKind::Cloud,
                endpoint: "https://api.x.ai/v1",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "grok-3",
                        label: "Grok 3",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "grok-3-mini",
                        label: "Grok 3 Mini",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "grok-2-latest",
                        label: "Grok 2 Latest",
                        supports_tools: true,
                    },
                ]),
            },
            ProviderDefinition {
                id: "deepseek",
                label: "DeepSeek",
                kind: ProviderKind::Cloud,
                endpoint: "https://api.deepseek.com/v1",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "deepseek-chat",
                        label: "DeepSeek Chat",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "deepseek-reasoner",
                        label: "DeepSeek Reasoner",
                        supports_tools: true,
                    },
                ]),
            },
            ProviderDefinition {
                id: "together",
                label: "Together AI",
                kind: ProviderKind::Cloud,
                endpoint: "https://api.together.xyz/v1",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
                        label: "Llama 3.3 70B Instruct Turbo",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "Qwen/Qwen2.5-Coder-32B-Instruct",
                        label: "Qwen 2.5 Coder 32B",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "mistralai/Mixtral-8x7B-Instruct-v0.1",
                        label: "Mixtral 8x7B Instruct",
                        supports_tools: false,
                    },
                ]),
            },
            ProviderDefinition {
                id: "openrouter",
                label: "OpenRouter",
                kind: ProviderKind::Cloud,
                endpoint: "https://openrouter.ai/api/v1",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "openai/gpt-4o",
                        label: "OpenAI GPT-4o",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "anthropic/claude-3.7-sonnet",
                        label: "Claude 3.7 Sonnet",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "google/gemini-2.5-pro",
                        label: "Gemini 2.5 Pro",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "meta-llama/llama-3.3-70b-instruct",
                        label: "Llama 3.3 70B Instruct",
                        supports_tools: false,
                    },
                ]),
            },
            ProviderDefinition {
                id: "fireworks",
                label: "Fireworks",
                kind: ProviderKind::Cloud,
                endpoint: "https://api.fireworks.ai/inference/v1",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "accounts/fireworks/models/llama-v3p1-405b-instruct",
                        label: "Llama 3.1 405B Instruct",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "accounts/fireworks/models/qwen2p5-coder-32b-instruct",
                        label: "Qwen 2.5 Coder 32B",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "accounts/fireworks/models/mixtral-8x7b-instruct",
                        label: "Mixtral 8x7B Instruct",
                        supports_tools: false,
                    },
                ]),
            },
            ProviderDefinition {
                id: "perplexity",
                label: "Perplexity",
                kind: ProviderKind::Cloud,
                endpoint: "https://api.perplexity.ai",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "sonar-pro",
                        label: "Sonar Pro",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "sonar",
                        label: "Sonar",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "sonar-reasoning",
                        label: "Sonar Reasoning",
                        supports_tools: true,
                    },
                ]),
            },
            ProviderDefinition {
                id: "ollama",
                label: "Ollama",
                kind: ProviderKind::Local,
                endpoint: "http://localhost:11434",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "llama3.1:8b",
                        label: "Llama 3.1 8B",
                        supports_tools: false,
                    },
                    ModelDefinition {
                        id: "qwen2.5-coder:7b",
                        label: "Qwen 2.5 Coder 7B",
                        supports_tools: false,
                    },
                    ModelDefinition {
                        id: "llama3.1:70b",
                        label: "Llama 3.1 70B",
                        supports_tools: false,
                    },
                    ModelDefinition {
                        id: "mistral-nemo:12b",
                        label: "Mistral Nemo 12B",
                        supports_tools: false,
                    },
                    ModelDefinition {
                        id: "deepseek-coder-v2:16b",
                        label: "DeepSeek Coder V2 16B",
                        supports_tools: false,
                    },
                ]),
            },
            ProviderDefinition {
                id: "docker_model_runner",
                label: "Docker AI",
                kind: ProviderKind::Local,
                endpoint: "http://localhost:12434/engines/v1",
                models: Arc::new(vec![ModelDefinition {
                    id: "ai/gemma4:4B",
                    label: "Gemma 4 4B (Docker)",
                    supports_tools: true,
                }]),
            },
            ProviderDefinition {
                id: "lmstudio",
                label: "LM Studio",
                kind: ProviderKind::Local,
                endpoint: "http://localhost:1234/v1",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "local-default",
                        label: "Local Default",
                        supports_tools: false,
                    },
                    ModelDefinition {
                        id: "qwen2.5-coder-14b",
                        label: "Qwen 2.5 Coder 14B",
                        supports_tools: false,
                    },
                    ModelDefinition {
                        id: "llama-3.1-8b-instruct",
                        label: "Llama 3.1 8B Instruct",
                        supports_tools: false,
                    },
                ]),
            },
            ProviderDefinition {
                id: "vllm",
                label: "vLLM",
                kind: ProviderKind::Local,
                endpoint: "http://localhost:8000/v1",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "meta-llama/Llama-3.1-70B-Instruct",
                        label: "Llama 3.1 70B Instruct",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "Qwen/Qwen2.5-Coder-32B-Instruct",
                        label: "Qwen 2.5 Coder 32B",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "mistralai/Mistral-Nemo-Instruct-2407",
                        label: "Mistral Nemo Instruct",
                        supports_tools: false,
                    },
                ]),
            },
            ProviderDefinition {
                id: "llama_cpp",
                label: "llama.cpp Server",
                kind: ProviderKind::Local,
                endpoint: "http://localhost:8080/v1",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "qwen2.5-coder-7b-instruct-q4_k_m",
                        label: "Qwen 2.5 Coder 7B (Q4_K_M)",
                        supports_tools: false,
                    },
                    ModelDefinition {
                        id: "llama-3.1-8b-instruct-q4_k_m",
                        label: "Llama 3.1 8B (Q4_K_M)",
                        supports_tools: false,
                    },
                    ModelDefinition {
                        id: "phi-4-mini-instruct-q4_k_m",
                        label: "Phi 4 Mini Instruct (Q4_K_M)",
                        supports_tools: false,
                    },
                ]),
            },
            ProviderDefinition {
                id: "openrouter",
                label: "OpenRouter",
                kind: ProviderKind::Cloud,
                endpoint: "https://openrouter.ai/api/v1/chat/completions",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "openai/gpt-4o",
                        label: "GPT-4o (OpenRouter)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "anthropic/claude-3.7-sonnet",
                        label: "Claude 3.7 Sonnet (OpenRouter)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "google/gemini-2.5-pro",
                        label: "Gemini 2.5 Pro (OpenRouter)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "meta-llama/llama-3.3-70b-instruct",
                        label: "Llama 3.3 70B (OpenRouter)",
                        supports_tools: false,
                    },
                ]),
            },
            ProviderDefinition {
                id: "groq",
                label: "Groq",
                kind: ProviderKind::Cloud,
                endpoint: "https://api.groq.com/openai/v1/chat/completions",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "llama-3.3-70b-versatile",
                        label: "Llama 3.3 70B Versatile",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "llama-3.1-8b-instant",
                        label: "Llama 3.1 8B Instant",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "mixtral-8x7b-32768",
                        label: "Mixtral 8x7B",
                        supports_tools: false,
                    },
                ]),
            },
            ProviderDefinition {
                id: "together",
                label: "Together AI",
                kind: ProviderKind::Cloud,
                endpoint: "https://api.together.xyz/v1/chat/completions",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
                        label: "Llama 3.3 70B Instruct Turbo",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "Qwen/Qwen2.5-Coder-32B-Instruct",
                        label: "Qwen 2.5 Coder 32B",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "mistralai/Mixtral-8x7B-Instruct-v0.1",
                        label: "Mixtral 8x7B Instruct",
                        supports_tools: false,
                    },
                ]),
            },
            ProviderDefinition {
                id: "gemini",
                label: "Google Gemini (Direct)",
                kind: ProviderKind::Cloud,
                endpoint: "https://generativelanguage.googleapis.com/v1beta/openai/",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "gemini-2.5-pro",
                        label: "Gemini 2.5 Pro",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "gemini-2.5-flash",
                        label: "Gemini 2.5 Flash",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "gemini-2.0-flash",
                        label: "Gemini 2.0 Flash",
                        supports_tools: true,
                    },
                ]),
            },
            ProviderDefinition {
                id: "jan",
                label: "Jan",
                kind: ProviderKind::Local,
                endpoint: "http://localhost:1337/v1",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "jan-local-default",
                        label: "Jan Local Default",
                        supports_tools: false,
                    },
                    ModelDefinition {
                        id: "jan-qwen2.5-coder-7b",
                        label: "Jan Qwen 2.5 Coder 7B",
                        supports_tools: false,
                    },
                ]),
            },
        ]
    }

    pub(super) fn static_str(value: String) -> &'static str {
        Box::leak(value.into_boxed_str())
    }

    pub(super) fn custom_provider_to_definition(provider: &CustomProvider) -> ProviderDefinition {
        let models = provider
            .models
            .iter()
            .map(|model| ModelDefinition {
                id: Self::static_str(model.id.clone()),
                label: Self::static_str(model.label.clone()),
                supports_tools: model.supports_tools,
            })
            .collect::<Vec<_>>();

        ProviderDefinition {
            id: Self::static_str(provider.id.clone()),
            label: Self::static_str(provider.label.clone()),
            kind: ProviderKind::Local,
            endpoint: Self::static_str(provider.endpoint.clone()),
            models: Arc::new(models),
        }
    }

    pub(super) fn wip_providers_from_catalog(
        provider_catalog: &[ProviderDefinition],
        provider_registry: &ProviderRegistry,
    ) -> HashMap<&'static str, String> {
        provider_catalog
            .iter()
            .filter(|provider| provider_registry.get(provider.id).is_none())
            .map(|provider| (provider.id, "WIP provider".to_string()))
            .collect()
    }
}
