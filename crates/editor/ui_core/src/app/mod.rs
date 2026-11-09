//! Core application module

mod agent_chat_panel;
mod chrome;
mod constructors;
pub mod event_handlers;
mod layout;
pub(crate) use layout::saved_window_bounds;
mod manual_tool_panel;
mod menu_actions;
mod nav_sidebar;
mod navigation;
mod open_editors;
mod plugin_startup;
mod radial_menu;
mod render;
mod script_problems;
mod state;
mod status_bar;
mod tab_management;
mod task_queue_panel;
mod window_management;

use gpui::{App, AppContext, Context, DismissEvent, Focusable, Window};
use ui_common::menu;

use crate::actions::*;

pub(crate) fn refresh_plugin_editor_settings(manager: &mut plugin_manager::PluginManager) {
    use engine_state::settings::{global_config, ConfigValue, NS_EDITOR};
    use plugin_editor_api::{EditorSettingValue, EditorSettingsSnapshot};

    let settings = global_config()
        .list_settings(NS_EDITOR, "code_editor")
        .unwrap_or_default()
        .into_iter()
        .filter_map(|setting| {
            let value = global_config()
                .get(NS_EDITOR, "code_editor", &setting.key)
                .ok()?;
            let value = match value {
                ConfigValue::Bool(value) => EditorSettingValue::Boolean(value),
                ConfigValue::Int(value) => EditorSettingValue::Integer(value),
                ConfigValue::Float(value) => EditorSettingValue::Float(value),
                ConfigValue::String(value) => EditorSettingValue::Text(value),
                ConfigValue::Color(_) | ConfigValue::Array(_) => return None,
            };
            Some((setting.key, value))
        })
        .collect();

    manager.set_editor_settings_snapshot(EditorSettingsSnapshot::new(settings));
}

/// Main Pulsar application
pub struct PulsarApp {
    state: state::AppState,
}

impl PulsarApp {
    pub(super) fn install_tab_preview_handlers(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dock_area = self.state.dock_area.clone();
        let panels = dock_area.read(cx).tab_panels(cx);
        let sidebar = self.state.nav_sidebar.downgrade();

        for (_, tabs) in panels {
            let panel_id = tabs.entity_id();
            if self.state.preview_tab_panels.contains(&panel_id) {
                continue;
            }

            let preview_decorator_sidebar = self.state.nav_sidebar.clone();
            let preview_area_sidebar = sidebar.clone();
            tabs.update(cx, |tabs, cx| {
                tabs.set_tab_preview_callbacks(
                    move |panel, trigger, _window, app| {
                        nav_sidebar::decorate_horizontal_tab_preview(
                            preview_decorator_sidebar.clone(),
                            panel,
                            trigger,
                            app,
                        )
                    },
                    move |hovered, _window, app| {
                        let _ = preview_area_sidebar.update(app, |sidebar, cx| {
                            sidebar.preview_area_hovered(hovered, cx);
                        });
                    },
                    cx,
                );
            });
            cx.subscribe_in(&tabs, window, event_handlers::on_tab_panel_event)
                .detach();
            self.state.preview_tab_panels.push(panel_id);
        }
    }

    // Action handlers
    fn on_toggle_file_manager(
        &mut self,
        _: &ToggleFileManager,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_drawer(window, cx);
    }

    fn on_toggle_problems(
        &mut self,
        _: &ToggleProblems,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_problems(window, cx);
    }

    fn on_toggle_type_debugger(
        &mut self,
        _: &ToggleTypeDebugger,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_type_debugger(window, cx);
    }

    fn on_toggle_multiplayer(
        &mut self,
        _: &ToggleMultiplayer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_multiplayer(window, cx);
    }

    fn on_toggle_agent_chat(
        &mut self,
        _: &ToggleAgentChat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_agent_chat(window, cx);
    }

    fn on_open_file(&mut self, action: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        self.open_path(action.path.clone(), window, cx);
    }

    fn on_open_asset(
        &mut self,
        action: &plugin_editor_api::OpenAsset,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_path(action.path.clone(), window, cx);
    }

    fn on_activate_open_editor(
        &mut self,
        action: &ActivateOpenEditor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let activated = self.activate_open_editor_by_global_index(action.index, window, cx);

        if !activated {
            tracing::warn!("ActivateOpenEditor index out of range: {}", action.index);
        }

        self.refresh_open_editor_snapshot(cx);
    }

    fn on_go_back(&mut self, _: &menu::GoBack, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.state.go_back() {
            self.open_path_with_history(path, false, window, cx);
        }
    }

    fn on_go_forward(&mut self, _: &menu::GoForward, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.state.go_forward() {
            self.open_path_with_history(path, false, window, cx);
        }
    }

    fn on_toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use ui_common::command_palette::{GenericPalette, PaletteViewDelegate};

        self.state.command_palette_open = !self.state.command_palette_open;

        if self.state.command_palette_open {
            // Get palette from state
            let palette = self
                .state
                .command_palette
                .clone()
                .expect("Palette not initialized");

            // Create or reuse view
            if let Some(view) = &self.state.command_palette_view {
                // Reuse existing view - reset search state for fresh open
                view.update(cx, |palette_view, _cx| {
                    palette_view.reset_filter();
                });
                let input_handle = view.read(cx).search_input.read(cx).focus_handle(cx);
                input_handle.focus(window, cx);
            } else {
                // Create new view with delegate
                let delegate = PaletteViewDelegate::new(palette.clone(), &*cx);
                let view = cx.new(|cx| GenericPalette::new(delegate, window, cx));

                // Subscribe to dismiss event
                cx.subscribe_in(
                    &view,
                    window,
                    move |this: &mut PulsarApp, view, _event: &DismissEvent, window, cx| {
                        // Extract selected item ID
                        let selected_item_id =
                            view.update(cx, |view, _cx| view.delegate_mut().take_selected_item());

                        // Execute callback if item selected
                        if let Some(item_id) = selected_item_id {
                            palette.update(cx, |palette, cx| {
                                let _ = palette.execute_item(item_id, window, cx);
                            });
                        }

                        this.state.command_palette_open = false;
                        this.state.focus_handle.focus(window, cx);
                        cx.notify();
                    },
                )
                .detach();

                // Focus input
                let input_handle = view.read(cx).search_input.read(cx).focus_handle(cx);
                input_handle.focus(window, cx);

                self.state.command_palette_view = Some(view);
            }
        } else {
            self.state.focus_handle.focus(window, cx);
        }

        cx.notify();
    }

    /// Update Discord Rich Presence with current editor state
    pub(crate) fn update_discord_presence(&self, cx: &App) {
        if let Some(engine_state) = engine_state::EngineContext::global() {
            let project_name = self.state.project_path.as_ref().and_then(|path| {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
            });

            let (tab_name, file_path, discord_icon_key) = self
                .state
                .center_tabs
                .read(cx)
                .active_panel(cx)
                .map(|panel| {
                    let tab_name = panel.panel_name(cx).to_string();
                    let discord_key = panel.discord_icon_key(cx);
                    let file_path: Option<String> = None;
                    (Some(tab_name), file_path, Some(discord_key))
                })
                .unwrap_or((None, None, None));

            tracing::debug!(
                "🎮 Updating Discord presence: project={:?}, tab={:?}, file={:?}, icon={:?}",
                project_name,
                tab_name,
                file_path,
                discord_icon_key
            );

            if let Some(discord) = engine_state.discord() {
                discord.update_all_with_icon(project_name, tab_name, file_path, discord_icon_key);
            }
        } else {
            tracing::warn!("⚠️  Cannot update Discord presence: EngineState not available");
        }
    }
}
