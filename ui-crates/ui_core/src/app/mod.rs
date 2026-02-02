//! Core application module

mod state;
mod constructors;
pub mod event_handlers;
mod tab_management;
mod window_management;
mod render;
mod panel_window;

use gpui::{App, AppContext, Context, DismissEvent, Focusable, Window};

use crate::actions::*;

/// Main Pulsar application
pub struct PulsarApp {
    state: state::AppState,
}

impl PulsarApp {
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

    fn on_toggle_flamegraph(
        &mut self,
        _: &ToggleFlamegraph,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_flamegraph(window, cx);
    }

    fn on_open_file(&mut self, action: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        self.open_path(action.path.clone(), window, cx);
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
            let palette = self.state.command_palette.clone().expect("Palette not initialized");

            // Create or reuse view
            if let Some(view) = &self.state.command_palette_view {
                // Reuse existing view (already subscribed)
                let input_handle = view.read(cx).search_input.read(cx).focus_handle(cx);
                input_handle.focus(window);
            } else {
                // Create new view with delegate
                let delegate = PaletteViewDelegate::new(palette.clone(), &*cx);
                let view = cx.new(|cx| GenericPalette::new(delegate, window, cx));

                // Subscribe to dismiss event
                cx.subscribe_in(&view, window, move |this: &mut PulsarApp, view, _event: &DismissEvent, window, cx| {
                    // Extract selected item ID
                    let selected_item_id = view.update(cx, |view, _cx| {
                        view.delegate_mut().take_selected_item()
                    });

                    // Execute callback if item selected
                    if let Some(item_id) = selected_item_id {
                        palette.update(cx, |palette, cx| {
                            let _ = palette.execute_item(item_id, window, cx);
                        });
                    }

                    this.state.command_palette_open = false;
                    this.state.focus_handle.focus(window);
                    cx.notify();
                }).detach();

                // Focus input
                let input_handle = view.read(cx).search_input.read(cx).focus_handle(cx);
                input_handle.focus(window);

                self.state.command_palette_view = Some(view);
            }
        } else {
            self.state.focus_handle.focus(window);
        }

        cx.notify();
    }


    /// Update Discord Rich Presence with current editor state
    pub(crate) fn update_discord_presence(&self, cx: &App) {
        if let Some(engine_state) = engine_state::EngineState::global() {
            let project_name = self.state.project_path.as_ref().and_then(|path| {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
            });

            let (tab_name, file_path, discord_icon_key) = self.state.center_tabs.read(cx).active_panel(cx)
                .map(|panel| {
                    let tab_name = panel.panel_name(cx).to_string();
                    let discord_key = panel.discord_icon_key(cx);
                    let file_path: Option<String> = None;
                    (Some(tab_name), file_path, Some(discord_key))
                })
                .unwrap_or((None, None, None));

            tracing::debug!("🎮 Updating Discord presence: project={:?}, tab={:?}, file={:?}, icon={:?}",
                project_name, tab_name, file_path, discord_icon_key);

            if let Some(discord) = engine_state.discord() {
                discord.update_all_with_icon(project_name, tab_name, file_path, discord_icon_key);
            }
        } else {
            tracing::warn!("⚠️  Cannot update Discord presence: EngineState not available");
        }
    }
}
