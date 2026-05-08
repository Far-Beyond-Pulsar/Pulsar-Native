use agent_chat_core::{
    AuthHost, AuthMethod, AuthResult, AvailabilityState, ChatMessage as ProviderChatMessage,
    ChatRequest, ChatRole, ProcessEnvironment, PromptTokenRequest, ProviderEnvironment,
    ToolDefinition,
    ProviderRegistry,
};
use agent_chat_tools::{ToolContext, ToolRegistry};
use agent_provider_demo_random::DemoRandomProvider;
use agent_provider_github_copilot::GithubCopilotProvider;
use engine_state;
use gpui::{prelude::FluentBuilder as _, *};
use serde::{Deserialize, Serialize};
use smol::Timer;
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use ui::{
    button::{Button, ButtonVariants as _},
    dock::{Panel, PanelEvent},
    dropdown::{SearchableList, SearchableListEvent, SearchableListItemState},
    input::Enter,
    popover::Popover,
    scroll::{Scrollbar, ScrollbarState},
    Disableable,
    h_flex,
    input::{InputState, TextInput},
    v_flex, v_virtual_list, ActiveTheme as _, IconName, Sizable, StyledExt,
    VirtualListScrollHandle,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderKind {
    Cloud,
    Local,
}

#[derive(Clone, Debug)]
struct ModelDefinition {
    id: &'static str,
    label: &'static str,
    supports_tools: bool,
}

#[derive(Clone, Debug)]
struct ProviderDefinition {
    id: &'static str,
    label: &'static str,
    kind: ProviderKind,
    endpoint: &'static str,
    models: Arc<Vec<ModelDefinition>>,
}

#[derive(Clone, Debug)]
struct ChatMessage {
    role: &'static str,
    content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedChatMessage {
    role: String,
    content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ChatSessionFile {
    id: String,
    title: String,
    created_at: u64,
    updated_at: u64,
    messages: Vec<PersistedChatMessage>,
}

#[derive(Clone, Debug)]
struct ChatHistoryEntry {
    id: String,
    title: String,
    updated_at: u64,
}

pub struct AgentChatPanel {
    focus_handle: FocusHandle,
    messages_scroll_handle: VirtualListScrollHandle,
    messages_scroll_state: ScrollbarState,
    prompt_input: Entity<InputState>,
    auth_token_input: Entity<InputState>,
    chat_history_list: Entity<SearchableList<ChatHistoryEntry>>,
    provider_list: Entity<SearchableList<ProviderDefinition>>,
    model_list: Entity<SearchableList<ModelDefinition>>,
    provider_catalog: Vec<ProviderDefinition>,
    wip_providers: HashMap<&'static str, String>,
    provider_registry: ProviderRegistry,
    tool_registry: ToolRegistry,
    provider_tokens: HashMap<&'static str, String>,
    pending_auth_provider: Option<&'static str>,
    pending_device_code: Option<String>,
    current_chat_id: String,
    current_chat_created_at: u64,
    loaded_chat_project_root: Option<PathBuf>,
    message_row_heights: HashMap<usize, Pixels>,
    active_provider_ix: usize,
    active_model_ix: usize,
    messages: Vec<ChatMessage>,
    _subscriptions: Vec<Subscription>,
}

impl AgentChatPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let provider_catalog = Self::default_provider_catalog();
        let mut provider_registry = ProviderRegistry::new();
        provider_registry.register(Arc::new(GithubCopilotProvider::new()));
        provider_registry.register(Arc::new(DemoRandomProvider::new()));
        let wip_providers = Self::wip_providers_from_catalog(&provider_catalog, &provider_registry);

        let prompt_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Ask the engine assistant..."));
        let auth_token_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Paste provider token..."));

        let chat_history_list = cx.new(|cx| {
            SearchableList::new(window, cx, Vec::<ChatHistoryEntry>::new(), |chat| {
                format!("{} ({})", chat.title, chat.id)
            })
            .with_empty_text("No chats found")
            .with_max_width(px(340.0))
            .with_max_height(px(200.0))
        });

        let wip_for_list = wip_providers.clone();
        let provider_list = cx.new(|cx| {
            SearchableList::new(window, cx, provider_catalog.clone(), |p: &ProviderDefinition| {
                format!("{} ({})", p.label, p.id)
            })
            .with_empty_text("No providers found")
            .with_max_width(px(220.0))
            .with_max_height(px(320.0))
            .with_icon_getter(|_| IconName::Brain)
            .with_item_state(move |provider| {
                if wip_for_list.contains_key(provider.id) {
                    SearchableListItemState::Disabled
                } else {
                    SearchableListItemState::Enabled
                }
            })
        });

        let initial_models = provider_catalog
            .first()
            .map(|provider| provider.models.as_ref().clone())
            .unwrap_or_default();
        let model_list = cx.new(|cx| {
            SearchableList::new(window, cx, initial_models.clone(), |m: &ModelDefinition| {
                format!("{} ({})", m.label, m.id)
            })
            .with_empty_text("No models found")
            .with_max_width(px(220.0))
            .with_max_height(px(360.0))
            .with_icon_getter(|_| IconName::Cpu)
        });

        let subscriptions = vec![
            cx.subscribe(
                &provider_list,
                move |this, _, event: &SearchableListEvent<ProviderDefinition>, cx| {
                    let SearchableListEvent::Select(selected_provider) = event;
                    if let Some(index) = this
                        .provider_catalog
                        .iter()
                        .position(|provider| provider.id == selected_provider.id)
                    {
                        this.set_provider(index, cx);
                    }
                },
            ),
            cx.subscribe(
                &model_list,
                move |this, _, event: &SearchableListEvent<ModelDefinition>, cx| {
                    let SearchableListEvent::Select(selected_model) = event;
                    if let Some(provider) = this.active_provider() {
                        if let Some(index) = provider
                            .models
                            .iter()
                            .position(|model| model.id == selected_model.id)
                        {
                            this.set_model(index, cx);
                        }
                    }
                },
            ),
            cx.subscribe(
                &chat_history_list,
                move |this, _, event: &SearchableListEvent<ChatHistoryEntry>, cx| {
                    let SearchableListEvent::Select(entry) = event;
                    this.load_chat_session(&entry.id, cx);
                },
            ),
        ];

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            messages_scroll_handle: VirtualListScrollHandle::new(),
            messages_scroll_state: ScrollbarState::default(),
            prompt_input,
            auth_token_input,
            chat_history_list,
            provider_list,
            model_list,
            provider_catalog,
            wip_providers,
            provider_registry,
            tool_registry: ToolRegistry::with_default_tools(),
            provider_tokens: HashMap::new(),
            pending_auth_provider: None,
            pending_device_code: None,
            current_chat_id: String::new(),
            current_chat_created_at: 0,
            loaded_chat_project_root: None,
            message_row_heights: HashMap::new(),
            active_provider_ix: 0,
            active_model_ix: 0,
            messages: vec![ChatMessage {
                role: "system",
                content: "Agent Chat is ready. Choose provider/model and ask anything about your project.".to_string(),
            }],
            _subscriptions: subscriptions,
        };

        this.bootstrap_chat_storage(cx);
        this
    }

    fn wip_providers_from_catalog(
        provider_catalog: &[ProviderDefinition],
        provider_registry: &ProviderRegistry,
    ) -> HashMap<&'static str, String> {
        provider_catalog
            .iter()
            .filter(|provider| provider_registry.get(provider.id).is_none())
            .map(|provider| (provider.id, "WIP provider".to_string()))
            .collect()
    }

    fn default_provider_catalog() -> Vec<ProviderDefinition> {
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
                        id: "gpt-5.3-codex",
                        label: "GPT-5.3 Codex",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "gpt-5.3-mini",
                        label: "GPT-5.3 Mini",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "gpt-5.3",
                        label: "GPT-5.3",
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
                        id: "claude-opus-4-1",
                        label: "Claude Opus 4.1",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "claude-4-opus",
                        label: "Claude 4 Opus",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "claude-4-sonnet",
                        label: "Claude 4 Sonnet",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "claude-3.5-sonnet",
                        label: "Claude 3.5 Sonnet",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "claude-3.7-sonnet",
                        label: "Claude 3.7 Sonnet",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "claude-3.5-haiku",
                        label: "Claude 3.5 Haiku",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "claude-3-opus",
                        label: "Claude 3 Opus",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "claude-3-sonnet",
                        label: "Claude 3 Sonnet",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "claude-3-haiku",
                        label: "Claude 3 Haiku",
                        supports_tools: false,
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

    fn active_provider(&self) -> Option<&ProviderDefinition> {
        self.provider_catalog.get(self.active_provider_ix)
    }

    fn now_epoch_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn now_epoch_nanos() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }

    fn chats_dir() -> Option<PathBuf> {
        let project_root = engine_state::get_project_path().map(PathBuf::from)?;
        Some(project_root.join(".pulsar").join("chats"))
    }

    fn ensure_chats_dir() -> Option<PathBuf> {
        let dir = Self::chats_dir()?;
        if fs::create_dir_all(&dir).is_ok() {
            Some(dir)
        } else {
            None
        }
    }

    fn chat_file_path(chat_id: &str) -> Option<PathBuf> {
        Some(Self::ensure_chats_dir()?.join(format!("{chat_id}.json")))
    }

    fn normalize_role(role: &str) -> &'static str {
        match role {
            "user" => "user",
            "assistant" => "assistant",
            "system" => "system",
            _ => "assistant",
        }
    }

    fn default_system_message() -> ChatMessage {
        ChatMessage {
            role: "system",
            content: "Agent Chat is ready. Choose provider/model and ask anything about your project.".to_string(),
        }
    }

    fn inferred_chat_title(messages: &[ChatMessage]) -> String {
        if let Some(user_message) = messages.iter().find(|m| m.role == "user") {
            user_message
                .content
                .chars()
                .take(60)
                .collect::<String>()
                .trim()
                .to_string()
        } else {
            "New Chat".to_string()
        }
    }

    fn save_current_chat(&self) {
        if self.current_chat_id.is_empty() {
            return;
        }

        let Some(path) = Self::chat_file_path(&self.current_chat_id) else {
            return;
        };

        let payload = ChatSessionFile {
            id: self.current_chat_id.clone(),
            title: Self::inferred_chat_title(&self.messages),
            created_at: self.current_chat_created_at,
            updated_at: Self::now_epoch_secs(),
            messages: self
                .messages
                .iter()
                .map(|m| PersistedChatMessage {
                    role: m.role.to_string(),
                    content: m.content.clone(),
                })
                .collect(),
        };

        if let Ok(serialized) = serde_json::to_string_pretty(&payload) {
            let _ = fs::write(path, serialized);
        }
    }

    fn read_chat_index() -> Vec<ChatHistoryEntry> {
        let Some(dir) = Self::ensure_chats_dir() else {
            return Vec::new();
        };

        let mut entries = Vec::new();
        let Ok(files) = fs::read_dir(dir) else {
            return entries;
        };

        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }

            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(chat) = serde_json::from_str::<ChatSessionFile>(&raw) else {
                continue;
            };

            entries.push(ChatHistoryEntry {
                id: chat.id,
                title: chat.title,
                updated_at: chat.updated_at,
            });
        }

        entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        entries
    }

    fn refresh_chat_history_list(&mut self, cx: &mut Context<Self>) {
        let entries = Self::read_chat_index();
        self.chat_history_list.update(cx, |list, cx| {
            list.set_items(entries, cx);
        });
    }

    fn load_chat_session(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        let Some(path) = Self::chat_file_path(chat_id) else {
            return;
        };
        let Ok(raw) = fs::read_to_string(path) else {
            return;
        };
        let Ok(chat) = serde_json::from_str::<ChatSessionFile>(&raw) else {
            return;
        };

        self.current_chat_id = chat.id;
        self.current_chat_created_at = chat.created_at;
        self.message_row_heights.clear();
        self.messages = chat
            .messages
            .into_iter()
            .map(|m| ChatMessage {
                role: Self::normalize_role(&m.role),
                content: m.content,
            })
            .collect();

        if self.messages.is_empty() {
            self.messages.push(Self::default_system_message());
        }

        self.scroll_messages_to_bottom();
        cx.notify();
    }

    fn start_new_chat(&mut self, cx: &mut Context<Self>) {
        self.current_chat_id = format!("chat-{}", Self::now_epoch_nanos());
        self.current_chat_created_at = Self::now_epoch_secs();
        self.message_row_heights.clear();
        self.messages = vec![Self::default_system_message()];
        self.save_current_chat();
        self.refresh_chat_history_list(cx);
        self.scroll_messages_to_bottom();
        cx.notify();
    }

    fn bootstrap_chat_storage(&mut self, cx: &mut Context<Self>) {
        let entries = Self::read_chat_index();
        self.chat_history_list.update(cx, |list, cx| {
            list.set_items(entries.clone(), cx);
        });

        if let Some(latest) = entries.first() {
            self.load_chat_session(&latest.id, cx);
        } else {
            self.start_new_chat(cx);
        }

        self.loaded_chat_project_root = engine_state::get_project_path().map(PathBuf::from);
    }

    fn maybe_reload_chats_from_disk(&mut self, cx: &mut Context<Self>) {
        let current_root = engine_state::get_project_path().map(PathBuf::from);
        if current_root.is_none() {
            return;
        }

        if self.loaded_chat_project_root != current_root {
            self.bootstrap_chat_storage(cx);
        }
    }

    fn active_model(&self) -> Option<&ModelDefinition> {
        self.active_provider()
            .and_then(|provider| provider.models.get(self.active_model_ix))
    }

    fn auth_token_for_provider(&self, provider_id: &str) -> Option<String> {
        self.provider_tokens
            .get(provider_id)
            .cloned()
            .or_else(|| ProcessEnvironment.get_env("GITHUB_COPILOT_TOKEN"))
            .or_else(|| ProcessEnvironment.get_env("COPILOT_TOKEN"))
    }

    fn maybe_require_auth_for_active_provider(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.active_provider() else {
            self.pending_auth_provider = None;
            return;
        };

        if self.wip_providers.contains_key(provider.id) {
            self.pending_auth_provider = None;
            return;
        }

        if self.auth_token_for_provider(provider.id).is_some() {
            self.pending_auth_provider = None;
            return;
        }

        if let Some(provider_impl) = self.provider_registry.get(provider.id) {
            let availability = provider_impl.availability(&ProcessEnvironment);
            if matches!(availability.state, AvailabilityState::RequiresAuth) {
                self.pending_auth_provider = Some(provider.id);
                cx.notify();
                return;
            }
        }

        self.pending_auth_provider = None;
    }

    fn set_provider(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.provider_catalog.len() {
            if self
                .provider_catalog
                .get(index)
                .is_some_and(|provider| self.wip_providers.contains_key(provider.id))
            {
                return;
            }

            self.active_provider_ix = index;
            self.active_model_ix = 0;

            let models = self
                .active_provider()
                .map(|provider| provider.models.as_ref().clone())
                .unwrap_or_default();
            self.model_list.update(cx, |list, cx| {
                list.set_items(models, cx);
            });

            self.maybe_require_auth_for_active_provider(cx);

            cx.notify();
        }
    }

    fn set_model(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(provider) = self.active_provider() {
            if index < provider.models.len() {
                self.active_model_ix = index;
                cx.notify();
            }
        }
    }

    fn complete_prompt_auth(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(provider_id) = self.pending_auth_provider else {
            return;
        };

        let token = self.auth_token_input.read(cx).text().to_string();
        let token = token.trim().to_string();
        if token.is_empty() {
            return;
        }

        let Some(provider) = self.provider_registry.get(provider_id).cloned() else {
            return;
        };

        struct PromptOnlyAuthHost {
            token: String,
        }

        impl AuthHost for PromptOnlyAuthHost {
            fn prompt_for_token(
                &mut self,
                _request: PromptTokenRequest,
            ) -> anyhow::Result<Option<String>> {
                Ok(Some(self.token.clone()))
            }

            fn open_browser_for_token(
                &mut self,
                _request: agent_chat_core::OpenBrowserRequest,
            ) -> anyhow::Result<Option<String>> {
                Ok(None)
            }
        }

        let mut host = PromptOnlyAuthHost { token };
        match provider.authenticate(AuthMethod::PromptToken, &mut host) {
            Ok(AuthResult::Authenticated { token }) => {
                self.provider_tokens.insert(provider_id, token);
                self.pending_auth_provider = None;
                self.auth_token_input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                });
                self.messages.push(ChatMessage {
                    role: "system",
                    content: format!("{} authenticated successfully.", provider_id),
                });
                self.save_current_chat();
                self.refresh_chat_history_list(cx);
                self.scroll_messages_to_bottom();
                cx.notify();
            }
            Ok(AuthResult::Cancelled) => {}
            Err(err) => {
                self.messages.push(ChatMessage {
                    role: "system",
                    content: format!("Authentication failed: {err}"),
                });
                self.save_current_chat();
                self.refresh_chat_history_list(cx);
                self.scroll_messages_to_bottom();
                cx.notify();
            }
        }
    }

    fn begin_browser_auth(&mut self, cx: &mut Context<Self>) {
        let Some(provider_id) = self.pending_auth_provider else {
            return;
        };
        let Some(provider) = self.provider_registry.get(provider_id).cloned() else {
            return;
        };

        // If the provider supports the OAuth device-code flow, use it instead of
        // asking the user to paste a token (PATs are rejected by the Copilot API).
        if let Some(flow_result) = provider.start_device_flow() {
            match flow_result {
                Ok(info) => {
                    self.messages.push(ChatMessage {
                        role: "system",
                        content: format!(
                            "Open {} in your browser and enter code: **{}**",
                            info.verification_uri, info.user_code
                        ),
                    });
                    self.pending_device_code = Some(info.device_code.clone());
                    self.scroll_messages_to_bottom();
                    cx.notify();
                    cx.open_url(&info.verification_uri);

                    let device_code = info.device_code;
                    let interval = info.interval.max(5);

                    cx.spawn(async move |this, cx| {
                        loop {
                            Timer::after(Duration::from_secs(interval)).await;

                            // Perform a single blocking poll on whatever thread GPUI picks.
                            let poll = cx.update(|cx| {
                                this.update(cx, |panel, _cx| {
                                    panel
                                        .provider_registry
                                        .get(provider_id)
                                        .cloned()
                                        .map(|p| p.poll_device_code(&device_code))
                                })
                                .ok()
                                .flatten()
                            });

                            match poll {
                                Ok(Some(Ok(Some(token)))) => {
                                    cx.update(|cx| {
                                        this.update(cx, |panel, cx| {
                                            panel.provider_tokens.insert(provider_id, token);
                                            panel.pending_device_code = None;
                                            panel.pending_auth_provider = None;
                                            panel.messages.push(ChatMessage {
                                                role: "system",
                                                content: format!(
                                                    "{} authenticated successfully.",
                                                    provider_id
                                                ),
                                            });
                                            panel.save_current_chat();
                                            panel.refresh_chat_history_list(cx);
                                            panel.scroll_messages_to_bottom();
                                            cx.notify();
                                        })
                                        .ok();
                                    })
                                    .ok();
                                    break;
                                }
                                // authorization_pending or slow_down — keep polling
                                Ok(Some(Ok(None))) => {}
                                // error or the entity/context was dropped
                                _ => {
                                    cx.update(|cx| {
                                        this.update(cx, |panel, cx| {
                                            panel.pending_device_code = None;
                                            panel.messages.push(ChatMessage {
                                                role: "system",
                                                content: "Device code authentication failed or timed out.".to_string(),
                                            });
                                            panel.scroll_messages_to_bottom();
                                            cx.notify();
                                        })
                                        .ok();
                                    })
                                    .ok();
                                    break;
                                }
                            }
                        }
                    })
                    .detach();
                }
                Err(err) => {
                    self.messages.push(ChatMessage {
                        role: "system",
                        content: format!("Failed to start device flow: {err}"),
                    });
                    self.scroll_messages_to_bottom();
                    cx.notify();
                }
            }
            return;
        }

        // Fallback: providers that only support opening a URL (no device-code polling).
        struct BrowserOnlyAuthHost {
            browser_url: Option<String>,
        }

        impl AuthHost for BrowserOnlyAuthHost {
            fn prompt_for_token(
                &mut self,
                _request: PromptTokenRequest,
            ) -> anyhow::Result<Option<String>> {
                Ok(None)
            }

            fn open_browser_for_token(
                &mut self,
                request: agent_chat_core::OpenBrowserRequest,
            ) -> anyhow::Result<Option<String>> {
                self.browser_url = Some(request.url);
                Ok(None)
            }
        }

        let mut host = BrowserOnlyAuthHost { browser_url: None };
        if provider
            .authenticate(AuthMethod::BrowserDeviceCode, &mut host)
            .is_ok()
        {
            if let Some(url) = host.browser_url {
                cx.open_url(&url);
            }
        }
    }

    fn stream_assistant_chunks(
        &mut self,
        chunks: Vec<String>,
        fallback_message: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let chunks = if chunks.is_empty() {
            fallback_message
                .map(|text| vec![text])
                .unwrap_or_else(|| vec!["Provider returned an empty response.".to_string()])
        } else {
            chunks
        };

        let message_ix = self.messages.len();
        self.messages.push(ChatMessage {
            role: "assistant",
            content: String::new(),
        });
        self.scroll_messages_to_bottom();
        cx.notify();

        cx.spawn(async move |this, cx| {
            for chunk in chunks {
                cx.update(|cx| {
                    this.update(cx, |panel, cx| {
                        if let Some(message) = panel.messages.get_mut(message_ix) {
                            message.content.push_str(&chunk);
                        }
                        panel.message_row_heights.remove(&message_ix);
                        panel.save_current_chat();
                        panel.scroll_messages_to_bottom();
                        cx.notify();
                    })
                    .ok();
                })
                .ok();

                Timer::after(Duration::from_millis(14)).await;
            }
        })
        .detach();
    }

    fn on_prompt_enter(&mut self, enter: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if enter.secondary {
            return;
        }

        if self.prompt_input.read(cx).focus_handle(cx).is_focused(window) {
            self.send_prompt(window, cx);
        }
    }

    fn scroll_messages_to_bottom(&self) {
        self.messages_scroll_handle.scroll_to_bottom();
    }

    fn message_row_height(message: &ChatMessage) -> Pixels {
        let explicit_lines = message.content.lines().collect::<Vec<_>>();
        let visual_lines: usize = explicit_lines
            .iter()
            .map(|line| {
                // Use a conservative wrap estimate so rows never under-size and overlap.
                let chars = line.chars().count().max(1);
                chars.div_ceil(64)
            })
            .sum::<usize>()
            .max(1);

        // Header + paddings + line-height budget + row gap.
        let estimated = 10.0 + 14.0 + 14.0 + (visual_lines as f32 * 18.0) + 6.0;
        px(estimated.min(520.0))
    }

    fn send_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = self.prompt_input.read(cx).text().to_string();
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return;
        }

        self.messages.push(ChatMessage {
            role: "user",
            content: prompt.clone(),
        });
        self.scroll_messages_to_bottom();

        let provider_id = self
            .active_provider()
            .map(|p| p.id)
            .unwrap_or("unknown_provider");

        if self.wip_providers.contains_key(provider_id) {
            self.messages.push(ChatMessage {
                role: "assistant",
                content: "Selected provider is still WIP and not yet executable.".to_string(),
            });
        } else if let Some(provider) = self.provider_registry.get(provider_id) {
            let token = self.auth_token_for_provider(provider_id);
            let model = self
                .active_model()
                .map(|m| m.id.to_string())
                .unwrap_or_else(|| "default".to_string());
            let availability = provider.availability(&ProcessEnvironment);

            if matches!(availability.state, AvailabilityState::RequiresAuth) && token.is_none() {
                self.pending_auth_provider = Some(provider_id);
                self.messages.push(ChatMessage {
                    role: "assistant",
                    content: "Authentication required. Paste token in the auth row above.".to_string(),
                });
            } else {
                let token = token.unwrap_or_default();
                let request = ChatRequest {
                    model,
                    messages: vec![ProviderChatMessage {
                        role: ChatRole::User,
                        content: prompt.clone(),
                    }],
                    enable_tool_calls: true,
                    tools: self
                        .tool_registry
                        .available_tools_schema()
                        .into_iter()
                        .filter_map(|schema| {
                            Some(ToolDefinition {
                                name: schema.get("name")?.as_str()?.to_string(),
                                description: schema
                                    .get("description")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string()),
                                parameters_json_schema: schema.get("parameters")?.clone(),
                            })
                        })
                        .collect(),
                    temperature: Some(0.2),
                    top_p: Some(1.0),
                    max_tokens: Some(1024),
                };

                match provider.chat_completion(&token, &request) {
                    Ok(response) => {
                        if !response.tool_calls.is_empty() {
                            let workspace_root = engine_state::get_project_path()
                                .map(PathBuf::from)
                                .or_else(|| std::env::current_dir().ok())
                                .unwrap_or_else(|| std::path::PathBuf::from("."));
                            let tool_ctx = ToolContext { workspace_root };

                            let mut followup_messages = request.messages.clone();
                            for tool_call in response.tool_calls {
                                let result = self.tool_registry.execute(
                                    &tool_call.name,
                                    tool_call.arguments_json,
                                    &tool_ctx,
                                );

                                let rendered = match result {
                                    Ok(value) => value,
                                    Err(err) => serde_json::json!({
                                        "error": err.to_string(),
                                    }),
                                };

                                followup_messages.push(ProviderChatMessage {
                                    role: ChatRole::Tool,
                                    content: rendered.to_string(),
                                });
                            }

                            let followup_request = ChatRequest {
                                model: request.model.clone(),
                                messages: followup_messages,
                                enable_tool_calls: false,
                                tools: Vec::new(),
                                temperature: request.temperature,
                                top_p: request.top_p,
                                max_tokens: request.max_tokens,
                            };

                            match provider.chat_completion(&token, &followup_request) {
                                Ok(final_response) => {
                                    self.stream_assistant_chunks(
                                        final_response.streamed_text_chunks,
                                        final_response.assistant_message,
                                        cx,
                                    );
                                }
                                Err(err) => {
                                    self.messages.push(ChatMessage {
                                        role: "assistant",
                                        content: format!(
                                            "Provider follow-up after tool calls failed: {err}"
                                        ),
                                    });
                                }
                            }
                        } else if response.assistant_message.is_some()
                            || !response.streamed_text_chunks.is_empty()
                        {
                            self.stream_assistant_chunks(
                                response.streamed_text_chunks,
                                response.assistant_message,
                                cx,
                            );
                        } else {
                            self.messages.push(ChatMessage {
                                role: "assistant",
                                content: "Provider returned an empty response.".to_string(),
                            });
                        }
                    }
                    Err(err) => {
                        self.messages.push(ChatMessage {
                            role: "assistant",
                            content: format!("Provider request failed: {err}"),
                        });
                    }
                }
            }
        } else {
            let provider = self
                .active_provider()
                .map(|p| p.label)
                .unwrap_or("Unknown Provider");
            let model = self.active_model().map(|m| m.label).unwrap_or("Unknown Model");
            self.messages.push(ChatMessage {
                role: "assistant",
                content: format!("Queued with {provider} / {model}."),
            });
        }

        self.prompt_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        self.save_current_chat();
        self.refresh_chat_history_list(cx);
        self.scroll_messages_to_bottom();
        cx.notify();
    }
}

