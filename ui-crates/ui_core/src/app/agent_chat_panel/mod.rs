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
use agent_chat_core::ProviderRegistry;
use agent_chat_tools::ToolRegistry;
use agent_provider_anthropic::AnthropicProvider;
use agent_provider_demo_random::DemoRandomProvider;
use agent_provider_gemini::GeminiProvider;
use agent_provider_github_copilot::GithubCopilotProvider;
use agent_provider_groq::GroqProvider;
use agent_provider_mistral::MistralProvider;
use agent_provider_openai::OpenAiProvider;
use agent_provider_openrouter::OpenRouterProvider;
use agent_provider_together::TogetherProvider;
use gpui::{prelude::FluentBuilder as _, *};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
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
    v_flex, v_virtual_list, ActiveTheme as _, Disableable, IconName, Sizable, Size, StyledExt,
    VirtualListScrollHandle,
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
    pub(crate) pending_rollback_confirm_ix: Option<usize>,
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) _subscriptions: Vec<Subscription>,
}

impl AgentChatPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut provider_catalog = Self::default_provider_catalog();
        let custom_providers_list =
            custom_providers::load_custom_providers(&Self::custom_provider_config_dir());
        let custom_provider_ids = Rc::new(RefCell::new(
            custom_providers_list
                .iter()
                .map(|provider| provider.id.clone())
                .collect::<HashSet<_>>(),
        ));
        provider_catalog.extend(
            custom_providers_list
                .iter()
                .map(Self::custom_provider_to_definition),
        );

        let mut provider_registry = ProviderRegistry::new();
        provider_registry.register(Arc::new(GithubCopilotProvider::new()));
        provider_registry.register(Arc::new(DemoRandomProvider::new()));
        provider_registry.register(Arc::new(OpenAiProvider::new()));
        provider_registry.register(Arc::new(AnthropicProvider::new()));
        provider_registry.register(Arc::new(OpenRouterProvider::new()));
        provider_registry.register(Arc::new(GroqProvider::new()));
        provider_registry.register(Arc::new(TogetherProvider::new()));
        provider_registry.register(Arc::new(MistralProvider::new()));
        provider_registry.register(Arc::new(GeminiProvider::new()));
        let wip_providers = Self::wip_providers_from_catalog(&provider_catalog, &provider_registry);

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
            .with_icon_getter(|_| IconName::Brain)
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
            pending_rollback_confirm_ix: None,
            messages: vec![ChatMessage {
                role: "system",
                content: "Agent Chat is ready. Choose provider/model and ask anything about your project.".to_string(),
            }],
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
        let message_count = self.messages.len();
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
                .chain(std::iter::once(size(px(0.0), px(120.0))))
                .collect::<Vec<_>>(),
        );

        let provider_popover =
            Popover::<SearchableList<ProviderDefinition>>::new("agent-chat-provider-popover")
                .anchor(Corner::TopLeft)
                .trigger(
                    Button::new("agent-chat-provider-trigger")
                        .xsmall()
                        .ghost()
                        .justify_start()
                        .tooltip("Select provider")
                        .label(
                            provider
                                .map(|p| format!("Provider: {} ({})", p.label, p.id))
                                .unwrap_or_else(|| "No provider".to_string()),
                        )
                        .dropdown_caret(true),
                )
                .content(move |_window, _cx| provider_list.clone());

        let model_popover =
            Popover::<SearchableList<ModelDefinition>>::new("agent-chat-model-popover")
                .anchor(Corner::TopLeft)
                .trigger(
                    Button::new("agent-chat-model-trigger")
                        .xsmall()
                        .ghost()
                        .justify_start()
                        .tooltip("Select model")
                        .label(
                            model
                                .map(|m| format!("Model: {} ({})", m.label, m.id))
                                .unwrap_or_else(|| "No model".to_string()),
                        )
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
                        .tooltip("Select chat")
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
                            .gap_2()
                            .child(provider_popover)
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
                    .relative()
                    .flex_1()
                    .child(
                        v_virtual_list(
                            cx.entity().clone(),
                            "agent-chat-messages-virtual-list",
                            message_item_sizes,
                            move |this,
                                  range: std::ops::Range<usize>,
                                  window,
                                  cx: &mut Context<Self>| {
                                range
                                    .map(|ix| {
                                        if ix == message_count {
                                            return div().h(px(120.0)).into_any_element();
                                        }

                                        let Some(message) = this.messages.get(ix) else {
                                            return div().h(px(52.0)).into_any_element();
                                        };

                                        let is_user = message.role == "user";
                                        let is_streaming_assistant =
                                            !is_user && this.streaming_message_ix == Some(ix);
                                        let is_actionable_message = message.role != "system";
                                        let hover_group = format!("agent-chat-msg-hover-{ix}");
                                        let is_confirming_rollback =
                                            this.pending_rollback_confirm_ix == Some(ix);
                                        let panel = cx.entity().clone();
                                        let content = message.content.clone();

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
                                                            let measured = bounds.size.height;
                                                            if panel
                                                                .message_row_heights
                                                                .get(&ix)
                                                                .copied()
                                                                != Some(measured)
                                                            {
                                                                panel
                                                                    .message_row_heights
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
                                                                cx.theme().primary.opacity(0.16)
                                                            } else {
                                                                cx.theme().secondary
                                                            })
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .font_semibold()
                                                                    .text_color(
                                                                        cx.theme().muted_foreground,
                                                                    )
                                                                    .child(if is_user {
                                                                        "You"
                                                                    } else {
                                                                        "Agent"
                                                                    }),
                                                            )
                                                            .child(if is_user {
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
                                                            } else if is_streaming_assistant {
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
                                            .when(is_actionable_message, |el| {
                                                el.child(
                                                    h_flex()
                                                        .absolute()
                                                        .w_full()
                                                        .bottom(px(-8.0))
                                                        .px_6()
                                                        .justify_start()
                                                        .when(is_user, |this| this.justify_end())
                                                        .invisible()
                                                        .group_hover(hover_group, |this| this.visible())
                                                        .child(
                                                            h_flex()
                                                                .gap_1()
                                                                .p_1()
                                                                .rounded(px(8.0))
                                                                .bg(cx.theme().background)
                                                                .border_1()
                                                                .border_color(cx.theme().border)
                                                                .when(!is_confirming_rollback, |el| {
                                                                    el.child(
                                                                        Button::new((
                                                                            "agent-chat-rollback",
                                                                            ix,
                                                                        ))
                                                                        .xsmall()
                                                                        .ghost()
                                                                        .icon(IconName::Undo)
                                                                        .tooltip("Rollback to this message")
                                                                        .disabled(
                                                                            this.is_request_in_flight,
                                                                        )
                                                                        .on_click(cx.listener(
                                                                            move |this, _, _, cx| {
                                                                                this.request_rollback_confirmation(
                                                                                    ix, cx,
                                                                                );
                                                                            },
                                                                        )),
                                                                    )
                                                                })
                                                                .when(is_confirming_rollback, |el| {
                                                                    el.child(
                                                                        Button::new((
                                                                            "agent-chat-rollback-confirm",
                                                                            ix,
                                                                        ))
                                                                        .xsmall()
                                                                        .primary()
                                                                        .icon(IconName::Check)
                                                                        .tooltip("Confirm rollback")
                                                                        .disabled(
                                                                            this.is_request_in_flight,
                                                                        )
                                                                        .on_click(cx.listener(
                                                                            move |this, _, _, cx| {
                                                                                this.rollback_chat_to_message(
                                                                                    ix, cx,
                                                                                );
                                                                            },
                                                                        )),
                                                                    )
                                                                    .child(
                                                                        Button::new((
                                                                            "agent-chat-rollback-cancel",
                                                                            ix,
                                                                        ))
                                                                        .xsmall()
                                                                        .ghost()
                                                                        .icon(IconName::Close)
                                                                        .tooltip("Cancel rollback")
                                                                        .on_click(cx.listener(
                                                                            |this, _, _, cx| {
                                                                                this.cancel_rollback_confirmation(
                                                                                    cx,
                                                                                );
                                                                            },
                                                                        )),
                                                                    )
                                                                })
                                                                .child(
                                                                    Button::new((
                                                                        "agent-chat-fork",
                                                                        ix,
                                                                    ))
                                                                    .xsmall()
                                                                    .ghost()
                                                                    .icon(IconName::GitFork)
                                                                    .tooltip("Fork conversation from here")
                                                                    .disabled(
                                                                        this.is_request_in_flight,
                                                                    )
                                                                    .on_click(cx.listener(
                                                                        move |this, _, _, cx| {
                                                                            this.fork_chat_here(
                                                                                ix, cx,
                                                                            );
                                                                        },
                                                                    )),
                                                                ),
                                                        ),
                                                )
                                            })
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
                                    .tooltip("Start new chat")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.start_new_chat(cx);
                                    })),
                            )
                            .child(
                                Button::new("agent-chat-import")
                                    .icon(IconName::Upload)
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Import chat")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.import_chat(cx);
                                    })),
                            )
                            .child(
                                Button::new("agent-chat-export")
                                    .icon(IconName::Download)
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Export current chat")
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
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .child(TextInput::new(&self.prompt_input).flex_1().min_w_0())
                            .when(self.is_request_in_flight, |this| {
                                this.child(
                                    h_flex()
                                        .gap_1()
                                        .items_center()
                                        .child(Spinner::new().with_size(Size::Small))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("Thinking..."),
                                        ),
                                )
                            })
                            .child(
                                Button::new("agent-chat-send")
                                    .icon(IconName::Send)
                                    .label("Send")
                                    .tooltip("Send message")
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
                    ),
            )
    }
}
