mod chat_history;
mod chat_storage;
mod custom_provider_wizard;
mod prompt_ranking;
mod provider_catalog;
mod provider_selection;
mod streaming;
pub mod types;

pub use types::*;

use crate::custom_providers::{self, CustomProvider};
use agent_chat_core::{ChatMessage, ChatProvider, ChatRole, ProviderCrate, ProviderEntry, ProviderRegistry};
use agent_chat_tools::ToolRegistry;
use agent_provider_anthropic::AnthropicProviderCrate;
use agent_provider_aws_bedrock::AwsBedrockProviderCrate;
use agent_provider_demo_random::DemoRandomProviderCrate;
use agent_provider_docker_model_runner::DockerModelRunnerProviderCrate;
use agent_provider_gemini::GeminiProviderCrate;
use agent_provider_github_copilot::GithubCopilotProviderCrate;
use agent_provider_openai::OpenAiProviderCrate;
use agent_provider_vertex_ai::VertexAiProviderCrate;
use gpui::{prelude::FluentBuilder as _, *};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    path::PathBuf,
    rc::Rc,
    sync::{Arc, RwLock},
};
use ui::{
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockItem, Panel, PanelEvent, TabPanel},
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubagentCompletionMode {
    Auto,
    Manual,
}

pub struct AgentChatPanel {
    pub(crate) dock_area: Entity<DockArea>,
    pub(crate) center_tabs: Entity<TabPanel>,
    pub(crate) parent_window_handle: AnyWindowHandle,
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
    pub(crate) pending_custom_provider: Option<PendingCustomProvider>,
    pub(crate) pending_custom_provider_step: Option<AddProviderPromptStep>,
    pub(crate) provider_registry: ProviderRegistry,
    pub(crate) provider_states: HashMap<String, ProviderState>,
    pub(crate) provider_states_shared: Rc<RefCell<HashMap<String, ProviderState>>>,
    pub(crate) provider_entries: HashMap<String, ProviderEntry>,
    pub(crate) crate_instances: Vec<Box<dyn ProviderCrate>>,
    pub(crate) configuring_provider: Option<String>,
    pub(crate) configuring_field_index: usize,
    pub(crate) config_values: HashMap<String, String>,
    pub(crate) config_error: Option<String>,
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
    /// FIFO queue of completed subagents waiting for main-agent processing.
    pub(crate) pending_subagent_events: VecDeque<serde_json::Value>,
    /// True while the main agent is actively processing one queued subagent event.
    pub(crate) is_processing_subagent_event: bool,
    /// Subagent ID currently under main-agent processing lock.
    pub(crate) processing_subagent_id: Option<String>,
    /// Whether subagent completions are auto-processed when chat is idle.
    pub(crate) subagent_completion_mode: SubagentCompletionMode,
    /// Whether the chat should automatically scroll to new content.
    /// Disabled when the user explicitly scrolls up; re-enabled when they scroll
    /// back near the bottom or click the "jump to bottom" button.
    pub(crate) auto_scroll: bool,
    /// Height of the message-list viewport in pixels — measured each render frame.
    pub(crate) chat_viewport_height: Pixels,
    pub(crate) _subscriptions: Vec<Subscription>,
}

