//! Handlers for the global app-menu actions (View / Go / Search / Project /
//! Tools).
//!
//! The menus themselves live in `ui_common::menu`, which cannot see the app;
//! each action is handled here by calling the same methods the keyboard
//! shortcuts and toolbar already use, so a menu entry is never a second
//! implementation.

use gpui::{Context, InteractiveElement, Window, px};
use ui::{ActiveTheme as _, Theme};
use ui_common::menu;

use crate::actions::ToggleCommandPalette;

use super::PulsarApp;

/// Smallest / largest UI font size the zoom commands reach, in logical pixels.
const MIN_FONT: f32 = 10.0;
const MAX_FONT: f32 = 24.0;
const DEFAULT_FONT: f32 = 14.0;

impl PulsarApp {
    /// Attach the global-menu action handlers to `element`.
    pub(super) fn with_menu_actions<E: InteractiveElement>(
        element: E,
        cx: &mut Context<Self>,
    ) -> E {
        element
            // View
            .on_action(cx.listener(|this, _: &menu::ToggleExplorer, window, cx| {
                this.toggle_drawer(window, cx)
            }))
            .on_action(cx.listener(|this, _: &menu::ToggleProblems, window, cx| {
                this.toggle_problems(window, cx)
            }))
            .on_action(cx.listener(|this, _: &menu::ShowAgentChat, window, cx| {
                this.toggle_agent_chat(window, cx)
            }))
            .on_action(
                cx.listener(|_, _: &menu::ToggleFullscreen, window, _| window.toggle_fullscreen()),
            )
            .on_action(
                cx.listener(|_, _: &menu::ZoomIn, window, cx| Self::zoom_ui(1.0, window, cx)),
            )
            .on_action(
                cx.listener(|_, _: &menu::ZoomOut, window, cx| Self::zoom_ui(-1.0, window, cx)),
            )
            .on_action(cx.listener(|_, _: &menu::ResetZoom, window, cx| {
                Theme::global_mut(cx).font_size = px(DEFAULT_FONT);
                if let Err(error) = engine_state::GlobalSettings::new().set_and_save(
                    "appearance",
                    "font_size",
                    engine_state::ConfigValue::Int(DEFAULT_FONT as i64),
                ) {
                    tracing::warn!(%error, "Could not persist appearance font size");
                }
                window.refresh();
            }))
            // View / Go / Search: one palette serves them all
            .on_action(cx.listener(|this, _: &menu::CommandPalette, window, cx| {
                this.on_toggle_command_palette(&ToggleCommandPalette, window, cx)
            }))
            .on_action(cx.listener(|this, _: &menu::GoToFile, window, cx| {
                this.on_toggle_command_palette(&ToggleCommandPalette, window, cx)
            }))
            .on_action(cx.listener(|this, _: &menu::QuickOpen, window, cx| {
                this.on_toggle_command_palette(&ToggleCommandPalette, window, cx)
            }))
            // Project
            .on_action(cx.listener(|_, _: &menu::ProjectSettings, _, cx| {
                use gpui::UpdateGlobal as _;
                window_manager::WindowRegistry::update_global(cx, |reg, cx| {
                    reg.open("SettingsWindow", cx)
                });
            }))
            .on_action(cx.listener(|this, _: &menu::OpenCargoToml, window, cx| {
                if let Some(root) = this.state.project_path.clone() {
                    this.open_path(root.join("Cargo.toml"), window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &menu::RevealProjectFolder, _, cx| {
                if let Some(root) = this.state.project_path.as_deref() {
                    cx.reveal_path(root);
                }
            }))
            // Tools
            .on_action(cx.listener(|this, _: &menu::ToggleProfiler, window, cx| {
                this.toggle_flamegraph(window, cx)
            }))
            .on_action(cx.listener(|this, _: &menu::ToggleConsole, window, cx| {
                this.toggle_log_viewer(window, cx)
            }))
            .on_action(cx.listener(|this, _: &menu::ShowTypeDebugger, window, cx| {
                this.toggle_type_debugger(window, cx)
            }))
            .on_action(cx.listener(|this, _: &menu::ToggleNetwork, window, cx| {
                this.toggle_multiplayer(window, cx)
            }))
    }

    /// Change the UI font size by `delta` px, within sensible limits.
    fn zoom_ui(delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        let current = f32::from(cx.theme().font_size);
        let next = (current + delta).clamp(MIN_FONT, MAX_FONT);
        Theme::global_mut(cx).font_size = px(next);
        if let Err(error) = engine_state::GlobalSettings::new().set_and_save(
            "appearance",
            "font_size",
            engine_state::ConfigValue::Int(next.round() as i64),
        ) {
            tracing::warn!(%error, "Could not persist appearance font size");
        }
        window.refresh();
    }
}
