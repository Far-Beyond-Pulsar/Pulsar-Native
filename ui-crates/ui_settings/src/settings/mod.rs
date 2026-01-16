mod views;
pub use views::*;

use ui::settings::EngineSettings;
use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, Icon, IconName, Theme, ThemeRegistry,
    input::{InputState, TextInput},
    scroll::ScrollbarAxis,
    StyledExt as _,
};
use std::path::PathBuf;
use gpui::prelude::FluentBuilder as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsCategory {
    Appearance,
    Editor,
    Project,
    Advanced,
}

impl SettingsCategory {
    fn label(&self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Editor => "Editor",
            Self::Project => "Project",
            Self::Advanced => "Advanced",
        }
    }

    fn icon(&self) -> IconName {
        match self {
            Self::Appearance => IconName::Palette,
            Self::Editor => IconName::Code,
            Self::Project => IconName::Folder,
            Self::Advanced => IconName::Settings,
        }
    }

    fn description(&self) -> &'static str {
        match self {
            Self::Appearance => "Themes and visual customization",
            Self::Editor => "Code editor preferences",
            Self::Project => "Project defaults and file management",
            Self::Advanced => "Performance and debugging",
        }
    }
}

/// Props for the settings screen
pub struct SettingsScreenProps {
    /// Path to the config file (engine.toml)
    pub config_path: PathBuf,
}

/// The settings screen entity
pub struct SettingsScreen {
    /// Current settings loaded from disk
    pub settings: EngineSettings,
    /// Path to config file
    pub config_path: PathBuf,
    /// Currently selected theme (may be unsaved)
    pub selected_theme: String,
    /// Active settings category
    active_category: SettingsCategory,
    /// Search input state
    search_input: Entity<InputState>,
    /// Search query
    search_query: String,
}

impl SettingsScreen {
    pub fn new(props: SettingsScreenProps, window: &mut Window, cx: &mut App) -> Self {
        let settings = EngineSettings::load(&props.config_path);
        let selected_theme = settings.active_theme.clone();

        let search_input = cx.new(|cx| InputState::new(window, cx));

        Self {
            settings,
            config_path: props.config_path,
            selected_theme,
            active_category: SettingsCategory::Appearance,
            search_input,
            search_query: String::new(),
        }
    }
}

impl Render for SettingsScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Update search query from input state
        let current_input_value = self.search_input.read(cx).value().to_string();
        if current_input_value != self.search_query {
            self.search_query = current_input_value;
        }

        let theme = cx.theme();

        v_flex()
            .size_full()
            .bg(theme.background)
            .on_action(cx.listener(|screen, action: &SelectThemeAction, _w: &mut Window, cx| {
                screen.selected_theme = action.theme_name.to_string();
                if let Some(theme_config) = ThemeRegistry::global(cx).themes().get(&action.theme_name).cloned() {
                    Theme::global_mut(cx).apply_config(&theme_config);
                    cx.refresh_windows();
                }
                cx.notify();
            }))
            .child(self.render_header(cx))
            .child(
                // Main content: sidebar + settings with proper overflow
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.render_sidebar(cx))
                    .child(self.render_settings_content(window, cx))
            )
    }
}