impl AgentChatPanel {
    pub fn new(
        dock_area: Entity<DockArea>,
        center_tabs: Entity<TabPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // --- Provider registry: all providers registered via ProviderCrate ---
        let mut provider_registry = ProviderRegistry::new();

        let crate_instances: Vec<Box<dyn ProviderCrate>> = vec![
            Box::new(OpenAiProviderCrate),
            Box::new(AnthropicProviderCrate),
            Box::new(GeminiProviderCrate),
            Box::new(GithubCopilotProviderCrate),
            Box::new(AwsBedrockProviderCrate),
            Box::new(VertexAiProviderCrate),
            Box::new(DemoRandomProviderCrate),
            Box::new(DockerModelRunnerProviderCrate),
        ];

        let mut provider_states: HashMap<String, ProviderState> = HashMap::new();
        let provider_states_shared: Rc<RefCell<HashMap<String, ProviderState>>> = Rc::new(RefCell::new(HashMap::new()));
        let mut provider_entries: HashMap<String, ProviderEntry> = HashMap::new();
        let disabled_providers = ["aws_bedrock", "vertex_ai"];

        for crate_impl in &crate_instances {
            let entries = crate_impl.entries();
            for entry in entries {
                let needs_config = entry.config_fields.iter().any(|f| f.required);
                let config = agent_chat_core::ProviderConfig {
                    values: std::collections::HashMap::new(),
                };
                if let Ok(provider) = crate_impl.create(entry.id, config) {
                    let state = if disabled_providers.contains(&entry.id) {
                        ProviderState::Disabled
                    } else if needs_config {
                        ProviderState::Unconfigured
                    } else {
                        ProviderState::Ready
                    };
                    let id = entry.id.to_string();
                    provider_entries.insert(id.clone(), entry);
                    provider_states.insert(id.clone(), state.clone());
                    provider_states_shared.borrow_mut().insert(id.clone(), state);
                    provider_registry.register(Arc::from(provider));
                }
            }
        }

        // --- Custom providers added by the user ---
        let custom_providers_list =
            custom_providers::load_custom_providers(&Self::custom_provider_config_dir());

        // --- Build provider catalog from registry, sorted by state ---
        let mut provider_catalog: Vec<ProviderDefinition> = Vec::new();
        for (id, provider) in provider_registry.all() {
            provider_catalog.push(ProviderDefinition {
                id: Box::leak(id.clone().into_boxed_str()),
                label: Box::leak(provider.display_name().to_string().into_boxed_str()),
                kind: ProviderKind::Cloud,
                endpoint: Box::leak(String::new().into_boxed_str()),
                models: Arc::new(vec![]),
            });
        }

        // Sort: Ready first (alpha), then Unconfigured (alpha), then Disabled (alpha)
        let state_order = |id: &str| -> u8 {
            match provider_states.get(id) {
                Some(ProviderState::Ready) => 0,
                Some(ProviderState::Unconfigured) => 1,
                Some(ProviderState::Disabled) | None => 2,
            }
        };
        provider_catalog.sort_by(|a, b| {
            let ta = state_order(a.id);
            let tb = state_order(b.id);
            ta.cmp(&tb).then_with(|| a.label.cmp(b.label))
        });

        // Append custom provider definitions for display
        provider_catalog.extend(
            custom_providers_list
                .iter()
                .map(Self::custom_provider_to_definition),
        );

        let plugin_bridge = plugin_manager::global()
            .map(|manager_lock| Arc::new(RwLock::new(manager_lock.read().build_tool_bridge())));

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


        let states_shared = provider_states_shared.clone();
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
            .with_item_state(move |p: &ProviderDefinition| {
                let map = states_shared.borrow();
                match map.get(p.id) {
                    Some(ProviderState::Ready) => SearchableListItemState::Enabled,
                    Some(ProviderState::Unconfigured) => SearchableListItemState::Locked,
                    Some(ProviderState::Disabled) | None => SearchableListItemState::Disabled,
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
            dock_area,
            center_tabs,
            parent_window_handle: window.window_handle(),
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
            pending_custom_provider: None,
            pending_custom_provider_step: None,
            provider_registry,
            provider_states,
            provider_states_shared,
            provider_entries,
            crate_instances,
            configuring_provider: None,
            configuring_field_index: 0,
            config_values: HashMap::new(),
            config_error: None,
            tool_registry: agent_chat_tools::build_default_registry(),
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
            pending_subagent_events: VecDeque::new(),
            is_processing_subagent_event: false,
            processing_subagent_id: None,
            subagent_completion_mode: SubagentCompletionMode::Auto,
            auto_scroll: true,
            chat_viewport_height: px(0.0),
            _subscriptions: subscriptions,
        };

        this.bootstrap_chat_storage(cx);
        // Kick off a background model fetch for whichever provider is shown first.
        let initial_ix = this.active_provider_ix;
        this.fetch_models_in_background(initial_ix, cx);
        this
    }

    pub(crate) fn refresh_open_editor_snapshot(&self, cx: &App) {
        let mut snapshot = Vec::new();
        let mut global_index = 0usize;

        fn visit_item(
            item: &DockItem,
            snapshot: &mut Vec<crate::app::open_editors::OpenEditorInfo>,
            global_index: &mut usize,
            cx: &App,
        ) {
            match item {
                DockItem::Split { items, .. } => {
                    for child in items {
                        visit_item(child, snapshot, global_index, cx);
                    }
                }
                DockItem::Tabs { view, .. } => {
                    let active_local = view.read(cx).active_tab_index();
                    let panels = view.read(cx).all_panels();
                    for (local_ix, panel) in panels.into_iter().enumerate() {
                        let panel_name = panel.panel_name(cx).to_string();
                        let tab_name = panel
                            .tab_name(cx)
                            .map(|name| name.to_string())
                            .unwrap_or_else(|| panel_name.clone());
                        let file_path = panel.panel_file_path(cx).map(|p| p.display().to_string());
                        snapshot.push(crate::app::open_editors::OpenEditorInfo {
                            index: *global_index,
                            panel_name,
                            tab_name,
                            is_active: active_local == Some(local_ix),
                            file_path,
                        });
                        *global_index += 1;
                    }
                }
                DockItem::Tiles { .. } | DockItem::Panel { .. } => {}
            }
        }

        let items = {
            let dock = self.dock_area.read(cx);
            dock.items().clone()
        };
        visit_item(&items, &mut snapshot, &mut global_index, cx);
        crate::app::open_editors::set_snapshot(snapshot);
    }

    pub(crate) fn open_path_in_default_editor(
        &self,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let center_tabs = self.center_tabs.clone();
        let project_path = engine_state::get_project_path().map(PathBuf::from);
        let update_result = cx.update_window(self.parent_window_handle, |_root, window, cx| {
            let pm_lock = plugin_manager::global()
                .ok_or_else(|| "Global plugin manager not available".to_string())?;
            let mut pm = pm_lock.write();

            pm.set_project_root(project_path);
            let panel = pm
                .create_editor_for_file(&path, window, cx)
                .map_err(|err| err.to_string())?;

            center_tabs.update(cx, |tabs, cx| {
                tabs.add_panel(panel, window, cx);
            });
            Ok::<(), String>(())
        });

        match update_result {
            Ok(Ok(())) => {
                self.refresh_open_editor_snapshot(cx);
                Ok(())
            }
            Ok(Err(err)) => Err(format!("Failed to open file {:?}: {}", path, err)),
            Err(err) => Err(format!(
                "Failed to update parent window during OpenFile: {}",
                err
            )),
        }
    }

    pub(crate) fn activate_open_editor_by_global_index(
        &self,
        target_index: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        fn find_and_activate(
            item: &DockItem,
            current_index: &mut usize,
            target_index: usize,
            window: &mut Window,
            cx: &mut App,
        ) -> bool {
            match item {
                DockItem::Split { items, .. } => {
                    for child in items {
                        if find_and_activate(child, current_index, target_index, window, cx) {
                            return true;
                        }
                    }
                    false
                }
                DockItem::Tabs { view, .. } => {
                    let panels = view.read(cx).all_panels();
                    for (local_ix, _panel) in panels.into_iter().enumerate() {
                        if *current_index == target_index {
                            view.update(cx, |tab_panel, cx| {
                                tab_panel.set_active_tab(local_ix, window, cx);
                            });
                            return true;
                        }
                        *current_index += 1;
                    }
                    false
                }
                DockItem::Tiles { .. } | DockItem::Panel { .. } => false,
            }
        }

        let dock_area = self.dock_area.clone();
        let update_result = cx.update_window(self.parent_window_handle, |_root, window, cx| {
            let items = {
                let dock = dock_area.read(cx);
                dock.items().clone()
            };
            let mut current_index = 0usize;
            find_and_activate(&items, &mut current_index, target_index, window, cx)
        });

        match update_result {
            Ok(true) => {
                self.refresh_open_editor_snapshot(cx);
                Ok(())
            }
            Ok(false) => Err(format!(
                "ActivateOpenEditor index out of range: {}",
                target_index
            )),
            Err(err) => Err(format!(
                "Failed to update parent window during ActivateOpenEditor: {}",
                err
            )),
        }
    }

    /// Get stored auth token for a provider (from the old token-based auth system).
    /// In the new config-based system, tokens are handled via provider config.
    pub(super) fn auth_token_for_provider(&self, provider_id: &str) -> Option<String> {
        self.provider_tokens.get(provider_id).cloned()
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
        self.poll_subagent_completion_events(cx);

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
        let configuring = self.configuring_provider.clone();
        let add_provider_prompt = self
            .pending_custom_provider_step
            .map(|s| Self::add_provider_prompt_title(s).to_string());
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
        let queued_subagent_count = self.pending_subagent_events.len();
        let subagent_mode_is_manual =
            self.subagent_completion_mode == SubagentCompletionMode::Manual;
        let subagent_status_text = if self.is_processing_subagent_event {
            let active_id = self
                .processing_subagent_id
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            format!(
                "Processing subagent completion ({active_id}). {} waiting.",
                queued_subagent_count
            )
        } else if queued_subagent_count > 0 {
            format!(
                "{} subagent completion(s) waiting ({})",
                queued_subagent_count,
                if subagent_mode_is_manual {
                    "manual mode"
                } else {
                    "auto mode"
                }
            )
        } else {
            "No subagent completions waiting".to_string()
        };

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
                    .when_some(configuring.clone(), |el, provider_id| {
                        let entry = self.provider_entries.get(&provider_id);
                        let fields = entry.map(|e| &e.config_fields[..]).unwrap_or(&[]);
                        let field_ix = self.configuring_field_index;
                        let field = fields.get(field_ix);
                        let has_error = self.config_error.is_some();
                        let err_text = self.config_error.clone().unwrap_or_default();

                        el.child(
                            v_flex()
                                .w_full()
                                .gap_2()
                                .p_3()
                                .rounded(px(8.0))
                                .bg(if has_error {
                                    cx.theme().colors.danger.opacity(0.08)
                                } else {
                                    cx.theme().colors.background.opacity(0.5)
                                })
                                .border_1()
                                .border_color(if has_error {
                                    cx.theme().colors.danger.opacity(0.35)
                                } else {
                                    cx.theme().colors.border.opacity(0.5)
                                })
                                .child(
                                    v_flex()
                                        .w_full()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .child(match (entry, field) {
                                                    (Some(e), Some(f)) => format!("{} — {}", e.display_name, f.label),
                                                    (Some(e), None) => e.display_name.to_string(),
                                                    (None, _) => provider_id.clone(),
                                                }),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().colors.muted_foreground)
                                                .child(match field {
                                                    Some(f) => f.description,
                                                    None => "",
                                                }),
                                        ),
                                )
                                .child(
                                    TextInput::new(&self.custom_provider_input)
                                        .w_full()
                                        .xsmall(),
                                )
                                .when(has_error, |el| {
                                    el.child(
                                        div()
                                            .w_full()
                                            .text_xs()
                                            .text_color(cx.theme().danger)
                                            .child(err_text),
                                    )
                                })
                                .child(
                                    h_flex()
                                        .w_full()
                                        .gap_2()
                                        .justify_end()
                                        .child(
                                            Button::new("provider-config-cancel")
                                                .xsmall()
                                                .ghost()
                                                .label("Cancel")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.configuring_provider = None;
                                                    this.config_error = None;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("provider-config-submit")
                                                .xsmall()
                                                .primary()
                                                .label(if has_error { "Retry" } else { "Save" })
                                                .disabled(
                                                    (self.custom_provider_input
                                                        .read(cx)
                                                        .text()
                                                        .to_string()
                                                        .trim()
                                                        .is_empty()
                                                        && field.map(|f| f.required).unwrap_or(false))
                                                        || field.is_none(),
                                                )
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    let value = this.custom_provider_input.read(cx).text().to_string();
                                                    let pid = this.configuring_provider.clone();
                                                    if let Some(ref id) = pid {
                                                        let entry = this.provider_entries.get(id);
                                                        let fields = entry.map(|e| e.config_fields.clone()).unwrap_or_default();

                                                        let field_key = fields.get(this.configuring_field_index).map(|f| f.key).unwrap_or("value").to_string();
                                                        let is_sensitive = fields.get(this.configuring_field_index).map(|f| f.sensitive).unwrap_or(false);
                                                        this.config_values.insert(field_key, value);

                                                        this.configuring_field_index += 1;
                                                        if this.configuring_field_index >= fields.len() {
                                                            let config = agent_chat_core::ProviderConfig {
                                                                values: this.config_values.drain().collect(),
                                                            };
                                                            let mut validated = false;
                                                            for c in &this.crate_instances {
                                                                if let Ok(p) = c.create(id, config.clone()) {
                                                                    match p.validate_config() {
                                                                     Ok(()) => {
                                                                             this.provider_registry.register(Arc::from(p));
                                                                             this.provider_states.insert(id.clone(), ProviderState::Ready);
                                                                             this.provider_states_shared.borrow_mut().insert(id.clone(), ProviderState::Ready);
                                                                             this.provider_entries.remove(id);
                                                                             this.configuring_provider = None;
                                                                             this.config_error = None;
                                                                             this.refresh_provider_catalog(cx);
                                                                             // Fetch models from the freshly-configured provider
                                                                             if this.active_provider_ix < this.provider_catalog.len() {
                                                                                 this.fetch_models_in_background(this.active_provider_ix, cx);
                                                                             }
                                                                             validated = true;
                                                                        }
                                                                        Err(e) => {
                                                                            this.config_error = Some(e.to_string());
                                                                            // Clear sensitive fields on error
                                                                            for (k, v) in &mut this.config_values.iter_mut() {
                                                                                if fields.iter().any(|f| f.key == k.as_str() && f.sensitive) {
                                                                                    v.clear();
                                                                                }
                                                                            }
                                                                            this.configuring_field_index = 0;
                                                                            this.custom_provider_input.update(cx, |input, cx| {
                                                                                input.set_value("", window, cx);
                                                                            });
                                                                        }
                                                                    }
                                                                    break;
                                                                }
                                                            }
                                                            if !validated {
                                                                this.catalog_for_current_provider(cx);
                                                            }
                                                        } else {
                                                            this.custom_provider_input.update(cx, |input, cx| {
                                                                input.set_value("", window, cx);
                                                            });
                                                        }
                                                        cx.notify();
                                                    }
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
                                        .child(add_provider_prompt.unwrap_or_default()),
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
                                                .on_click(cx.listener(|this, _, _window, cx| {
                                                    this.cancel_add_provider_prompt(cx);
                                                })),
                                        )
                                        .child(
                                            Button::new("agent-chat-add-provider-next")
                                                .xsmall()
                                                .primary()
                                                .label("Save Provider")
                                                .tooltip("Save this custom provider")
                                                .disabled(
                                                    self.custom_provider_input
                                                        .read(cx)
                                                        .text()
                                                        .to_string()
                                                        .trim()
                                                        .is_empty(),
                                                )
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    let step = this.pending_custom_provider_step;
                                                    if let Some(s) = step {
                                                        let value = this.custom_provider_input.read(cx).text().to_string();
                                                        match s {
                                                            AddProviderPromptStep::ProviderLabel => {
                                                                if let Some(ref mut p) = this.pending_custom_provider {
                                                                    p.label = value;
                                                                }
                                                                this.pending_custom_provider_step = Some(AddProviderPromptStep::Endpoint);
                                                                this.custom_provider_input.update(cx, |input, cx| {
                                                                    input.set_value("", window, cx);
                                                                });
                                                            }
                                                            AddProviderPromptStep::Endpoint => {
                                                                if let Some(ref mut p) = this.pending_custom_provider {
                                                                    p.endpoint = value;
                                                                }
                                                                this.submit_custom_provider(window, cx);
                                                            }
                                                            _ => {}
                                                        }
                                                    }
                                                    cx.notify();
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
                                                let copy_tool_block = calls
                                                    .iter()
                                                    .map(|call| {
                                                        let args = if call.args_full.is_empty() {
                                                            call.args_preview.clone()
                                                        } else {
                                                            call.args_full.clone()
                                                        };
                                                        let result = call
                                                            .result_full
                                                            .as_deref()
                                                            .or(call.result_preview.as_deref())
                                                            .unwrap_or("running…");
                                                        format!(
                                                            "tool: {}\nargs: {}\nresult: {}{}",
                                                            call.name,
                                                            args,
                                                            result,
                                                            if call.is_error { "\nstatus: error" } else { "" }
                                                        )
                                                    })
                                                    .collect::<Vec<_>>()
                                                    .join("\n\n");
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
                                                                        Button::new(("tool-call-copy", ix))
                                                                            .xsmall()
                                                                            .ghost()
                                                                            .icon(IconName::Copy)
                                                                            .tooltip("Copy full tool block")
                                                                            .on_click(cx.listener(
                                                                                move |_, _, _, cx| {
                                                                                    cx.write_to_clipboard(
                                                                                        gpui::ClipboardItem::new_string(
                                                                                            copy_tool_block.clone(),
                                                                                        ),
                                                                                    );
                                                                                },
                                                                            )),
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
                                                let copy_summary = summary.clone();
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
                                                                        Button::new(("compaction-copy", ix))
                                                                            .xsmall()
                                                                            .ghost()
                                                                            .icon(IconName::Copy)
                                                                            .tooltip("Copy full compacted summary")
                                                                            .on_click(cx.listener(
                                                                                move |_, _, _, cx| {
                                                                                    cx.write_to_clipboard(
                                                                                        gpui::ClipboardItem::new_string(
                                                                                            copy_summary.clone(),
                                                                                        ),
                                                                                    );
                                                                                },
                                                                            )),
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
                                                let copy_content = content.clone();
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
                                                                        Button::new(("system-prompt-copy", ix))
                                                                            .xsmall()
                                                                            .ghost()
                                                                            .icon(IconName::Copy)
                                                                            .tooltip("Copy full system prompt")
                                                                            .on_click(cx.listener(
                                                                                move |_, _, _, cx| {
                                                                                    cx.write_to_clipboard(
                                                                                        gpui::ClipboardItem::new_string(
                                                                                            copy_content.clone(),
                                                                                        ),
                                                                                    );
                                                                                },
                                                                            )),
                                                                    )
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
                                                let copy_content = content.clone();
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
                                                                        Button::new(("thinking-copy", ix))
                                                                            .xsmall()
                                                                            .ghost()
                                                                            .icon(IconName::Copy)
                                                                            .tooltip("Copy full thinking block")
                                                                            .on_click(cx.listener(
                                                                                move |_, _, _, cx| {
                                                                                    cx.write_to_clipboard(
                                                                                        gpui::ClipboardItem::new_string(
                                                                                            copy_content.clone(),
                                                                                        ),
                                                                                    );
                                                                                },
                                                                            )),
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

                                            DisplayItem::SubagentInvocation {
                                                subagent_id: _,
                                                name,
                                                task,
                                                steps,
                                                is_expanded,
                                                status,
                                                started_at_ms,
                                                finished_at_ms,
                                            } => {
                                                let name = name.clone();
                                                let task = task.clone();
                                                let steps = steps.clone();
                                                let is_expanded = *is_expanded;
                                                let status = *status;
                                                let subagent_elapsed = Self::format_elapsed(*started_at_ms, *finished_at_ms, render_now_ms);
                                                
                                                let accent = match status {
                                                    SubagentStepStatus::Error => cx.theme().danger,
                                                    SubagentStepStatus::Success => cx.theme().success,
                                                    SubagentStepStatus::Running => cx.theme().info,
                                                    SubagentStepStatus::Pending => cx.theme().muted_foreground,
                                                };
                                                
                                                let status_icon = match status {
                                                    SubagentStepStatus::Error => IconName::CircleX,
                                                    SubagentStepStatus::Success => IconName::CircleCheck,
                                                    SubagentStepStatus::Running | SubagentStepStatus::Pending => IconName::Loader,
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
                                                            .gap_px()
                                                            .rounded(px(6.0))
                                                            .border_1()
                                                            .border_color(accent.opacity(0.25))
                                                            .bg(cx.theme().secondary)
                                                            .overflow_hidden()
                                                            .child(
                                                                h_flex()
                                                                    .id(("subagent-header", ix))
                                                                    .w_full()
                                                                    .px_3()
                                                                    .py(px(6.0))
                                                                    .gap_2()
                                                                    .cursor_pointer()
                                                                    .on_click(cx.listener(
                                                                        move |this, _, _, cx| {
                                                                            if let Some(
                                                                                DisplayItem::SubagentInvocation {
                                                                                    is_expanded,
                                                                                    ..
                                                                                },
                                                                            ) = this
                                                                                .display_items
                                                                                .get_mut(ix)
                                                                            {
                                                                                *is_expanded = !*is_expanded;
                                                                                this.display_item_heights.remove(&ix);
                                                                            }
                                                                            cx.notify();
                                                                        },
                                                                    ))
                                                                    .child(
                                                                        Icon::new(IconName::GitBranch)
                                                                            .size_3()
                                                                            .text_color(accent),
                                                                    )
                                                                    .child(
                                                                        v_flex()
                                                                            .flex_1()
                                                                            .gap_px()
                                                                            .child(
                                                                                div()
                                                                                    .text_xs()
                                                                                    .font_semibold()
                                                                                    .text_color(cx.theme().foreground)
                                                                                    .child(format!("Subagent: {}", name)),
                                                                            )
                                                                            .child(
                                                                                div()
                                                                                    .text_xs()
                                                                                    .text_color(cx.theme().muted_foreground)
                                                                                    .child(task),
                                                                            ),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(accent.opacity(0.7))
                                                                            .font_family("JetBrains Mono")
                                                                            .child(subagent_elapsed),
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
                                                                        .text_color(cx.theme().muted_foreground.opacity(0.6)),
                                                                    ),
                                                            )
                                                            .when(is_expanded && !steps.is_empty(), |el| {
                                                                el.child(
                                                                    v_flex()
                                                                        .w_full()
                                                                        .gap_px()
                                                                        .children(
                                                                            steps.iter().map(|step| {
                                                                                let step_accent = match step.status {
                                                                                    SubagentStepStatus::Error => cx.theme().danger,
                                                                                    SubagentStepStatus::Success => cx.theme().success,
                                                                                    SubagentStepStatus::Running | SubagentStepStatus::Pending => cx.theme().info,
                                                                                };
                                                                                let step_icon = match step.status {
                                                                                    SubagentStepStatus::Error => IconName::CircleX,
                                                                                    SubagentStepStatus::Success => IconName::CircleCheck,
                                                                                    SubagentStepStatus::Running => IconName::Loader,
                                                                                    SubagentStepStatus::Pending => IconName::Circle,
                                                                                };
                                                                                
                                                                                v_flex()
                                                                                    .w_full()
                                                                                    .px_3()
                                                                                    .py_2()
                                                                                    .gap_1()
                                                                                    .border_t_1()
                                                                                    .border_color(cx.theme().border)
                                                                                    .child(
                                                                                        h_flex()
                                                                                            .gap_2()
                                                                                            .items_center()
                                                                                            .child(
                                                                                                Icon::new(step_icon)
                                                                                                    .size_2()
                                                                                                    .text_color(step_accent),
                                                                                            )
                                                                                            .child(
                                                                                                div()
                                                                                                    .text_xs()
                                                                                                    .font_semibold()
                                                                                                    .text_color(cx.theme().foreground)
                                                                                                    .child(step.description.clone()),
                                                                                            ),
                                                                                    )
                                                                                    .when(!step.details.is_empty(), |el| {
                                                                                        el.child(
                                                                                            div()
                                                                                                .text_xs()
                                                                                                .font_family("JetBrains Mono")
                                                                                                .text_color(cx.theme().muted_foreground.opacity(0.8))
                                                                                                .whitespace_normal()
                                                                                                .child(step.details.clone()),
                                                                                        )
                                                                                    })
                                                                            }),
                                                                        ),
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
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(if self.is_processing_subagent_event {
                                    IconName::Loader
                                } else if queued_subagent_count > 0 {
                                    IconName::GitBranch
                                } else {
                                    IconName::CircleCheck
                                })
                                .size_3()
                                .text_color(if self.is_processing_subagent_event {
                                    cx.theme().info
                                } else if queued_subagent_count > 0 {
                                    cx.theme().warning
                                } else {
                                    cx.theme().success
                                }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(subagent_status_text),
                            )
                            .child(
                                div()
                                    .flex_1(),
                            )
                            .child(
                                Button::new("agent-chat-subagent-mode")
                                    .xsmall()
                                    .ghost()
                                    .label(if subagent_mode_is_manual {
                                        "Manual Queue"
                                    } else {
                                        "Auto Queue"
                                    })
                                    .tooltip("Toggle automatic processing of queued subagent completions")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.subagent_completion_mode = if this.subagent_completion_mode
                                            == SubagentCompletionMode::Auto
                                        {
                                            SubagentCompletionMode::Manual
                                        } else {
                                            SubagentCompletionMode::Auto
                                        };
                                        if this.subagent_completion_mode
                                            == SubagentCompletionMode::Auto
                                        {
                                            this.maybe_start_next_subagent_processing(cx);
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("agent-chat-subagent-process-next")
                                    .xsmall()
                                    .ghost()
                                    .label("Process Next")
                                    .tooltip("Process the next queued subagent completion now")
                                    .disabled(
                                        queued_subagent_count == 0
                                            || self.is_request_in_flight
                                            || self.is_processing_subagent_event,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.process_next_subagent_completion_now(cx);
                                    })),
                            ),
                    )
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
