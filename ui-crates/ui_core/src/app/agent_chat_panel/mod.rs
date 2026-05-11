mod auth;
mod chat_history;
mod chat_storage;
mod custom_provider_wizard;
mod provider_catalog;
mod provider_selection;
mod streaming;
pub mod types;

pub use types::*;

use crate::custom_providers::{self, CustomProvider};
use agent_chat_core::{ChatMessage, ChatRole, ProviderRegistry};
use agent_chat_tools::ToolRegistry;
use agent_provider_anthropic::AnthropicProvider;
use agent_provider_aws_bedrock::AwsBedrockProvider;
use agent_provider_azure_openai::AzureOpenAIProvider;
use agent_provider_cohere::CohereProvider;
use agent_provider_deepseek::DeepSeekProvider;
use agent_provider_demo_random::DemoRandomProvider;
use agent_provider_docker_model_runner::DockerModelRunnerProvider;
use agent_provider_fireworks::FireworksProvider;
use agent_provider_gemini::GeminiProvider;
use agent_provider_github_copilot::GithubCopilotProvider;
use agent_provider_groq::GroqProvider;
use agent_provider_llama_cpp::LlamaCppProvider;
use agent_provider_lmstudio::LmStudioProvider;
use agent_provider_mistral::MistralProvider;
use agent_provider_ollama::OllamaProvider;
use agent_provider_openai::{OpenAiCompatibleProvider, OpenAiProvider};
use agent_provider_openrouter::OpenRouterProvider;
use agent_provider_perplexity::PerplexityProvider;
use agent_provider_together::TogetherProvider;
use agent_provider_vertex_ai::VertexAiProvider;
use agent_provider_vllm::VllmProvider;
use agent_provider_xai::XaiProvider;
use gpui::{prelude::FluentBuilder as _, *};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    sync::{Arc, RwLock},
};
use ui::{
    button::{Button, ButtonVariants as _},
    dock::{Panel, PanelEvent},
    dropdown::{
        SearchableList, SearchableListEvent, SearchableListItemAction, SearchableListItemState,
    },
    h_flex,
    input::Enter,
    input::{InputState, TextInput},
    popover::Popover,
    scroll::{Scrollbar, ScrollbarState},
    spinner::Spinner,
    text::TextView,
    v_flex, v_virtual_list, ActiveTheme as _, Disableable, Icon, IconName, Sizable, Size,
    StyledExt, VirtualListScrollHandle,
};

pub struct AgentChatPanel {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) messages_scroll_handle: VirtualListScrollHandle,
    pub(crate) messages_scroll_state: ScrollbarState,
    pub(crate) prompt_input: Entity<InputState>,
    pub(crate) auth_token_input: Entity<InputState>,
    pub(crate) custom_provider_input: Entity<InputState>,
    pub(crate) chat_history_list: Entity<SearchableList<ChatHistoryEntry>>,
    pub(crate) provider_list: Entity<SearchableList<ProviderDefinition>>,
    pub(crate) model_list: Entity<SearchableList<ModelDefinition>>,
    pub(crate) provider_catalog: Vec<ProviderDefinition>,
    pub(crate) custom_providers_list: Vec<CustomProvider>,
    pub(crate) custom_provider_ids: Rc<RefCell<HashSet<String>>>,
    pub(crate) pending_custom_provider: Option<PendingCustomProvider>,
    pub(crate) pending_custom_provider_step: Option<AddProviderPromptStep>,
    pub(crate) wip_providers: HashMap<&'static str, String>,
    pub(crate) provider_registry: ProviderRegistry,
    pub(crate) tool_registry: ToolRegistry,
    pub(crate) plugin_bridge: Option<Arc<RwLock<plugin_manager::PluginToolBridge>>>,
    pub(crate) provider_tokens: HashMap<&'static str, String>,
    pub(crate) pending_auth_provider: Option<&'static str>,
    pub(crate) pending_device_code: Option<String>,
    pub(crate) current_chat_id: String,
    pub(crate) current_chat_created_at: u64,
    pub(crate) loaded_chat_project_root: Option<PathBuf>,
    pub(crate) message_row_heights: HashMap<usize, Pixels>,
    pub(crate) active_provider_ix: usize,
    pub(crate) active_model_ix: usize,
    pub(crate) is_request_in_flight: bool,
    pub(crate) streaming_message_ix: Option<usize>,
    /// Index into `display_items` of the currently-streaming assistant bubble.
    pub(crate) streaming_display_item_ix: Option<usize>,
    pub(crate) pending_rollback_confirm_ix: Option<usize>,
    pub(crate) messages: Vec<ChatMessage>,
    /// Flat list of items rendered in the chat — user/assistant bubbles plus
    /// collapsed tool-call blocks. System and raw Tool-role messages are excluded.
    pub(crate) display_items: Vec<DisplayItem>,
    pub(crate) display_item_heights: HashMap<usize, Pixels>,
    /// Sender half of the per-request cancel channel. `None` when idle.
    pub(crate) cancel_tx: Option<smol::channel::Sender<()>>,
    /// Whether the chat should automatically scroll to new content.
    /// Disabled when the user explicitly scrolls up; re-enabled when they scroll
    /// back near the bottom or click the "jump to bottom" button.
    pub(crate) auto_scroll: bool,
    /// Height of the message-list viewport in pixels — measured each render frame.
    pub(crate) chat_viewport_height: Pixels,
    pub(crate) _subscriptions: Vec<Subscription>,
}