impl SettingsScreen {
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .w_full()
            .gap_4()
            .px_8()
            .py_5()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_4()
                            .items_center()
                            .child(
                                div()
                                    .w(px(56.0))
                                    .h(px(56.0))
                                    .rounded_xl()
                                    .bg(hsla(theme.primary.h, theme.primary.s, theme.primary.l, 0.15))
                                    .border_1()
                                    .border_color(hsla(theme.primary.h, theme.primary.s, theme.primary.l, 0.3))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Icon::new(IconName::Settings)
                                            .size(px(28.0))
                                            .text_color(theme.primary)
                                    )
                            )
                            .child(
                                v_flex()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .text_3xl()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.foreground)
                                            .child("Engine Settings")
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.muted_foreground)
                                            .child("Customize your Pulsar Engine experience")
                                    )
                            )
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                Button::new("reset")
                                    .ghost()
                                    .icon(IconName::Refresh)
                                    .label("Reset")
                                    .on_click(cx.listener(|screen, _, _window, cx| {
                                        screen.settings = EngineSettings::load(&screen.config_path);
                                        screen.selected_theme = screen.settings.active_theme.clone();
                                        cx.notify();
                                    }))
                            )
                            .child(
                                Button::new("save-all")
                                    .primary()
                                    .icon(IconName::Check)
                                    .label("Save All")
                                    .on_click(cx.listener(|screen, _, _window, cx| {
                                        screen.settings.save(&screen.config_path);
                                        cx.notify();
                                    }))
                            )
                    )
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(600.0))
                    .child(
                        TextInput::new(&self.search_input)
                            .w_full()
                            .prefix(
                                Icon::new(IconName::Search)
                                    .size_4()
                                    .text_color(theme.muted_foreground)
                            )
                    )
            )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let categories = [
            SettingsCategory::Appearance,
            SettingsCategory::Editor,
            SettingsCategory::Project,
            SettingsCategory::Advanced,
        ];

        v_flex()
            .w(px(300.0))
            .h_full()
            .flex_shrink_0()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .px_5()
                    .py_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.muted_foreground)
                            .child("CATEGORIES")
                    )
            )
            .child(
                v_flex()
                    .id("settings-sidebar-categories")
                    .flex_1()
                    .p_3()
                    .gap_2()
                    .scrollable(Axis::Vertical)
                    .children(categories.iter().map(|category| {
                        self.render_category_button(*category, cx)
                    }))
            )
    }

    fn render_category_button(&self, category: SettingsCategory, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let is_active = self.active_category == category;

        div()
            .w_full()
            .px_4()
            .py_3p5()
            .rounded_lg()
            .when(is_active, |this| {
                this.bg(hsla(theme.primary.h, theme.primary.s, theme.primary.l, 0.15))
                    .border_1()
                    .border_color(hsla(theme.primary.h, theme.primary.s, theme.primary.l, 0.3))
            })
            .when(!is_active, |this| {
                this.hover(|style| {
                    style.bg(theme.secondary.opacity(0.5))
                })
            })
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(move |screen, _event, _window, cx| {
                screen.active_category = category;
                cx.notify();
            }))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .w(px(40.0))
                            .h(px(40.0))
                            .rounded_lg()
                            .bg(if is_active {
                                hsla(theme.primary.h, theme.primary.s, theme.primary.l, 0.2)
                            } else {
                                theme.muted.opacity(0.1)
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(category.icon())
                                    .size(px(20.0))
                                    .text_color(if is_active {
                                        theme.primary
                                    } else {
                                        theme.muted_foreground
                                    })
                            )
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(if is_active {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::MEDIUM
                                    })
                                    .text_color(if is_active {
                                        theme.foreground
                                    } else {
                                        theme.foreground.opacity(0.9)
                                    })
                                    .child(category.label())
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(category.description())
                            )
                    )
            )
    }

    fn render_settings_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .flex_1()
            .min_w_0()
            .size_full()
            .scrollable(ScrollbarAxis::Vertical)
            .child(
                v_flex()
                    .w_full()
                    .p_8()
                    .pb(px(120.0))
                    .gap_6()
                    .child(match self.active_category {
                        SettingsCategory::Appearance => self.render_appearance_view(window, cx).into_any_element(),
                        SettingsCategory::Editor => self.render_editor_view(window, cx).into_any_element(),
                        SettingsCategory::Project => self.render_project_view(window, cx).into_any_element(),
                        SettingsCategory::Advanced => self.render_advanced_view(window, cx).into_any_element(),
                    })
            )
    }
}

#[derive(Clone, PartialEq, Eq, gpui::Action)]
#[action(namespace = ui, no_json)]
pub struct SelectThemeAction {
    theme_name: SharedString,
}

impl SelectThemeAction {
    pub fn new(theme_name: SharedString) -> Self {
        Self { theme_name }
    }
}
