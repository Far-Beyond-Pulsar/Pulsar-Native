use crate::custom_providers::{self, CustomProvider};
use agent_chat_core::{
    ChatMessage, ChatProvider, ChatRole, ProviderCrate, ProviderEntry, ProviderRegistry,
};
use agent_chat_tools::ToolRegistry;
use gpui::{prelude::FluentBuilder as _, *};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    path::PathBuf,
    rc::Rc,
    sync::{Arc, RwLock},
};
use ui::{
    dock::{DockArea, DockItem, Panel, PanelEvent, TabPanel},
    dropdown::{SearchableList, SearchableListItemAction, SearchableListItemState},
    input::InputState,
    scroll::ScrollbarState,
    VirtualListScrollHandle,
};

use super::chat_storage;
use super::provider_selection;
use super::types::*;

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
    pub(crate) streaming_display_item_ix: Option<usize>,
    pub(crate) pending_rollback_confirm_ix: Option<usize>,
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) display_items: Vec<DisplayItem>,
    pub(crate) display_item_heights: HashMap<usize, Pixels>,
    pub(crate) cancel_tx: Option<smol::channel::Sender<()>>,
    pub(crate) pending_subagent_events: VecDeque<serde_json::Value>,
    pub(crate) is_processing_subagent_event: bool,
    pub(crate) processing_subagent_id: Option<String>,
    pub(crate) subagent_completion_mode: SubagentCompletionMode,
    pub(crate) auto_scroll: bool,
    pub(crate) chat_viewport_height: Pixels,
    pub(crate) _subscriptions: Vec<Subscription>,
}

impl AgentChatPanel {
    /// Get stored auth token for a provider (from the old token-based auth system).
    /// In the new config-based system, tokens are handled via provider config.
    pub(super) fn auth_token_for_provider(&self, provider_id: &str) -> Option<String> {
        self.provider_tokens.get(provider_id).cloned()
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
