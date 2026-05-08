use gpui::{prelude::FluentBuilder as _, *};
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    dock::{Panel, PanelEvent},
    dropdown::{SearchableList, SearchableListEvent},
    popover::Popover,
    Disableable,
    h_flex,
    input::{InputState, TextInput},
    v_flex, ActiveTheme as _, IconName, Sizable, StyledExt,
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

pub struct AgentChatPanel {
    focus_handle: FocusHandle,
    prompt_input: Entity<InputState>,
    provider_list: Entity<SearchableList<ProviderDefinition>>,
    model_list: Entity<SearchableList<ModelDefinition>>,
    provider_catalog: Vec<ProviderDefinition>,
    active_provider_ix: usize,
    active_model_ix: usize,
    messages: Vec<ChatMessage>,
    _subscriptions: Vec<Subscription>,
}

impl AgentChatPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let provider_catalog = Self::default_provider_catalog();
        let prompt_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Ask the engine assistant..."));

        let provider_list = cx.new(|cx| {
            SearchableList::new(window, cx, provider_catalog.clone(), |p: &ProviderDefinition| {
                format!("{} ({})", p.label, p.id)
            })
            .with_empty_text("No providers found")
            .with_max_width(px(220.0))
            .with_max_height(px(320.0))
            .with_icon_getter(|_| IconName::Brain)
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
        ];

        Self {
            focus_handle: cx.focus_handle(),
            prompt_input,
            provider_list,
            model_list,
            provider_catalog,
            active_provider_ix: 0,
            active_model_ix: 0,
            messages: vec![ChatMessage {
                role: "system",
                content: "Agent Chat is ready. Choose provider/model and ask anything about your project.".to_string(),
            }],
            _subscriptions: subscriptions,
        }
    }

    fn default_provider_catalog() -> Vec<ProviderDefinition> {
        vec![
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
                label: "GitHub Copilot",
                kind: ProviderKind::Cloud,
                endpoint: "https://api.githubcopilot.com/chat/completions",
                models: Arc::new(vec![
                    ModelDefinition {
                        id: "gpt-5.3-codex",
                        label: "GPT-5.3 Codex (Copilot)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "claude-sonnet-4",
                        label: "Claude Sonnet 4 (Copilot)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "o4-mini",
                        label: "o4 Mini (Copilot)",
                        supports_tools: true,
                    },
                    ModelDefinition {
                        id: "gemini-2.5-pro",
                        label: "Gemini 2.5 Pro (Copilot)",
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

    fn active_model(&self) -> Option<&ModelDefinition> {
        self.active_provider()
            .and_then(|provider| provider.models.get(self.active_model_ix))
    }

    fn set_provider(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.provider_catalog.len() {
            self.active_provider_ix = index;
            self.active_model_ix = 0;

            let models = self
                .active_provider()
                .map(|provider| provider.models.as_ref().clone())
                .unwrap_or_default();
            self.model_list.update(cx, |list, cx| {
                list.set_items(models, cx);
            });

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

        let provider = self
            .active_provider()
            .map(|p| p.label)
            .unwrap_or("Unknown Provider");
        let model = self.active_model().map(|m| m.label).unwrap_or("Unknown Model");
        self.messages.push(ChatMessage {
            role: "assistant",
            content: format!(
                "Queued with {provider} / {model}. Provider adapters are modular; add a transport implementation to stream live responses here."
            ),
        });

        self.prompt_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
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
        let provider = self.active_provider();
        let model = self.active_model();
        let provider_list = self.provider_list.clone();
        let model_list = self.model_list.clone();

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

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
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
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .px_3()
                    .py_2()
                    .child(
                        v_flex()
                            .w_full()
                            .gap_2()
                            .children(self.messages.iter().enumerate().map(|(ix, message)| {
                                let is_user = message.role == "user";
                                h_flex()
                                    .w_full()
                                    .justify_start()
                                    .when(is_user, |this| this.justify_end())
                                    .child(
                                        v_flex()
                                            .max_w(px(520.0))
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
                                                    .child(if is_user { "You" } else { "Agent" }),
                                            )
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(cx.theme().foreground)
                                                    .child(message.content.clone()),
                                            ),
                                    )
                                    .id(("agent-chat-message", ix))
                            })),
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
                            .gap_2()
                            .items_center()
                            .child(TextInput::new(&self.prompt_input).flex_1())
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