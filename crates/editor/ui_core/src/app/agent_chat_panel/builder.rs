use crate::custom_providers;
use agent_chat_core::{ChatMessage, ChatRole, ProviderConfig, ProviderCrate};
use agent_chat_tools::ToolRegistry;
use agent_provider_anthropic::AnthropicProviderCrate;
use agent_provider_aws_bedrock::AwsBedrockProviderCrate;
use agent_provider_demo_random::DemoRandomProviderCrate;
use agent_provider_docker_model_runner::DockerModelRunnerProviderCrate;
use agent_provider_gemini::GeminiProviderCrate;
use agent_provider_github_copilot::GithubCopilotProviderCrate;
use agent_provider_openai::OpenAiProviderCrate;
use agent_provider_opencode::OpenCodeProviderCrate;
use agent_provider_vertex_ai::VertexAiProviderCrate;
use gpui::*;
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};
use ui::{
    VirtualListScrollHandle,
    dock::{DockArea, TabPanel},
    dropdown::{
        SearchableList, SearchableListEvent, SearchableListItemAction, SearchableListItemState,
    },
    input::InputState,
    scroll::ScrollbarState,
};

use super::panel::AgentChatPanel;
use super::types::*;
use super::{chat_storage, provider_selection};

impl AgentChatPanel {
    pub fn new(
        dock_area: Entity<DockArea>,
        center_tabs: Entity<TabPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut provider_registry = agent_chat_core::ProviderRegistry::new();

        let crate_instances: Vec<Box<dyn ProviderCrate>> = vec![
            Box::new(OpenAiProviderCrate),
            Box::new(OpenCodeProviderCrate),
            Box::new(AnthropicProviderCrate),
            Box::new(GeminiProviderCrate),
            Box::new(GithubCopilotProviderCrate),
            Box::new(AwsBedrockProviderCrate),
            Box::new(VertexAiProviderCrate),
            Box::new(DemoRandomProviderCrate),
            Box::new(DockerModelRunnerProviderCrate),
        ];

        let mut provider_states: HashMap<String, ProviderState> = HashMap::new();
        let provider_states_shared: Rc<RefCell<HashMap<String, ProviderState>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let mut provider_entries: HashMap<String, agent_chat_core::ProviderEntry> = HashMap::new();
        let disabled_providers = ["aws_bedrock", "vertex_ai"];

        for crate_impl in &crate_instances {
            let entries = crate_impl.entries();
            for entry in entries {
                // Templates aren't providers: they list as "New … connection"
                // and each saved configuration becomes its own provider below.
                if entry.template {
                    let id = entry.id.to_string();
                    provider_states.insert(id.clone(), ProviderState::Template);
                    provider_states_shared
                        .borrow_mut()
                        .insert(id.clone(), ProviderState::Template);
                    provider_entries.insert(id, entry);
                    continue;
                }
                let needs_config = entry.config_fields.iter().any(|f| f.required);
                let config = ProviderConfig {
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
                    provider_states_shared
                        .borrow_mut()
                        .insert(id.clone(), state.clone());
                    provider_registry.register(Arc::from(provider));
                }
            }
        }

        // Saved connections (configured templates) from the engine data dir.
        // Validated lazily when selected, like every other Ready provider.
        let provider_instances = custom_providers::load_provider_instances();
        for instance in &provider_instances {
            let Some(template) = provider_entries.get(&instance.template_id).cloned() else {
                tracing::warn!(
                    "Saved provider '{}' uses unknown template '{}'",
                    instance.name,
                    instance.template_id
                );
                continue;
            };
            match Self::instantiate_provider(&crate_instances, instance) {
                Ok(provider) => {
                    provider_registry.register(Arc::from(provider));
                    provider_states.insert(instance.id.clone(), ProviderState::Ready);
                    provider_states_shared
                        .borrow_mut()
                        .insert(instance.id.clone(), ProviderState::Ready);
                    provider_entries
                        .insert(instance.id.clone(), Self::instance_entry(&template, instance));
                }
                Err(e) => tracing::warn!("Saved provider '{}' failed to load: {e}", instance.name),
            }
        }

        let provider_catalog = Self::build_provider_catalog(
            &provider_registry,
            &provider_entries,
            &provider_states,
            &provider_instances,
            &HashMap::new(),
        );

        let plugin_bridge = plugin_manager::global().map(|manager_lock| {
            Arc::new(std::sync::RwLock::new(
                manager_lock.read().build_tool_bridge(),
            ))
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
                ProviderKind::Cloud => ui::IconName::Cloud,
                ProviderKind::Local => ui::IconName::Server,
            })
            .with_item_state(move |p: &ProviderDefinition| {
                let map = states_shared.borrow();
                match map.get(p.id) {
                    Some(ProviderState::Ready | ProviderState::Template) => {
                        SearchableListItemState::Enabled
                    }
                    Some(ProviderState::Unconfigured) => SearchableListItemState::Locked,
                    Some(ProviderState::Disabled) | None => SearchableListItemState::Disabled,
                }
            })
            .with_item_actions(|p: &ProviderDefinition| {
                if p.deletable {
                    vec![SearchableListItemAction {
                        id: "delete".into(),
                        icon: Some(ui::IconName::Trash),
                        label: Some("Delete connection".into()),
                        destructive: true,
                    }]
                } else {
                    vec![]
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
            .with_icon_getter(|_| ui::IconName::Cpu)
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
            provider_instances,
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
            pending_subagent_events: std::collections::VecDeque::new(),
            is_processing_subagent_event: false,
            processing_subagent_id: None,
            subagent_completion_mode: super::panel::SubagentCompletionMode::Auto,
            auto_scroll: true,
            chat_viewport_height: px(0.0),
            _subscriptions: subscriptions,
        };

        this.bootstrap_chat_storage(cx);
        let initial_ix = this.active_provider_ix;
        this.fetch_models_in_background(initial_ix, cx);
        this
    }
}
