//! The Game page's two asset pickers (`utils::game`): the shared searchable
//! [`AssetPicker`] in a popover, and a None button.
//!
//! The default level offers the project's `.level` files; the game instance
//! offers the class folders that implement the Game Instance trait.

use std::path::Path;

use gpui::{
    px, AnyElement, AppContext as _, Context, Corner, Entity, IntoElement, ParentElement as _,
    SharedString, Styled, Subscription, Window,
};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    popover::Popover,
    Disableable as _, Sizable as _,
};
use ui_common::asset_picker::{AssetPickedEvent, AssetPicker, AssetQuery};

use super::SettingsScreen;
use crate::utils::game::{self, DefaultLevel};

/// A picker and the subscription that takes its choice.
pub struct GamePicker {
    picker: Entity<AssetPicker>,
    _subscription: Subscription,
}

/// The editor's `project.default_map` setting, which the game consults when
/// the project names no default level.
fn default_map() -> Option<String> {
    engine_state::global_config()
        .get(engine_state::NS_PROJECT, "project", "default_map")
        .ok()?
        .as_str()
        .ok()
        .map(str::to_owned)
}

/// The last component of a path, as a button label.
fn file_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(
        || path.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// What the trigger button of the row `key` says.
fn label_of(settings: &game::GameSettings, key: &str) -> String {
    if key == game::GAME_INSTANCE {
        return settings
            .game_instance()
            .map_or_else(|| "None".to_owned(), file_name);
    }
    match settings.default_level(default_map().as_deref()) {
        DefaultLevel::Level(level) => file_name(&level),
        DefaultLevel::None => "None".to_owned(),
        DefaultLevel::Automatic(Some(level)) => format!("{} (default)", file_name(&level)),
        DefaultLevel::Automatic(None) => "None (no default level found)".to_owned(),
    }
}

impl SettingsScreen {
    fn game_picker(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<AssetPicker>> {
        if let Some(existing) = self.game_pickers.get(key) {
            return Some(existing.picker.clone());
        }
        let settings = self.game.as_ref()?;
        let root = settings.root().to_path_buf();
        let level = key == game::STARTUP_LEVEL;
        let selected = if level {
            match settings.default_level(default_map().as_deref()) {
                DefaultLevel::Level(level) | DefaultLevel::Automatic(Some(level)) => level,
                _ => String::new(),
            }
        } else {
            settings.game_instance().unwrap_or_default().to_owned()
        };
        let (builtins, queries) = if level {
            (Vec::new(), vec![AssetQuery::extension("level")])
        } else {
            (game::game_instance_classes(&root), Vec::new())
        };
        let picker =
            cx.new(|cx| AssetPicker::new(selected, builtins, Some(root), queries, window, cx));
        let name = key.to_owned();
        let subscription = cx.subscribe(&picker, move |this, picker, _: &AssetPickedEvent, cx| {
            let chosen = picker.read(cx).selected_path().to_owned();
            if let Some(settings) = this.game.as_mut() {
                if name == game::STARTUP_LEVEL {
                    settings.set_level(&chosen);
                } else {
                    settings.set_game_instance(&chosen);
                }
                crate::handlers::mark_changed(this, false);
                cx.notify();
            }
        });
        self.game_pickers.insert(
            key.to_owned(),
            GamePicker {
                picker: picker.clone(),
                _subscription: subscription,
            },
        );
        Some(picker)
    }

    /// The control of a Game page row.
    pub(crate) fn render_game_field(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(picker) = self.game_picker(key, window, cx) else {
            return gpui::div().child("No project is open").into_any_element();
        };
        let Some(settings) = self.game.as_ref() else {
            return gpui::div().into_any_element();
        };
        let label: SharedString = label_of(settings, key).into();
        let none_disabled = if key == game::GAME_INSTANCE {
            settings.game_instance().is_none()
        } else {
            settings.default_level(default_map().as_deref()) == DefaultLevel::None
        };
        let key = key.to_owned();
        let cleared = key.clone();
        h_flex()
            .gap_1()
            .items_center()
            .child(
                Popover::<AssetPicker>::new(SharedString::from(format!("game-{key}-picker")))
                    .anchor(Corner::TopRight)
                    .trigger(
                        Button::new(SharedString::from(format!("game-{key}")))
                            .label(label)
                            .dropdown_caret(true)
                            .outline()
                            .small()
                            .min_w(px(200.0)),
                    )
                    .content(move |_, _| picker.clone()),
            )
            .child(
                Button::new(SharedString::from(format!("game-{key}-none")))
                    .label("None")
                    .ghost()
                    .small()
                    .disabled(none_disabled)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let Some(settings) = this.game.as_mut() else {
                            return;
                        };
                        if cleared == game::STARTUP_LEVEL {
                            // Explicitly none: no fallback.
                            settings.set_no_level();
                        } else {
                            settings.reset(game::GAME_INSTANCE);
                        }
                        // The picker's highlight starts over.
                        this.game_pickers.remove(&cleared);
                        crate::handlers::mark_changed(this, false);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}