impl EventEmitter<PanelEvent> for AgentChatPanel {}

impl Focusable for AgentChatPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for AgentChatPanel {
    fn panel_name(&self) -> &'static str {
        "agent_chat"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Agent Chat".into_any_element()
    }

    fn closable(&self, _cx: &App) -> bool {
        false
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState::new(self)
    }
}

impl Render for AgentChatPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.maybe_reload_chats_from_disk(cx);

        let provider = self.active_provider();
        let model = self.active_model();
        let auth_provider = self.pending_auth_provider;
        let provider_list = self.provider_list.clone();
        let model_list = self.model_list.clone();
        let chat_history_list = self.chat_history_list.clone();
        let current_chat_id = self.current_chat_id.clone();
        let message_item_sizes = std::rc::Rc::new(
            self.messages
                .iter()
                .enumerate()
                .map(|(ix, message)| {
                    let h = self
                        .message_row_heights
                        .get(&ix)
                        .copied()
                        .unwrap_or_else(|| Self::message_row_height(message));
                    size(px(0.0), h)
                })
                .collect::<Vec<_>>(),
        );

        let provider_popover = Popover::<SearchableList<ProviderDefinition>>::new(
            "agent-chat-provider-popover",
        )
        .anchor(Corner::TopLeft)
        .trigger(
            Button::new("agent-chat-provider-trigger")
                .xsmall()
                .ghost()
            .justify_start()
                .label(
                    provider
                .map(|p| format!("Provider: {} ({})", p.label, p.id))
                        .unwrap_or_else(|| "No provider".to_string()),
                )
            .dropdown_caret(true),
        )
        .content(move |_window, _cx| provider_list.clone());

        let model_popover = Popover::<SearchableList<ModelDefinition>>::new(
            "agent-chat-model-popover",
        )
        .anchor(Corner::TopLeft)
        .trigger(
            Button::new("agent-chat-model-trigger")
                .xsmall()
                .ghost()
            .justify_start()
                .label(
                    model
                .map(|m| format!("Model: {} ({})", m.label, m.id))
                        .unwrap_or_else(|| "No model".to_string()),
                )
            .dropdown_caret(true),
        )
        .content(move |_window, _cx| model_list.clone());

        let chat_history_popover = Popover::<SearchableList<ChatHistoryEntry>>::new(
            "agent-chat-history-popover",
        )
        .anchor(Corner::BottomLeft)
        .trigger(
            Button::new("agent-chat-history-trigger")
                .xsmall()
                .ghost()
                .justify_start()
                .label(format!("Chat: {}", current_chat_id))
                .dropdown_caret(true),
        )
        .content(move |_window, _cx| chat_history_list.clone());

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .on_action(cx.listener(Self::on_prompt_enter))
            .child(
                v_flex()
                    .w_full()
                    .gap_1()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().tab_bar)
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .child(provider_popover),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .child(model_popover),
                    )
                    .when(auth_provider.is_some(), |el| {
                        el.child(
                            v_flex()
                                .w_full()
                                .gap_1()
                                .p_2()
                                .rounded(px(6.0))
                                .bg(cx.theme().danger.opacity(0.08))
                                .border_1()
                                .border_color(cx.theme().danger.opacity(0.25))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().danger)
                                        .child("Authentication required for selected provider"),
                                )
                                .child(TextInput::new(&self.auth_token_input).w_full().xsmall())
                                .child(
                                    h_flex()
                                        .w_full()
                                        .gap_1()
                                        .child(
                                            Button::new("agent-chat-auth-browser")
                                                .xsmall()
                                                .ghost()
                                                .label("Open Browser")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.begin_browser_auth(cx);
                                                })),
                                        )
                                        .child(
                                            Button::new("agent-chat-auth-token")
                                                .xsmall()
                                                .primary()
                                                .label("Use Token")
                                                .disabled(
                                                    self.auth_token_input
                                                        .read(cx)
                                                        .text()
                                                        .to_string()
                                                        .trim()
                                                        .is_empty(),
                                                )
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.complete_prompt_auth(window, cx);
                                                })),
                                            ),
                                        ),
                        )
                    }),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .child(
                        v_virtual_list(
                            cx.entity().clone(),
                            "agent-chat-messages-virtual-list",
                            message_item_sizes,
                            move |
                                this,
                                range: std::ops::Range<usize>,
                                _window,
                                cx: &mut Context<Self>,
                            | {
                                range
                                    .map(|ix| {
                                        let Some(message) = this.messages.get(ix) else {
                                            return div().h(px(52.0)).into_any_element();
                                        };

                                        let is_user = message.role == "user";
                                        let panel = cx.entity().clone();

                                        div()
                                            .relative()
                                            .w_full()
                                            .min_w_0()
                                            .px_3()
                                            .py_1()
                                            .child(
                                                h_flex()
                                                    .w_full()
                                                    .min_w_0()
                                                    .justify_start()
                                                    .when(is_user, |el| el.justify_end())
                                                    .child(
                                                        v_flex()
                                                            .w_auto()
                                                            .max_w(px(620.0))
                                                            .min_w_0()
                                                            .gap_1()
                                                            .px_3()
                                                            .py_2()
                                                            .rounded(px(8.0))
                                                            .bg(if is_user {
                                                                cx.theme().primary.opacity(0.16)
                                                            } else {
                                                                cx.theme().secondary
                                                            })
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .font_semibold()
                                                                    .text_color(cx.theme().muted_foreground)
                                                                    .child(if is_user {
                                                                        "You"
                                                                    } else {
                                                                        "Agent"
                                                                    }),
                                                            )
                                                            .child(
                                                                div()
                                                                    .w_full()
                                                                    .min_w_0()
                                                                    .whitespace_normal()
                                                                    .text_sm()
                                                                    .text_color(cx.theme().foreground)
                                                                    .child(message.content.clone()),
                                                            ),
                                                    )
                                                    .id(("agent-chat-message", ix)),
                                            )
                                            .child(
                                                canvas(
                                                    move |bounds, _, cx| {
                                                        panel.update(cx, |panel, cx| {
                                                            let measured = bounds.size.height;
                                                            if panel
                                                                .message_row_heights
                                                                .get(&ix)
                                                                .copied()
                                                                != Some(measured)
                                                            {
                                                                panel.message_row_heights.insert(
                                                                    ix, measured,
                                                                );
                                                                cx.notify();
                                                            }
                                                        });
                                                    },
                                                    |_, _, _, _| {},
                                                )
                                                .absolute()
                                                .inset_0(),
                                            )
                                            .into_any_element()
                                    })
                                    .collect::<Vec<_>>()
                            },
                        )
                        .track_scroll(&self.messages_scroll_handle)
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .child(Scrollbar::vertical(
                                &self.messages_scroll_state,
                                &self.messages_scroll_handle,
                            )),
                    ),
            )
            .child(
                v_flex()
                    .w_full()
                    .gap_1()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap_1()
                            .items_center()
                            .child(chat_history_popover)
                            .child(
                                Button::new("agent-chat-new-chat")
                                    .xsmall()
                                    .ghost()
                                    .label("+")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.start_new_chat(cx);
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .child(TextInput::new(&self.prompt_input).flex_1().min_w_0())
                            .child(
                                // Rope-based input text is converted to String for validation.
                                Button::new("agent-chat-send")
                                    .icon(IconName::Send)
                                    .label("Send")
                                    .disabled(
                                        self.prompt_input
                                            .read(cx)
                                            .text()
                                            .to_string()
                                            .trim()
                                            .is_empty(),
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.send_prompt(window, cx);
                                    })),
                            ),
                    ),
            )
    }
}