impl AgentChatPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // --- Provider registry: single source of truth, no separate catalog ---
        let mut provider_registry = ProviderRegistry::new();

        // Implemented cloud providers
        provider_registry.register(Arc::new(GithubCopilotProvider::new()));
        provider_registry.register(Arc::new(DemoRandomProvider::new()));
        provider_registry.register(Arc::new(OpenAiProvider::new()));
        provider_registry.register(Arc::new(AnthropicProvider::new()));
        provider_registry.register(Arc::new(OpenRouterProvider::new()));
        provider_registry.register(Arc::new(GroqProvider::new()));
        provider_registry.register(Arc::new(TogetherProvider::new()));
        provider_registry.register(Arc::new(MistralProvider::new()));
        provider_registry.register(Arc::new(GeminiProvider::new()));
        provider_registry.register(Arc::new(XaiProvider::new()));
        provider_registry.register(Arc::new(DeepSeekProvider::new()));
        provider_registry.register(Arc::new(FireworksProvider::new()));
        provider_registry.register(Arc::new(PerplexityProvider::new()));
        // Hollow cloud providers (real structure, not yet fully implemented)
        provider_registry.register(Arc::new(AzureOpenAIProvider::new()));
        provider_registry.register(Arc::new(AwsBedrockProvider::new()));
        provider_registry.register(Arc::new(VertexAiProvider::new()));
        provider_registry.register(Arc::new(CohereProvider::new()));
        // Local providers
        provider_registry.register(Arc::new(DockerModelRunnerProvider::new()));
        provider_registry.register(Arc::new(OllamaProvider::new()));
        provider_registry.register(Arc::new(LmStudioProvider::new()));
        provider_registry.register(Arc::new(VllmProvider::new()));
        provider_registry.register(Arc::new(LlamaCppProvider::new()));

        // --- Custom providers added by the user ---
        let custom_providers_list =
            custom_providers::load_custom_providers(&Self::custom_provider_config_dir());
        let custom_provider_ids = Rc::new(RefCell::new(
            custom_providers_list
                .iter()
                .map(|provider| provider.id.clone())
                .collect::<HashSet<_>>(),
        ));
        for provider in &custom_providers_list {
            let models = provider
                .models
                .iter()
                .map(|model| (model.id.clone(), model.label.clone(), model.supports_tools))
                .collect::<Vec<_>>();
            let use_ollama = custom_provider_ids.borrow().contains(provider.id.as_str());
            let runtime_provider = if use_ollama {
                OpenAiCompatibleProvider::from_dynamic_ollama(
                    provider.id.clone(),
                    provider.label.clone(),
                    provider.endpoint.clone(),
                    agent_chat_core::ProviderKind::Local,
                    models,
                )
            } else {
                OpenAiCompatibleProvider::from_dynamic(
                    provider.id.clone(),
                    provider.label.clone(),
                    provider.endpoint.clone(),
                    agent_chat_core::ProviderKind::Local,
                    models,
                )
            };
            provider_registry.register(Arc::new(runtime_provider));
        }

        // --- Build provider catalog from registry (providers own their metadata) ---
        let env = agent_chat_core::ProcessEnvironment;
        let mut provider_catalog: Vec<ProviderDefinition> = provider_registry
            .catalog(&env)
            .into_iter()
            .map(|entry| ProviderDefinition {
                id: entry.metadata.id,
                label: entry.metadata.display_name,
                kind: match entry.metadata.kind {
                    agent_chat_core::ProviderKind::Cloud => ProviderKind::Cloud,
                    agent_chat_core::ProviderKind::Local => ProviderKind::Local,
                },
                endpoint: entry.metadata.endpoint,
                models: Arc::new(
                    entry
                        .models
                        .iter()
                        .map(|m| ModelDefinition {
                            id: m.id,
                            label: m.label,
                            supports_tools: m.supports_tools,
                            context_tokens: m.context_tokens,
                            compact_model: m.compact_model,
                        })
                        .collect(),
                ),
            })
            .collect();

        // Append custom provider definitions for display
        provider_catalog.extend(
            custom_providers_list
                .iter()
                .map(Self::custom_provider_to_definition),
        );

        // Providers that are not immediately usable: Wip (not implemented) or
        // RequiresAuth (implemented but no API key configured). Both are shown
        // greyed-out at the bottom of the sorted dropdown.
        let wip_providers: HashMap<&'static str, String> = provider_registry
            .catalog(&env)
            .into_iter()
            .filter(|e| {
                matches!(
                    e.availability.state,
                    agent_chat_core::AvailabilityState::Wip
                        | agent_chat_core::AvailabilityState::RequiresAuth
                )
            })
            .map(|e| {
                let reason = match e.availability.state {
                    agent_chat_core::AvailabilityState::Wip => "Not yet implemented".to_string(),
                    _ => "API key not configured".to_string(),
                };
                (e.metadata.id, reason)
            })
            .collect();

        // Sort: active providers alphabetically first, WIP/disabled alphabetically at the bottom.
        provider_catalog.sort_by(|a, b| {
            let a_wip = wip_providers.contains_key(a.id);
            let b_wip = wip_providers.contains_key(b.id);
            match (a_wip, b_wip) {
                (false, true) => std::cmp::Ordering::Less,
                (true, false) => std::cmp::Ordering::Greater,
                _ => a.label.cmp(b.label),
            }
        });
        let plugin_bridge = plugin_manager::global().and_then(|manager_lock| {
            manager_lock
                .read()
                .ok()
                .map(|manager| Arc::new(RwLock::new(manager.build_tool_bridge())))
        });

        let prompt_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Ask the engine assistant..."));
        let auth_token_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Paste provider token..."));
        let custom_provider_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Enter provider field value..."));

        let chat_history_list = cx.new(|cx| {
            SearchableList::new(window, cx, Vec::<ChatHistoryEntry>::new(), |chat| {
                format!("{} ({})", chat.title, chat.id)
            })
            .with_empty_text("No chats found")
            .with_max_width(px(340.0))
            .with_max_height(px(200.0))
        });

        let wip_for_list = wip_providers.clone();
        let custom_ids_for_list = custom_provider_ids.clone();
        let provider_list = cx.new(|cx| {
            SearchableList::new(
                window,
                cx,
                provider_catalog.clone(),
                |p: &ProviderDefinition| format!("{} ({})", p.label, p.id),
            )
            .with_empty_text("No providers found")
            .with_max_width(px(220.0))
            .with_max_height(px(320.0))
            .with_icon_getter(|p: &ProviderDefinition| match p.kind {
                ProviderKind::Cloud => IconName::Cloud,
                ProviderKind::Local => IconName::Server,
            })
            .with_item_actions(move |provider| {
                if custom_ids_for_list.borrow().contains(provider.id) {
                    vec![SearchableListItemAction {
                        id: "delete".into(),
                        icon: Some(IconName::Trash),
                        label: None,
                        destructive: true,
                    }]
                } else {
                    Vec::new()
                }
            })
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
                move |this, _, event: &SearchableListEvent<ProviderDefinition>, cx| match event {
                    SearchableListEvent::Select(selected_provider) => {
                        if let Some(index) = this
                            .provider_catalog
                            .iter()
                            .position(|provider| provider.id == selected_provider.id)
                        {
                            this.set_provider(index, cx);
                        }
                    }
                    SearchableListEvent::Action { item, action_id } => {
                        if action_id.as_ref() == "delete" {
                            this.delete_custom_provider(item.id, cx);
                        }
                    }
                },
            ),
            cx.subscribe(
                &model_list,
                move |this, _, event: &SearchableListEvent<ModelDefinition>, cx| {
                    if let SearchableListEvent::Select(selected_model) = event {
                        if let Some(provider) = this.active_provider() {
                            if let Some(index) = provider
                                .models
                                .iter()
                                .position(|model| model.id == selected_model.id)
                            {
                                this.set_model(index, cx);
                            }
                        }
                    }
                },
            ),
            cx.subscribe(
                &chat_history_list,
                move |this, _, event: &SearchableListEvent<ChatHistoryEntry>, cx| {
                    if let SearchableListEvent::Select(entry) = event {
                        this.load_chat_session(&entry.id, cx);
                    }
                },
            ),
        ];

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            messages_scroll_handle: VirtualListScrollHandle::new(),
            messages_scroll_state: ScrollbarState::default(),
            prompt_input,
            auth_token_input,
            custom_provider_input,
            chat_history_list,
            provider_list,
            model_list,
            provider_catalog,
            custom_providers_list,
            custom_provider_ids,
            pending_custom_provider: None,
            pending_custom_provider_step: None,
            wip_providers,
            provider_registry,
            tool_registry: ToolRegistry::with_default_tools(),
            plugin_bridge,
            provider_tokens: HashMap::new(),
            pending_auth_provider: None,
            pending_device_code: None,
            current_chat_id: String::new(),
            current_chat_created_at: 0,
            loaded_chat_project_root: None,
            message_row_heights: HashMap::new(),
            active_provider_ix: 0,
            active_model_ix: 0,
            is_request_in_flight: false,
            streaming_message_ix: None,
            streaming_display_item_ix: None,
            pending_rollback_confirm_ix: None,
            messages: vec![ChatMessage {
                role: ChatRole::System,
                content: "Agent Chat is ready. Choose provider/model and ask anything about your project.".to_string(),
                tool_call_id: None,
                tool_calls: vec![],
            }],
            display_items: vec![],
            display_item_heights: HashMap::new(),
            cancel_tx: None,
            auto_scroll: true,
            chat_viewport_height: px(0.0),
            _subscriptions: subscriptions,
        };

        this.bootstrap_chat_storage(cx);
        this
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

        // ── Auto-scroll safety net ─────────────────────────────────────────────
        // Scroll intent is detected via on_scroll_wheel on the container (below).
        // Here we only handle two passive cases:
        //   • request ended → always reset to following
        //   • user managed to drag/scroll back to within 100px of bottom → re-enable
        if !self.is_request_in_flight {
            self.auto_scroll = true;
        } else if !self.auto_scroll && self.distance_from_bottom() < px(100.0) {
            self.auto_scroll = true;
        }

        let show_jump_button = !self.auto_scroll && self.distance_from_bottom() > px(100.0);

        let provider = self.active_provider();
        let model = self.active_model();
        let auth_provider = self.pending_auth_provider;
        let add_provider_prompt = self
            .pending_custom_provider_step
            .map(Self::add_provider_prompt_title);
        let provider_list = self.provider_list.clone();
        let model_list = self.model_list.clone();
        let chat_history_list = self.chat_history_list.clone();
        let current_chat_id = self.current_chat_id.clone();
        let display_count = self.display_items.len();
        let render_now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let display_item_sizes = std::rc::Rc::new(
            self.display_items
                .iter()
                .enumerate()
                .map(|(ix, item)| {
                    let h = self
                        .display_item_heights
                        .get(&ix)
                        .copied()
                        .unwrap_or_else(|| Self::display_item_height(item));
                    size(px(0.0), h)
                })
                .chain(std::iter::once(size(px(0.0), px(120.0))))
                .collect::<Vec<_>>(),
        );

        let provider_label = provider
            .map(|p| p.label.to_string())
            .unwrap_or_else(|| "Provider".to_string());
        let model_label = model
            .map(|m| m.label.to_string())
            .unwrap_or_else(|| "Model".to_string());

        let provider_popover =
            Popover::<SearchableList<ProviderDefinition>>::new("agent-chat-provider-popover")
                .anchor(Corner::TopLeft)
                .trigger(
                    Button::new("agent-chat-provider-trigger")
                        .small()
                        .ghost()
                        .justify_start()
                        .tooltip("Select provider")
                        .label(provider_label)
                        .dropdown_caret(true),
                )
                .content(move |_window, _cx| provider_list.clone());

        let model_popover =
            Popover::<SearchableList<ModelDefinition>>::new("agent-chat-model-popover")
                .anchor(Corner::TopLeft)
                .trigger(
                    Button::new("agent-chat-model-trigger")
                        .small()
                        .ghost()
                        .justify_start()
                        .tooltip("Select model")
                        .label(model_label)
                        .dropdown_caret(true),
                )
                .content(move |_window, _cx| model_list.clone());

        let chat_history_popover =
            Popover::<SearchableList<ChatHistoryEntry>>::new("agent-chat-history-popover")
                .anchor(Corner::BottomLeft)
                .trigger(
                    Button::new("agent-chat-history-trigger")
                        .xsmall()
                        .ghost()
                        .justify_start()
                        .tooltip("Switch chat")
                        .icon(IconName::ChatBubble)
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
                    .px_3()
                    .py(px(6.0))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().tab_bar)
                    .child(
                        // Single header row: provider | model | refresh | + add
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap_1()
                            .child(provider_popover)
                            .child(
                                div()
                                    .text_color(cx.theme().border)
                                    .text_sm()
                                    .child("/"),
                            )
                            .child(model_popover)
                            .flex_1()
                            .child(
                                Button::new("agent-chat-refresh-models")
                                    .icon(IconName::Refresh)
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Refresh model list from provider")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.refresh_models_for_active_provider(cx);
                                    })),
                            )
                            .child(
                                Button::new("agent-chat-add-provider")
                                    .icon(IconName::Plus)
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Add custom provider")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.start_add_provider_prompt(window, cx);
                                    })),
                            ),
                    )
                    .child({
                        // Context-window usage meter.
                        let total_chars = self.active_context_chars();
                        let sliver_chars = Self::COMPACTION_SUMMARY_CHAR_BUDGET;
                        let usable_chars = total_chars.saturating_sub(sliver_chars);
                        let used: usize = self.messages.iter().map(|m| m.content.len()).sum();
                        // Fraction of the USABLE window filled (not counting the sliver).
                        let fill_pct = (used as f32 / usable_chars.max(1) as f32).min(1.0);
                        // Fraction of total window the sliver occupies.
                        let sliver_pct = sliver_chars as f32 / total_chars.max(1) as f32;
                        let bar_color = if fill_pct > 0.85 {
                            cx.theme().danger
                        } else if fill_pct > 0.6 {
                            cx.theme().warning
                        } else {
                            cx.theme().success
                        };
                        let model_ctx = self.active_model()
                            .and_then(|m| {
                                if m.context_tokens > 0 {
                                    Some(m.context_tokens)
                                } else {
                                    Self::infer_context_tokens(m.id).map(|t| t as u32)
                                }
                            })
                            .unwrap_or(6_000);
                        let ctx_label = if model_ctx >= 1_000_000 {
                            format!("{}M ctx", model_ctx / 1_000_000)
                        } else if model_ctx >= 1_000 {
                            format!("{}k ctx", model_ctx / 1_000)
                        } else {
                            format!("{} ctx", model_ctx)
                        };
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap_1()
                            .px(px(2.0))
                            .child(
                                // Outer track
                                div()
                                    .flex_1()
                                    .h(px(4.0))
                                    .rounded_full()
                                    .bg(cx.theme().border.opacity(0.3))
                                    .relative()
                                    // Filled portion (usable budget consumed)
                                    .child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .left_0()
                                            .h_full()
                                            .rounded_full()
                                            .bg(bar_color)
                                            .w(relative(fill_pct * (1.0 - sliver_pct))),
                                    )
                                    // Compaction-sliver indicator (right edge, always visible)
                                    .child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .h_full()
                                            .right_0()
                                            .rounded_r_full()
                                            .bg(cx.theme().muted_foreground.opacity(0.25))
                                            .w(relative(sliver_pct)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground.opacity(0.7))
                                    .text_xs()
                                    .child(format!("{}% · {}", (fill_pct * 100.0) as u32, ctx_label)),
                            )
                    })
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
                                                .tooltip("Authenticate with browser")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.begin_browser_auth(cx);
                                                })),
                                        )
                                        .child(
                                            Button::new("agent-chat-auth-token")
                                                .xsmall()
                                                .primary()
                                                .label("Use Token")
                                                .tooltip("Confirm authentication token")
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
                    })
                    .when(add_provider_prompt.is_some(), |el| {
                        el.child(
                            v_flex()
                                .w_full()
                                .gap_1()
                                .p_2()
                                .rounded(px(6.0))
                                .bg(cx.theme().primary.opacity(0.08))
                                .border_1()
                                .border_color(cx.theme().primary.opacity(0.25))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().primary)
                                        .child(add_provider_prompt.unwrap_or("")),
                                )
                                .child(
                                    TextInput::new(&self.custom_provider_input)
                                        .w_full()
                                        .xsmall(),
                                )
                                .child(
                                    h_flex()
                                        .w_full()
                                        .gap_1()
                                        .child(
                                            Button::new("agent-chat-add-provider-cancel")
                                                .xsmall()
                                                .ghost()
                                                .label("Cancel")
                                                .tooltip("Cancel adding provider")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.cancel_add_provider_prompt(window, cx);
                                                })),
                                        )
                                        .child(
                                            Button::new("agent-chat-add-provider-next")
                                                .xsmall()
                                                .primary()
                                                .label("Next")
                                                .tooltip("Continue to next step")
                                                .disabled(
                                                    self.custom_provider_input
                                                        .read(cx)
                                                        .text()
                                                        .to_string()
                                                        .trim()
                                                        .is_empty(),
                                                )
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.submit_add_provider_prompt(window, cx);
                                                })),
                                        ),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .id("agent-chat-scroll-area")
                    .relative()
                    .flex_1()
                    // Detect explicit user scroll-up → pause auto-scroll.
                    // Scrolling down is handled by the distance_from_bottom check above.
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                        // Detect explicit upward scroll — stop auto-following the bottom.
                        let scrolled_up = match event.delta {
                            ScrollDelta::Pixels(p) => p.y < px(0.0),
                            ScrollDelta::Lines(l) => l.y < 0.0,
                        };
                        if scrolled_up && this.auto_scroll {
                            this.auto_scroll = false;
                            cx.notify();
                        }
                        // Scroll down is handled passively via distance_from_bottom in render.
                    }))
                    .child(
                        v_virtual_list(
                            cx.entity().clone(),
                            "agent-chat-messages-virtual-list",
                            display_item_sizes,
                            move |this,
                                  range: std::ops::Range<usize>,
                                  window,
                                  cx: &mut Context<Self>| {
                                range
                                    .map(|ix| {
                                        if ix == display_count {
                                            return div().h(px(120.0)).into_any_element();
                                        }

                                        let Some(item) = this.display_items.get(ix) else {
                                            return div().h(px(52.0)).into_any_element();
                                        };

                                        let panel = cx.entity().clone();

                                        match item {
                                            DisplayItem::ToolCallGroup { calls, is_expanded, started_at_ms, finished_at_ms } => {
                                                let calls = calls.clone();
                                                let is_expanded = *is_expanded;
                                                let group_elapsed = Self::format_elapsed(*started_at_ms, *finished_at_ms, render_now_ms);
                                                let tool_names: Vec<String> =
                                                    calls.iter().map(|c| c.name.clone()).collect();
                                                let all_done =
                                                    calls.iter().all(|c| c.result_preview.is_some());
                                                let has_error =
                                                    calls.iter().any(|c| c.is_error);

                                                let accent = if has_error {
                                                    cx.theme().danger
                                                } else if all_done {
                                                    cx.theme().success
                                                } else {
                                                    cx.theme().muted_foreground
                                                };

                                                let status_icon = if has_error {
                                                    IconName::CircleX
                                                } else if all_done {
                                                    IconName::CircleCheck
                                                } else {
                                                    IconName::Loader
                                                };

                                                let header_label = if calls.len() == 1 {
                                                    format!("Used tool: {}", tool_names[0])
                                                } else {
                                                    format!(
                                                        "Used {} tools: {}",
                                                        calls.len(),
                                                        tool_names.join(", ")
                                                    )
                                                };

                                                div()
                                                    .relative()
                                                    .w_full()
                                                    .min_w_0()
                                                    .px_3()
                                                    .py_1()
                                                    .child(
                                                        canvas(
                                                            move |bounds, _, cx| {
                                                                panel.update(cx, |panel, cx| {
                                                                    let measured =
                                                                        bounds.size.height;
                                                                    if panel
                                                                        .display_item_heights
                                                                        .get(&ix)
                                                                        .copied()
                                                                        != Some(measured)
                                                                    {
                                                                        panel
                                                                            .display_item_heights
                                                                            .insert(ix, measured);
                                                                        cx.notify();
                                                                    }
                                                                });
                                                            },
                                                            |_, _, _, _| {},
                                                        )
                                                        .absolute()
                                                        .inset_0(),
                                                    )
                                                    .child(
                                                        v_flex()
                                                            .w_full()
                                                            .gap_px()
                                                            .rounded(px(6.0))
                                                            .border_1()
                                                            .border_color(accent.opacity(0.25))
                                                            .bg(cx.theme().secondary)
                                                            .overflow_hidden()
                                                            .child(
                                                                // Header row — always visible
                                                                h_flex()
                                                                    .id(("tool-call-header", ix))
                                                                    .w_full()
                                                                    .px_3()
                                                                    .py(px(6.0))
                                                                    .gap_2()
                                                                    .cursor_pointer()
                                                                    .on_click(cx.listener(
                                                                        move |this, _, _, cx| {
                                                                            if let Some(
                                                                                DisplayItem::ToolCallGroup {
                                                                                    is_expanded,
                                                                                    ..
                                                                                },
                                                                            ) = this
                                                                                .display_items
                                                                                .get_mut(ix)
                                                                            {
                                                                                *is_expanded =
                                                                                    !*is_expanded;
                                                                                this.display_item_heights
                                                                                    .remove(&ix);
                                                                            }
                                                                            cx.notify();
                                                                        },
                                                                    ))
                                                                    .child(
                                                                        // Category icon (tool wrench)
                                                                        Icon::new(IconName::Terminal)
                                                                            .size_3()
                                                                            .text_color(accent),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .flex_1()
                                                                            .text_xs()
                                                                            .text_color(
                                                                                cx.theme()
                                                                                    .muted_foreground,
                                                                            )
                                                                            .child(header_label),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(cx.theme().muted_foreground.opacity(0.6))
                                                                            .font_family("JetBrains Mono")
                                                                            .child(group_elapsed),
                                                                    )
                                                                    .child(
                                                                        // Status icon — shape conveys state for colorblind users
                                                                        Icon::new(status_icon)
                                                                            .size_3()
                                                                            .text_color(accent),
                                                                    )
                                                                    .child(
                                                                        Icon::new(if is_expanded {
                                                                            IconName::ChevronUp
                                                                        } else {
                                                                            IconName::ChevronDown
                                                                        })
                                                                        .size_3()
                                                                        .text_color(
                                                                            cx.theme()
                                                                                .muted_foreground
                                                                                .opacity(0.6),
                                                                        ),
                                                                    ),
                                                            )
                                                            .when(is_expanded, |el| {
                                                                el.child(
                                                                    v_flex()
                                                                        .w_full()
                                                                        .gap_px()
                                                                        .children(
                                                                            calls.iter().map(|call| {
                                                                                v_flex()
                                                                                    .w_full()
                                                                                    .px_3()
                                                                                    .py_2()
                                                                                    .gap_1()
                                                                                    .border_t_1()
                                                                                    .border_color(
                                                                                        cx.theme()
                                                                                            .border,
                                                                                    )
                                                                                    .child(
                                                                                        h_flex()
                                                                                            .gap_2()
                                                                                            .items_center()
                                                                                            .child(
                                                                                                div()
                                                                                                    .text_sm()
                                                                                                    .font_semibold()
                                                                                                    .text_color(cx.theme().foreground)
                                                                                                    .child(call.name.clone()),
                                                                                            )
                                                                                            .when(call.is_error, |el| {
                                                                                                el.child(
                                                                                                    div()
                                                                                                        .text_xs()
                                                                                                        .text_color(cx.theme().danger)
                                                                                                        .child("error"),
                                                                                                )
                                                                                            }),
                                                                                    )
                                                                                    .child(
                                                                                        div()
                                                                                            .text_xs()
                                                                                            .font_family("JetBrains Mono")
                                                                                            .text_color(cx.theme().muted_foreground)
                                                                                            .child(format!("args: {}", call.args_preview)),
                                                                                    )
                                                                                    .when_some(
                                                                                        call.result_preview.as_ref(),
                                                                                        |el, result| {
                                                                                            el.child(
                                                                                                div()
                                                                                                    .text_xs()
                                                                                                    .font_family("JetBrains Mono")
                                                                                                    .text_color(cx.theme().muted_foreground.opacity(0.8))
                                                                                                    .child(format!("→ {result}")),
                                                                                            )
                                                                                        },
                                                                                    )
                                                                                    .when(
                                                                                        call.result_preview.is_none(),
                                                                                        |el| {
                                                                                            el.child(
                                                                                                div()
                                                                                                    .text_xs()
                                                                                                    .text_color(
                                                                                                        cx.theme()
                                                                                                            .muted_foreground
                                                                                                            .opacity(0.5),
                                                                                                    )
                                                                                                    .child("running…"),
                                                                                            )
                                                                                        },
                                                                                    )
                                                                            }),
                                                                        ),
                                                                )
                                                            }),
                                                    )
                                                    .into_any_element()
                                            }

                                            DisplayItem::CompactionSummary {
                                                summary,
                                                is_expanded,
                                                started_at_ms,
                                                finished_at_ms,
                                            } => {
                                                let summary = summary.clone();
                                                let is_expanded = *is_expanded;
                                                let compact_elapsed = Self::format_elapsed(*started_at_ms, *finished_at_ms, render_now_ms);
                                                let accent = cx.theme().warning;

                                                div()
                                                    .relative()
                                                    .w_full()
                                                    .min_w_0()
                                                    .px_3()
                                                    .py_1()
                                                    .child(
                                                        canvas(
                                                            move |bounds, _, cx| {
                                                                panel.update(cx, |panel, cx| {
                                                                    let measured = bounds.size.height;
                                                                    if panel.display_item_heights.get(&ix).copied() != Some(measured) {
                                                                        panel.display_item_heights.insert(ix, measured);
                                                                        cx.notify();
                                                                    }
                                                                });
                                                            },
                                                            |_, _, _, _| {},
                                                        )
                                                        .absolute()
                                                        .inset_0(),
                                                    )
                                                    .child(
                                                        v_flex()
                                                            .w_full()
                                                            .rounded(px(6.0))
                                                            .border_1()
                                                            .border_color(accent.opacity(0.3))
                                                            .bg(cx.theme().secondary)
                                                            .overflow_hidden()
                                                            .child(
                                                                h_flex()
                                                                    .id(("compaction-header", ix))
                                                                    .w_full()
                                                                    .px_3()
                                                                    .py(px(5.0))
                                                                    .gap_2()
                                                                    .cursor_pointer()
                                                                    .on_click(cx.listener(
                                                                        move |this, _, _, cx| {
                                                                            if let Some(DisplayItem::CompactionSummary { is_expanded, .. }) = this.display_items.get_mut(ix) {
                                                                                *is_expanded = !*is_expanded;
                                                                                this.display_item_heights.remove(&ix);
                                                                            }
                                                                            cx.notify();
                                                                        },
                                                                    ))
                                                                    .child(
                                                                        Icon::new(IconName::Scissor)
                                                                            .size_3()
                                                                            .text_color(accent),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .flex_1()
                                                                            .text_xs()
                                                                            .text_color(cx.theme().muted_foreground)
                                                                            .child("Context compacted — earlier messages summarised"),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(accent.opacity(0.7))
                                                                            .font_family("JetBrains Mono")
                                                                            .child(compact_elapsed),
                                                                    )
                                                                    .child(
                                                                        Icon::new(if is_expanded {
                                                                            IconName::ChevronUp
                                                                        } else {
                                                                            IconName::ChevronDown
                                                                        })
                                                                        .size_3()
                                                                        .text_color(cx.theme().muted_foreground.opacity(0.5)),
                                                                    ),
                                                            )
                                                            .when(is_expanded && !summary.is_empty(), |el| {
                                                                el.child(
                                                                    div()
                                                                        .w_full()
                                                                        .px_3()
                                                                        .py_2()
                                                                        .border_t_1()
                                                                        .border_color(cx.theme().border)
                                                                        .text_xs()
                                                                        .font_family("JetBrains Mono")
                                                                        .text_color(cx.theme().muted_foreground)
                                                                        .whitespace_normal()
                                                                        .child(summary),
                                                                )
                                                            }),
                                                    )
                                                    .into_any_element()
                                            }

                                            DisplayItem::SystemPrompt {
                                                content,
                                                is_expanded,
                                                is_outdated,
                                            } => {
                                                let content = content.clone();
                                                let is_expanded = *is_expanded;
                                                let is_outdated = *is_outdated;
                                                let accent = if is_outdated {
                                                    cx.theme().warning
                                                } else {
                                                    cx.theme().muted_foreground
                                                };

                                                div()
                                                    .relative()
                                                    .w_full()
                                                    .min_w_0()
                                                    .px_3()
                                                    .py_1()
                                                    .child(
                                                        canvas(
                                                            move |bounds, _, cx| {
                                                                panel.update(cx, |panel, cx| {
                                                                    let measured = bounds.size.height;
                                                                    if panel.display_item_heights.get(&ix).copied() != Some(measured) {
                                                                        panel.display_item_heights.insert(ix, measured);
                                                                        cx.notify();
                                                                    }
                                                                });
                                                            },
                                                            |_, _, _, _| {},
                                                        )
                                                        .absolute()
                                                        .inset_0(),
                                                    )
                                                    .child(
                                                        v_flex()
                                                            .w_full()
                                                            .rounded(px(6.0))
                                                            .border_1()
                                                            .border_color(accent.opacity(0.25))
                                                            .bg(cx.theme().secondary)
                                                            .overflow_hidden()
                                                            .child(
                                                                h_flex()
                                                                    .id(("system-prompt-header", ix))
                                                                    .w_full()
                                                                    .px_3()
                                                                    .py(px(6.0))
                                                                    .gap_2()
                                                                    .cursor_pointer()
                                                                    .on_click(cx.listener(
                                                                        move |this, _, _, cx| {
                                                                            if let Some(DisplayItem::SystemPrompt { is_expanded, .. }) =
                                                                                this.display_items.get_mut(ix)
                                                                            {
                                                                                *is_expanded = !*is_expanded;
                                                                                this.display_item_heights.remove(&ix);
                                                                            }
                                                                            cx.notify();
                                                                        },
                                                                    ))
                                                                    .child(
                                                                        Icon::new(IconName::Settings)
                                                                            .size_3()
                                                                            .text_color(accent),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .flex_1()
                                                                            .text_xs()
                                                                            .font_semibold()
                                                                            .text_color(cx.theme().muted_foreground)
                                                                            .child("System Prompt"),
                                                                    )
                                                                    .when(is_outdated, |el| {
                                                                        el.child(
                                                                            Button::new("system-prompt-update")
                                                                                .xsmall()
                                                                                .ghost()
                                                                                .label("Update")
                                                                                .tooltip("Replace with current default system prompt")
                                                                                .on_click(cx.listener(|this, _, _, cx| {
                                                                                    this.apply_system_prompt_update(cx);
                                                                                })),
                                                                        )
                                                                    })
                                                                    .child(
                                                                        Icon::new(if is_expanded {
                                                                            IconName::ChevronUp
                                                                        } else {
                                                                            IconName::ChevronDown
                                                                        })
                                                                        .size_3()
                                                                        .text_color(cx.theme().muted_foreground.opacity(0.6)),
                                                                    ),
                                                            )
                                                            .when(is_expanded && !content.is_empty(), |el| {
                                                                el.child(
                                                                    div()
                                                                        .w_full()
                                                                        .px_3()
                                                                        .py_2()
                                                                        .border_t_1()
                                                                        .border_color(cx.theme().border)
                                                                        .text_xs()
                                                                        .font_family("JetBrains Mono")
                                                                        .text_color(cx.theme().muted_foreground)
                                                                        .whitespace_normal()
                                                                        .child(content),
                                                                )
                                                            }),
                                                    )
                                                    .into_any_element()
                                            }

                                            DisplayItem::ThinkingBlock {
                                                content,
                                                is_expanded,
                                                is_done,
                                                started_at_ms,
                                                finished_at_ms,
                                            } => {
                                                let content = content.clone();
                                                let is_expanded = *is_expanded;
                                                let is_done = *is_done;
                                                let think_elapsed = Self::format_elapsed(*started_at_ms, *finished_at_ms, render_now_ms);
                                                let accent = cx.theme().info;
                                                let status_icon = if is_done {
                                                    IconName::Brain
                                                } else {
                                                    IconName::Loader
                                                };
                                                let header_label = if is_done {
                                                    "Thought"
                                                } else {
                                                    "Thinking…"
                                                };

                                                div()
                                                    .relative()
                                                    .w_full()
                                                    .min_w_0()
                                                    .px_3()
                                                    .py_1()
                                                    .child(
                                                        canvas(
                                                            move |bounds, _, cx| {
                                                                panel.update(cx, |panel, cx| {
                                                                    let measured =
                                                                        bounds.size.height;
                                                                    if panel
                                                                        .display_item_heights
                                                                        .get(&ix)
                                                                        .copied()
                                                                        != Some(measured)
                                                                    {
                                                                        panel
                                                                            .display_item_heights
                                                                            .insert(ix, measured);
                                                                        cx.notify();
                                                                    }
                                                                });
                                                            },
                                                            |_, _, _, _| {},
                                                        )
                                                        .absolute()
                                                        .inset_0(),
                                                    )
                                                    .child(
                                                        v_flex()
                                                            .w_full()
                                                            .rounded(px(6.0))
                                                            .border_1()
                                                            .border_color(accent.opacity(0.25))
                                                            .bg(cx.theme().secondary)
                                                            .overflow_hidden()
                                                            .child(
                                                                h_flex()
                                                                    .id(("thinking-header", ix))
                                                                    .w_full()
                                                                    .px_3()
                                                                    .py(px(6.0))
                                                                    .gap_2()
                                                                    .cursor_pointer()
                                                                    .on_click(cx.listener(
                                                                        move |this, _, _, cx| {
                                                                            if let Some(
                                                                                DisplayItem::ThinkingBlock {
                                                                                    is_expanded,
                                                                                    ..
                                                                                },
                                                                            ) = this
                                                                                .display_items
                                                                                .get_mut(ix)
                                                                            {
                                                                                *is_expanded =
                                                                                    !*is_expanded;
                                                                                this.display_item_heights
                                                                                    .remove(&ix);
                                                                            }
                                                                            cx.notify();
                                                                        },
                                                                    ))
                                                                    .child(
                                                                        Icon::new(IconName::Brain)
                                                                            .size_3()
                                                                            .text_color(accent),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .flex_1()
                                                                            .text_xs()
                                                                            .text_color(
                                                                                cx.theme()
                                                                                    .muted_foreground,
                                                                            )
                                                                            .child(header_label),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(accent.opacity(0.7))
                                                                            .font_family("JetBrains Mono")
                                                                            .child(think_elapsed),
                                                                    )
                                                                    .child(
                                                                        Icon::new(status_icon)
                                                                            .size_3()
                                                                            .text_color(accent),
                                                                    )
                                                                    .child(
                                                                        Icon::new(if is_expanded {
                                                                            IconName::ChevronUp
                                                                        } else {
                                                                            IconName::ChevronDown
                                                                        })
                                                                        .size_3()
                                                                        .text_color(
                                                                            cx.theme()
                                                                                .muted_foreground
                                                                                .opacity(0.6),
                                                                        ),
                                                                    ),
                                                            )
                                                            .when(is_expanded && !content.is_empty(), |el| {
                                                                el.child(
                                                                    div()
                                                                        .w_full()
                                                                        .px_3()
                                                                        .py_2()
                                                                        .border_t_1()
                                                                        .border_color(cx.theme().border)
                                                                        .text_xs()
                                                                        .font_family("JetBrains Mono")
                                                                        .text_color(
                                                                            cx.theme().muted_foreground,
                                                                        )
                                                                        .whitespace_normal()
                                                                        .child(content),
                                                                )
                                                            }),
                                                    )
                                                    .into_any_element()
                                            }

                                            DisplayItem::UserMessage {
                                                content,
                                                message_index,
                                            }
                                            | DisplayItem::AssistantMessage {
                                                content,
                                                message_index,
                                                ..
                                            } => {
                                                let is_user =
                                                    matches!(item, DisplayItem::UserMessage { .. });
                                                let is_streaming = matches!(
                                                    item,
                                                    DisplayItem::AssistantMessage {
                                                        is_streaming: true,
                                                        ..
                                                    }
                                                );
                                                let content = content.clone();
                                                let copy_content = content.clone();
                                                let message_index = *message_index;
                                                let hover_group =
                                                    format!("agent-chat-msg-hover-{ix}");
                                                let is_confirming_rollback =
                                                    this.pending_rollback_confirm_ix == Some(ix);

                                                div()
                                                    .relative()
                                                    .group(hover_group.clone())
                                                    .w_full()
                                                    .min_w_0()
                                                    .px_3()
                                                    .py_1()
                                                    .child(
                                                        canvas(
                                                            move |bounds, _, cx| {
                                                                panel.update(cx, |panel, cx| {
                                                                    let measured =
                                                                        bounds.size.height;
                                                                    if panel
                                                                        .display_item_heights
                                                                        .get(&ix)
                                                                        .copied()
                                                                        != Some(measured)
                                                                    {
                                                                        panel
                                                                            .display_item_heights
                                                                            .insert(ix, measured);
                                                                        cx.notify();
                                                                    }
                                                                });
                                                            },
                                                            |_, _, _, _| {},
                                                        )
                                                        .absolute()
                                                        .inset_0(),
                                                    )
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
                                                                        cx.theme()
                                                                            .primary
                                                                            .opacity(0.16)
                                                                    } else {
                                                                        cx.theme().secondary
                                                                    })
                                                                    .child(
                                                                        div()
                                                                            .text_xs()
                                                                            .font_semibold()
                                                                            .text_color(
                                                                                cx.theme()
                                                                                    .muted_foreground,
                                                                            )
                                                                            .child(if is_user {
                                                                                "You"
                                                                            } else {
                                                                                "Agent"
                                                                            }),
                                                                    )
                                                                    .child(if is_user || is_streaming {
                                                                        div()
                                                                            .w_full()
                                                                            .min_w_0()
                                                                            .whitespace_normal()
                                                                            .text_sm()
                                                                            .text_color(
                                                                                cx.theme().foreground,
                                                                            )
                                                                            .child(content)
                                                                            .into_any_element()
                                                                    } else {
                                                                        TextView::markdown_with_code_font(
                                                                            ("agent-chat-md", ix),
                                                                            content,
                                                                            "JetBrains Mono",
                                                                            window,
                                                                            cx,
                                                                        )
                                                                        .debounce_ms(0)
                                                                        .selectable()
                                                                        .into_any_element()
                                                                    }),
                                                            )
                                                            .id(("agent-chat-message", ix)),
                                                    )
                                                    .child(
                                                        h_flex()
                                                            .absolute()
                                                            .w_full()
                                                            .bottom(px(-8.0))
                                                            .px_6()
                                                            .justify_start()
                                                            .when(is_user, |this| {
                                                                this.justify_end()
                                                            })
                                                            .invisible()
                                                            .group_hover(
                                                                hover_group,
                                                                |this| this.visible(),
                                                            )
                                                            .child(
                                                                h_flex()
                                                                    .gap_1()
                                                                    .p_1()
                                                                    .rounded(px(8.0))
                                                                    .bg(cx.theme().background)
                                                                    .border_1()
                                                                    .border_color(
                                                                        cx.theme().border,
                                                                    )
                                                                    .when(
                                                                        !is_confirming_rollback,
                                                                        |el| {
                                                                            el.child(
                                                                                Button::new((
                                                                                    "agent-chat-rollback",
                                                                                    ix,
                                                                                ))
                                                                                .xsmall()
                                                                                .ghost()
                                                                                .icon(
                                                                                    IconName::Undo,
                                                                                )
                                                                                .tooltip(
                                                                                    "Rollback to this message",
                                                                                )
                                                                                .disabled(
                                                                                    this.is_request_in_flight,
                                                                                )
                                                                                .on_click(
                                                                                    cx.listener(
                                                                                        move |this,
                                                                                              _,
                                                                                              _,
                                                                                              cx| {
                                                                                            this.request_rollback_confirmation(
                                                                                                ix,
                                                                                                cx,
                                                                                            );
                                                                                        },
                                                                                    ),
                                                                                ),
                                                                            )
                                                                        },
                                                                    )
                                                                    .when(
                                                                        is_confirming_rollback,
                                                                        |el| {
                                                                            el.child(
                                                                                Button::new((
                                                                                    "agent-chat-rollback-confirm",
                                                                                    ix,
                                                                                ))
                                                                                .xsmall()
                                                                                .primary()
                                                                                .icon(
                                                                                    IconName::Check,
                                                                                )
                                                                                .tooltip(
                                                                                    "Confirm rollback",
                                                                                )
                                                                                .disabled(
                                                                                    this.is_request_in_flight,
                                                                                )
                                                                                .on_click(
                                                                                    cx.listener(
                                                                                        move |this,
                                                                                              _,
                                                                                              _,
                                                                                              cx| {
                                                                                            this.rollback_chat_to_message(
                                                                                                ix,
                                                                                                message_index,
                                                                                                cx,
                                                                                            );
                                                                                        },
                                                                                    ),
                                                                                ),
                                                                            )
                                                                            .child(
                                                                                Button::new((
                                                                                    "agent-chat-rollback-cancel",
                                                                                    ix,
                                                                                ))
                                                                                .xsmall()
                                                                                .ghost()
                                                                                .icon(
                                                                                    IconName::Close,
                                                                                )
                                                                                .tooltip(
                                                                                    "Cancel rollback",
                                                                                )
                                                                                .on_click(
                                                                                    cx.listener(
                                                                                        |this,
                                                                                         _,
                                                                                         _,
                                                                                         cx| {
                                                                                            this.cancel_rollback_confirmation(
                                                                                                cx,
                                                                                            );
                                                                                        },
                                                                                    ),
                                                                                ),
                                                                            )
                                                                        },
                                                                    )
                                                                    .child(
                                                                        Button::new((
                                                                            "agent-chat-fork",
                                                                            ix,
                                                                        ))
                                                                        .xsmall()
                                                                        .ghost()
                                                                        .icon(IconName::GitFork)
                                                                        .tooltip(
                                                                            "Fork conversation from here",
                                                                        )
                                                                        .disabled(
                                                                            this.is_request_in_flight,
                                                                        )
                                                                        .on_click(cx.listener(
                                                                            move |this, _, _, cx| {
                                                                                this.fork_chat_here(
                                                                                    ix,
                                                                                    message_index,
                                                                                    cx,
                                                                                );
                                                                            },
                                                                        )),
                                                                    )
                                                                    // Copy message content to clipboard
                                                                    .child({
                                                                        Button::new((
                                                                            "agent-chat-copy",
                                                                            ix,
                                                                        ))
                                                                        .xsmall()
                                                                        .ghost()
                                                                        .icon(IconName::Copy)
                                                                        .tooltip("Copy message")
                                                                        .on_click(cx.listener(
                                                                            move |_, _, _, cx| {
                                                                                cx.write_to_clipboard(
                                                                                    gpui::ClipboardItem::new_string(copy_content.clone())
                                                                                );
                                                                            },
                                                                        ))
                                                                    })
                                                                    // Edit: user messages only — put text back in input
                                                                    .when(is_user && !is_confirming_rollback, |el| {
                                                                        el.child(
                                                                            Button::new((
                                                                                "agent-chat-edit",
                                                                                ix,
                                                                            ))
                                                                            .xsmall()
                                                                            .ghost()
                                                                            .icon(IconName::EditPencil)
                                                                            .tooltip("Edit message")
                                                                            .disabled(this.is_request_in_flight)
                                                                            .on_click(cx.listener(
                                                                                move |this, _, window, cx| {
                                                                                    this.edit_user_message(
                                                                                        ix,
                                                                                        message_index,
                                                                                        window,
                                                                                        cx,
                                                                                    );
                                                                                },
                                                                            ))
                                                                        )
                                                                    })
                                                                    // Regenerate: last assistant message only
                                                                    .when(!is_user && ix + 1 == display_count && !is_confirming_rollback, |el| {
                                                                        el.child(
                                                                            Button::new((
                                                                                "agent-chat-regen",
                                                                                ix,
                                                                            ))
                                                                            .xsmall()
                                                                            .ghost()
                                                                            .icon(IconName::Refresh)
                                                                            .tooltip("Regenerate response")
                                                                            .disabled(this.is_request_in_flight)
                                                                            .on_click(cx.listener(
                                                                                |this, _, _, cx| {
                                                                                    this.regenerate_response(cx);
                                                                                },
                                                                            ))
                                                                        )
                                                                    }),
                                                            ),
                                                    )
                                                    .into_any_element()
                                            }
                                        }
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
                    )
                    // Viewport-height measurement canvas — runs after layout,
                    // stores the value so distance_from_bottom() works correctly.
                    .child(
                        canvas(
                            {
                                let panel = cx.entity().clone();
                                move |bounds, _, cx| {
                                    let h = bounds.size.height;
                                    panel.update(cx, |p, cx| {
                                        if p.chat_viewport_height != h {
                                            p.chat_viewport_height = h;
                                            cx.notify();
                                        }
                                    });
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    // "Jump to bottom" floating button — appears when the user has
                    // scrolled up during streaming.
                    .when(show_jump_button, |el| {
                        el.child(
                            div()
                                .absolute()
                                .bottom(px(16.0))
                                .right(px(28.0))
                                .child(
                                    Button::new("agent-chat-jump-bottom")
                                        .icon(IconName::ArrowDown)
                                        .xsmall()
                                        .primary()
                                        .tooltip("Jump to bottom (re-enable auto-scroll)")
                                        .on_click(cx.listener(|this, _, _, _cx| {
                                            this.jump_to_bottom();
                                        })),
                                ),
                        )
                    }),
            )
            .child(
                v_flex()
                    .w_full()
                    .gap(px(6.0))
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    .child(
                        // Prompt input row
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap_2()
                            .items_end()
                            .child(TextInput::new(&self.prompt_input).flex_1().min_w_0())
                            .when(self.is_request_in_flight, |this| {
                                this.child(
                                    h_flex()
                                        .gap_1()
                                        .items_center()
                                        .child(Spinner::new().with_size(Size::Small))
                                        .child(
                                            Button::new("agent-chat-stop")
                                                .icon(IconName::Square)
                                                .xsmall()
                                                .ghost()
                                                .tooltip("Stop generation")
                                                .on_click(cx.listener(|this, _, _, _cx| {
                                                    if let Some(tx) = this.cancel_tx.take() {
                                                        let _ = tx.try_send(());
                                                    }
                                                })),
                                        ),
                                )
                            })
                            .child(
                                Button::new("agent-chat-send")
                                    .icon(IconName::Send)
                                    .label("Send")
                                    .primary()
                                    .tooltip("Send message (Enter)")
                                    .disabled(
                                        self.is_request_in_flight
                                            || self
                                                .prompt_input
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
                        // Utility toolbar
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap_1()
                            .items_center()
                            .child(chat_history_popover)
                            .child(
                                Button::new("agent-chat-new-chat")
                                    .icon(IconName::Plus)
                                    .xsmall()
                                    .ghost()
                                    .tooltip("New chat")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.start_new_chat(cx);
                                    })),
                            )
                            .child(
                                Button::new("agent-chat-import")
                                    .icon(IconName::Upload)
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Import chat from JSON")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.import_chat(cx);
                                    })),
                            )
                            .child(
                                Button::new("agent-chat-export")
                                    .icon(IconName::Download)
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Export this chat to JSON")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.export_current_chat();
                                    })),
                            )
                            .child(
                                Button::new("agent-chat-export-all")
                                    .icon(IconName::FolderOpen)
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Export all chats")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.export_all_chats();
                                    })),
                            ),
                    ),
            )
    }
}
