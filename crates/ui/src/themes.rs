//! Theme System
//!
//! This module provides theme management for the engine UI.
//!
//! ## Features
//!
//! - Load themes from embedded resources
//! - Extract themes to application data directory
//! - Theme switching with hot reload
//! - Persistent theme selection
//!
//! ## Theme Storage
//!
//! Themes are stored in:
//! - Embedded: `../../themes/*.json`
//! - Runtime: `{appdata}/themes/*.json`
//! - State: `{appdata}/state.json`
//!
//! ## Usage
//!
//! ```rust,ignore
//! // Initialize themes in GPUI app
//! crate::themes::init(cx);
//! 
//! // Theme will be loaded from last session
//! // or default to "Default Dark"
//! ```
//!
//! ## Implementation
//!
//! Uses GPUI's ThemeRegistry to manage themes and integrates with
//! the application's settings system.

use std::path::PathBuf;

use gpui::{
    div, px, Action, App, InteractiveElement as _, ParentElement as _, Render, SharedString,
};
use crate::{
    button::{Button, ButtonVariants},
    popup_menu::PopupMenuExt,
    scroll::ScrollbarShow,
    ActiveTheme, IconName, Sizable, Theme, ThemeRegistry,
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../themes"]
struct EmbeddedThemes;

fn get_state_file_path() -> PathBuf {
    let proj_dirs = directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .expect("Could not determine app data directory");
    let app_data_dir = proj_dirs.data_dir();
    app_data_dir.join("state.json")
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct State {
    theme: SharedString,
    scrollbar_show: Option<ScrollbarShow>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            theme: "Default Dark".into(),
            scrollbar_show: None,
        }
    }
}

pub fn init(cx: &mut App) {
    // Load last theme state
    let state_file_path = get_state_file_path();
    let json = std::fs::read_to_string(&state_file_path).unwrap_or(String::default());
    tracing::debug!("Load themes...");
    let state = serde_json::from_str::<State>(&json).unwrap_or_default();

    // Get app data directory
    let proj_dirs = directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .expect("Could not determine app data directory");
    let app_data_dir = proj_dirs.data_dir();
    let themes_dir = app_data_dir.join("themes");

    // Extract embedded themes if not already done
    if !themes_dir.exists() || !themes_dir.join("default.json").exists() {
        tracing::info!("Extracting embedded themes to {}", themes_dir.display());
        std::fs::create_dir_all(&themes_dir).expect("Failed to create themes directory");
        for file in EmbeddedThemes::iter() {
            let file_path = themes_dir.join(file.as_ref());
            tracing::info!("Extracting theme file: {}", file.as_ref());
            let data = EmbeddedThemes::get(&file).unwrap();
            std::fs::write(&file_path, data.data).expect("Failed to write theme file");
        }
        tracing::info!("Embedded themes extracted");
    } else {
        tracing::info!("Embedded themes already extracted");
    }

    tracing::info!("Themes dir: {}", themes_dir.display());
    if let Err(err) = ThemeRegistry::watch_dir(themes_dir, cx, move |cx| {
        if let Some(theme) = ThemeRegistry::global(cx)
            .themes()
            .get(&state.theme)
            .cloned()
        {
            Theme::global_mut(cx).apply_config(&theme);
        }
    }) {
        tracing::error!("Failed to watch themes directory: {}", err);
    } else {
        tracing::info!("Watching themes directory successfully");
    }

    if let Some(scrollbar_show) = state.scrollbar_show {
        Theme::global_mut(cx).scrollbar_show = scrollbar_show;
    }
    cx.refresh_windows();

    cx.observe_global::<Theme>(|cx| {
        let state = State {
            theme: cx.theme().theme_name().clone(),
            scrollbar_show: Some(cx.theme().scrollbar_show),
        };

        let json = serde_json::to_string_pretty(&state).unwrap();
        let state_file_path = get_state_file_path();
        if let Some(parent) = state_file_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(state_file_path, json).unwrap();
    })
    .detach();
}

#[derive(Action, Clone, PartialEq)]
#[action(namespace = themes, no_json)]
struct SwitchTheme(SharedString);

pub struct ThemeSwitcher {}

impl ThemeSwitcher {
    pub fn new(_: &mut App) -> Self {
        Self {}
    }
}

impl Render for ThemeSwitcher {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let theme_name = cx.theme().theme_name().clone();

        div()
            .id("theme-switcher")
            .on_action(cx.listener(|_, switch: &SwitchTheme, _, cx| {
                let theme_name = switch.0.clone();
                if let Some(theme_config) =
                    ThemeRegistry::global(cx).themes().get(&theme_name).cloned()
                {
                    Theme::global_mut(cx).apply_config(&theme_config);
                    cx.refresh_windows();
                }
                cx.notify();
            }))
            .child(
                Button::new("btn")
                    .icon(IconName::Palette)
                    .ghost()
                    .small()
                    .popup_menu({
                        let current_theme_id = theme_name.clone();
                        move |menu, _, cx| {
                            let mut menu = menu.scrollable().max_h(px(600.));

                            let names = ThemeRegistry::global(cx)
                                .sorted_themes()
                                .iter()
                                .map(|theme| theme.name.clone())
                                .collect::<Vec<SharedString>>();

                            for theme_name in names {
                                let is_selected = theme_name == current_theme_id;
                                menu = menu.menu_with_check(
                                    theme_name.clone(),
                                    is_selected,
                                    Box::new(SwitchTheme(theme_name.clone())),
                                );
                            }

                            menu
                        }
                    }),
            )
    }
}
