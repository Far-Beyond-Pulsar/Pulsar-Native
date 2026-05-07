use gpui::{prelude::FluentBuilder as _, *};
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    dock::{Panel, PanelEvent},
    dropdown::{SearchableList, SearchableListEvent},
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
            .with_max_height(px(180.0))
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
            .with_max_height(px(180.0))
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
                ]),
            },
            ProviderDefinition {
                id: "anthropic",
                label: "Anthropic",
                kind: ProviderKind::Cloud,
                endpoint: "https://api.anthropic.com/v1",
                models: Arc::new(vec![
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
                ]),
            },
            ProviderDefinition {
                id: "lmstudio",
                label: "LM Studio",
                kind: ProviderKind::Local,
                endpoint: "http://localhost:1234/v1",
                models: Arc::new(vec![ModelDefinition {
                    id: "local-default",
                    label: "Local Default",
                    supports_tools: false,
                }]),
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

    fn cycle_provider(&mut self, step: isize, cx: &mut Context<Self>) {
        if self.provider_catalog.is_empty() {
            return;
        }

        let len = self.provider_catalog.len() as isize;
        let current = self.active_provider_ix as isize;
        let next = (current + step).rem_euclid(len) as usize;
        self.active_provider_ix = next;
        self.active_model_ix = 0;
        cx.notify();
    }

    fn cycle_model(&mut self, step: isize, cx: &mut Context<Self>) {
        let Some(provider) = self.active_provider() else {
            return;
        };
        if provider.models.is_empty() {
            return;
        }

        let len = provider.models.len() as isize;
        let current = self.active_model_ix as isize;
        let next = (current + step).rem_euclid(len) as usize;
        self.active_model_ix = next;
        cx.notify();
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

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().tab_bar)
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(div().text_sm().font_semibold().child("Agentic Chat"))
                            .child(
                                div().text_xs().text_color(cx.theme().muted_foreground).child(
                                    provider
                                        .map(|p| match p.kind {
                                            ProviderKind::Cloud => "Cloud",
                                            ProviderKind::Local => "Local",
                                        })
                                        .unwrap_or("N/A"),
                                ),
                            ),
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Provider"),
                            )
                            .child(self.provider_list.clone()),
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Model"),
                            )
                            .child(self.model_list.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                provider
                                    .map(|p| format!("Endpoint: {}", p.endpoint))
                                    .unwrap_or_else(|| "Endpoint: n/a".to_string()),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(match model {
                                Some(m) if m.supports_tools => "Tools: supported",
                                Some(_) => "Tools: limited",
                                None => "Tools: unknown",
                            }),
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
                    .gap_2()
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
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Provider adapters are intentionally decoupled so cloud and local transports can be plugged in independently."),
                    ),
            )
    }
}