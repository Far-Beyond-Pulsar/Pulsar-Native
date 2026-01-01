//! Core application module

mod state;
mod constructors;
pub mod event_handlers;
mod tab_management;
mod window_management;
mod render;

use gpui::{App, AppContext, Context, DismissEvent, Focusable, Window};
use ui_common::command_palette::{CommandOrFile, CommandType};
use ui::{ContextModal, notification::Notification};

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

    fn on_toggle_terminal(
        &mut self,
        _: &ToggleTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_terminal(window, cx);
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

    fn on_toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::unified_palette::AnyPaletteDelegate;
        use ui_common::command_palette::GenericPalette;

        self.state.command_palette_open = !self.state.command_palette_open;

        if self.state.command_palette_open {
            if let Some(palette) = &self.state.command_palette {
                palette.update(cx, |palette, cx| {
                    let delegate = AnyPaletteDelegate::command(self.state.project_path.clone());
                    palette.swap_delegate(delegate, window, cx);
                });

                let input_handle = palette.read(cx).search_input.read(cx).focus_handle(cx);
                input_handle.focus(window);
            } else {
                let delegate = AnyPaletteDelegate::command(self.state.project_path.clone());
                let palette = cx.new(|cx| GenericPalette::new(delegate, window, cx));

                cx.subscribe_in(&palette, window, |this: &mut PulsarApp, palette, _event: &DismissEvent, window, cx| {
                    let selected_item = palette.update(cx, |palette, _cx| {
                        palette.delegate_mut().take_selected_command()
                    });

                    if let Some(item) = selected_item {
                        this.handle_command_or_file_selected(item, window, cx);
                    }

                    this.state.command_palette_open = false;
                    this.state.focus_handle.focus(window);
                    cx.notify();
                }).detach();

                let input_handle = palette.read(cx).search_input.read(cx).focus_handle(cx);
                input_handle.focus(window);

                self.state.command_palette = Some(palette);
            }
        } else {
            self.state.focus_handle.focus(window);
        }

        cx.notify();
    }

    pub(crate) fn handle_command_or_file_selected(
        &mut self,
        item: CommandOrFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match item {
            CommandOrFile::Command(cmd) => {
                match cmd.command_type {
                    CommandType::Files => {
                        self.state.command_palette_open = true;
                        return;
                    }
                    CommandType::ToggleFileManager => {
                        self.toggle_drawer(window, cx);
                    }
                    CommandType::ToggleTerminal => {
                        self.toggle_terminal(window, cx);
                    }
                    CommandType::ToggleMultiplayer => {
                        self.toggle_multiplayer(window, cx);
                    }
                    CommandType::ToggleProblems => {
                        self.toggle_problems(window, cx);
                    }
                    CommandType::OpenSettings => {
                        cx.dispatch_action(&ui::OpenSettings);
                    }
                    CommandType::BuildProject => {
                        window.push_notification(
                            Notification::info("Build")
                                .message("Building project..."),
                            cx
                        );
                    }
                    CommandType::RunProject => {
                        window.push_notification(
                            Notification::info("Run")
                                .message("Running project..."),
                            cx
                        );
                    }
                    CommandType::RestartAnalyzer => {
                        self.state.rust_analyzer.update(cx, |analyzer, cx| {
                            analyzer.restart(window, cx);
                        });
                    }
                    CommandType::StopAnalyzer => {
                        self.state.rust_analyzer.update(cx, |analyzer, cx| {
                            analyzer.stop(window, cx);
                        });
                    }
                }
            }
            CommandOrFile::File(file) => {
                self.open_path(file.path, window, cx);
            }
        }
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
