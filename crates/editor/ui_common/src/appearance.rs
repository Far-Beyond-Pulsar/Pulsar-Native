//! The editor's appearance settings (`editor.appearance`: theme mode,
//! color-blind mode, font size, corner radius, scrollbars) applied to the
//! global theme. Startup applies them once; the Editor Settings window
//! applies them again whenever one changes, so a change shows at once.

use engine_state::settings::{global_config, ConfigValue, NS_EDITOR};
use gpui::{px, App};
use pulsar_settings::editor::appearance as keys;
use ui::{scroll::ScrollbarShow, ColorBlindMode, Theme, ThemeMode};

fn text(key: &str) -> Option<String> {
    match global_config().get(NS_EDITOR, keys::OWNER, key) {
        Ok(ConfigValue::String(value)) => Some(value),
        _ => None,
    }
}

fn int(key: &str) -> Option<i64> {
    match global_config().get(NS_EDITOR, keys::OWNER, key) {
        Ok(ConfigValue::Int(value)) => Some(value),
        _ => None,
    }
}

/// The color-blind mode a stored value names.
pub fn color_blind_mode(value: &str) -> ColorBlindMode {
    match value {
        "protanopia" => ColorBlindMode::Protanopia,
        "deuteranopia" => ColorBlindMode::Deuteranopia,
        "tritanopia" => ColorBlindMode::Tritanopia,
        "achromatopsia" => ColorBlindMode::Achromatopsia,
        _ => ColorBlindMode::None,
    }
}

/// Apply every appearance setting to the theme (and every open window).
pub fn apply(cx: &mut App) {
    let mode = match text(keys::THEME_MODE).as_deref() {
        Some("light") => ThemeMode::Light,
        _ => ThemeMode::Dark,
    };
    let color_blind = color_blind_mode(text(keys::COLOR_BLIND_MODE).as_deref().unwrap_or("none"));
    if cx.has_global::<Theme>() {
        Theme::global_mut(cx).color_blind_mode = color_blind;
    }
    // Colors first: switching the mode reapplies the theme's own config.
    Theme::change(mode, None, cx);

    let theme = Theme::global_mut(cx);
    if let Some(font_size) = int(keys::FONT_SIZE) {
        theme.font_size = px(font_size.clamp(10, 24) as f32);
    }
    if let Some(radius) = int(keys::RADIUS) {
        theme.radius = px(match radius {
            0 | 4 | 8 => radius as f32,
            _ => 6.0,
        });
    }
    if let Some(scrollbar) = text(keys::SCROLLBAR_SHOW) {
        theme.scrollbar_show = match scrollbar.as_str() {
            "hover" => ScrollbarShow::Hover,
            "always" => ScrollbarShow::Always,
            _ => ScrollbarShow::Scrolling,
        };
    }
    cx.refresh_windows();
}

/// Switch between light and dark (a title bar's toggle) and keep the choice
/// for the next start.
pub fn set_theme_mode(mode: ThemeMode, cx: &mut App) {
    Theme::change(mode, None, cx);
    let value = if mode.is_dark() { "dark" } else { "light" };
    if let Err(error) =
        engine_state::GlobalSettings::new().set_and_save(keys::OWNER, keys::THEME_MODE, value)
    {
        tracing::warn!(%error, "Could not persist the theme mode");
    }
}
