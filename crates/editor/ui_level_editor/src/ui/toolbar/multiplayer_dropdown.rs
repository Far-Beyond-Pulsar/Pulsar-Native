use gpui::*;
use ui::{
    IconName, Sizable,
    button::{Button, ButtonVariants as _},
    popup_menu::PopupMenuExt,
};

use super::actions::SetMultiplayerMode;
use crate::state::MultiplayerMode;

/// Multiplayer mode dropdown - Styled appropriately for mode selection
pub struct MultiplayerDropdown;

impl MultiplayerDropdown {
    pub fn render<V>(
        state: &engine_state::playback::PlaybackState,
        _cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let mode_label = match state.multiplayer_mode {
            MultiplayerMode::Offline => "Offline",
            MultiplayerMode::Host => "Host",
            MultiplayerMode::Client => "Client",
        };

        let mode_icon = match state.multiplayer_mode {
            MultiplayerMode::Offline => IconName::CircleX,
            MultiplayerMode::Host => IconName::Server,
            MultiplayerMode::Client => IconName::Network,
        };

        let current_mode = state.multiplayer_mode;

        Button::new("multiplayer_dropdown")
            .label(mode_label)
            .icon(mode_icon)
            .small()
            .ghost()
            .tooltip("Select multiplayer mode")
            .popup_menu(move |menu, _, _| {
                menu.label("Multiplayer Mode")
                    .separator()
                    .menu_with_check(
                        "Offline",
                        current_mode == MultiplayerMode::Offline,
                        Box::new(SetMultiplayerMode(MultiplayerMode::Offline)),
                    )
                    .menu_with_check(
                        "Host Server",
                        current_mode == MultiplayerMode::Host,
                        Box::new(SetMultiplayerMode(MultiplayerMode::Host)),
                    )
                    .menu_with_check(
                        "Connect as Client",
                        current_mode == MultiplayerMode::Client,
                        Box::new(SetMultiplayerMode(MultiplayerMode::Client)),
                    )
            })
    }
}
