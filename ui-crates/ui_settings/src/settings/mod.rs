use ui::settings::EngineSettings;
use gpui::*;
use ui::label::Label;
use ui::menu::popup_menu::PopupMenuExt;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, Icon, IconName, Theme, ThemeRegistry,
    switch::Switch,
    input::{InputState, TextInput},
};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsCategory {
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
            Self::Appearance => "Visual theme and UI customization",
            Self::Editor => "Code editor preferences and behavior",
            Self::Project => "Project defaults and file management",
            Self::Advanced => "Performance, debugging, and extensions",
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
    settings: EngineSettings,
    /// Path to config file
    config_path: PathBuf,
    /// Currently selected theme (may be unsaved)
    selected_theme: String,
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Update search query from input state
        let current_input_value = self.search_input.read(cx).value().to_string();
        if current_input_value != self.search_query {
            self.search_query = current_input_value;
        }

        let theme = cx.theme();

        v_flex()
            .size_full()
            .bg(theme.background)
            .on_action(cx.listener(|screen, action: &SelectThemeAction, _w: &mut gpui::Window, cx| {
                screen.selected_theme = action.theme_name.to_string();
                if let Some(theme_config) = ThemeRegistry::global(cx).themes().get(&action.theme_name).cloned() {
                    Theme::global_mut(cx).apply_config(&theme_config);
                    cx.refresh_windows();
                }
                cx.notify();
            }))
            .child(
                // Professional header
                v_flex()
                    .w_full()
                    .gap_3()
                    .px_6()
                    .py_4()
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
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        div()
                                            .w(px(48.0))
                                            .h(px(48.0))
                                            .rounded_lg()
                                            .bg(theme.accent.opacity(0.15))
                                            .border_1()
                                            .border_color(theme.accent.opacity(0.3))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                Icon::new(IconName::Settings)
                                                    .size(px(24.0))
                                                    .text_color(theme.accent)
                                            )
                                    )
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_2xl()
                                                    .font_weight(gpui::FontWeight::BOLD)
                                                    .text_color(theme.foreground)
                                                    .child("Settings")
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
                                    .gap_2()
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
                            )
                    )
                    // Search bar
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
            )
            .child(
                // Main content: sidebar + settings
                h_flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.render_sidebar(cx))
                    .child(self.render_settings_content(_window, cx))
            )
    }
}

impl SettingsScreen {
    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let categories = [
            SettingsCategory::Appearance,
            SettingsCategory::Editor,
            SettingsCategory::Project,
            SettingsCategory::Advanced,
        ];

        v_flex()
            .w(px(280.0))
            .h_full()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.muted_foreground)
                            .child("CATEGORIES")
                    )
            )
            .child(
                div()
                    .id("settings-sidebar-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .p_2()
                            .gap_1()
                            .children(categories.iter().map(|category| {
                                self.render_category_button(*category, cx)
                            }))
                    )
            )
    }

    fn render_category_button(&self, category: SettingsCategory, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let is_active = self.active_category == category;

        let base_div = div()
            .w_full()
            .px_3()
            .py_2p5()
            .rounded_md();
        
        let styled_div = if is_active {
            base_div
                .bg(theme.accent.opacity(0.15))
                .border_l_2()
                .border_color(theme.accent)
        } else {
            base_div
        };

        styled_div
            .hover(|this| {
                this.bg(theme.secondary.opacity(0.5))
            })
            .cursor_pointer()
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |screen, _event, _window, cx| {
                screen.active_category = category;
                cx.notify();
            }))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .w(px(32.0))
                            .h(px(32.0))
                            .rounded_md()
                            .bg(if is_active {
                                theme.accent.opacity(0.15)
                            } else {
                                theme.muted.opacity(0.1)
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(category.icon())
                                    .size_4()
                                    .text_color(if is_active {
                                        theme.accent
                                    } else {
                                        theme.muted_foreground
                                    })
                            )
                    )
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(if is_active {
                                        gpui::FontWeight::SEMIBOLD
                                    } else {
                                        gpui::FontWeight::NORMAL
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
        div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .child(
                div()
                    .id("settings-content-scroll")
                    .w_full()
                    .h_full()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .w_full()
                            .p_8()
                            .pb(px(100.))
                            .gap_8()
                            .child(match self.active_category {
                                SettingsCategory::Appearance => self.render_appearance_category(window, cx),
                                SettingsCategory::Editor => self.render_editor_category(cx),
                                SettingsCategory::Project => self.render_project_category(cx),
                                SettingsCategory::Advanced => self.render_advanced_category(cx),
                            })
                    )
            )
    }

    fn render_appearance_category(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme_names: Vec<String> = ThemeRegistry::global(cx)
            .sorted_themes()
            .iter()
            .map(|t| t.name.to_string())
            .collect();

        v_flex()
            .w_full()
            .gap_4()
            .child(self.render_theme_setting(&theme_names, cx))
            .into_any_element()
    }

    fn render_editor_category(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .w_full()
            .gap_4()
            .child(self.render_font_size_setting(cx))
            .child(self.render_line_numbers_setting(cx))
            .child(self.render_word_wrap_setting(cx))
            .into_any_element()
    }

    fn render_project_category(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .w_full()
            .gap_4()
            .child(self.render_default_project_path_setting(cx))
            .child(self.render_auto_save_setting(cx))
            .child(self.render_backup_setting(cx))
            .into_any_element()
    }

    fn render_advanced_category(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .w_full()
            .gap_4()
            .child(self.render_performance_setting(cx))
            .child(self.render_debugging_setting(cx))
            .child(self.render_extensions_setting(cx))
            .into_any_element()
    }

    fn render_section(
        &self,
        title: &str,
        icon: IconName,
        settings: Vec<AnyElement>,
        cx: &mut Context<Self>
    ) -> AnyElement {
        let theme = cx.theme();

        v_flex()
            .w_full()
            .gap_4()
            .px_6()
            .py_5()
            .bg(theme.sidebar.opacity(0.5))
            .border_1()
            .border_color(theme.border.opacity(0.5))
            .rounded_lg()
            .shadow_sm()
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .w(px(40.0))
                            .h(px(40.0))
                            .rounded_lg()
                            .bg(theme.accent.opacity(0.15))
                            .border_1()
                            .border_color(theme.accent.opacity(0.3))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(icon)
                                    .size(px(20.0))
                                    .text_color(theme.accent)
                            )
                    )
                    .child(
                        div()
                            .text_lg()
                            .text_color(theme.foreground)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(title.to_string())
                    )
            )
            .children(settings)
            .into_any_element()
    }

    fn render_theme_setting(&self, theme_names: &[String], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();

        self.render_section(
            "Theme",
            IconName::Palette,
            vec![
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .line_height(rems(1.5))
                            .child("Choose your preferred visual theme. Changes apply instantly.")
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .flex_wrap()
                            .child(
                                Button::new("theme-dropdown")
                                    .label(&self.selected_theme)
                                    .icon(IconName::Palette)
                                    .popup_menu({
                                        let theme_names = theme_names.to_vec();
                                        let selected = self.selected_theme.clone();
                                        move |menu, _w: &mut gpui::Window, _cx| {
                                            let mut menu = menu.scrollable().max_h(px(300.));
                                            for name in &theme_names {
                                                let is_selected = name == &selected;
                                                menu = menu.menu_with_check(
                                                    name.clone(),
                                                    is_selected,
                                                    Box::new(SelectThemeAction::new(SharedString::from(name.clone()))),
                                                );
                                            }
                                            menu
                                        }
                                    })
                            )
                            .child(
                                Button::new("save-theme")
                                    .primary()
                                    .icon(IconName::Check)
                                    .label("Save")
                                    .on_click(cx.listener(|screen, _, _window: &mut gpui::Window, cx| {
                                        screen.settings.active_theme = screen.selected_theme.clone();
                                        screen.settings.save(&screen.config_path);
                                        cx.notify();
                                    }))
                            )
                    )
                    .into_any_element()
            ],
            cx
        )
    }

    fn render_font_size_setting(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();

        v_flex()
            .gap_3()
            .pt_4()
            .border_t_1()
            .border_color(theme.border)
            .child(
                Label::new("Font Size")
                    .text_base()
                    .text_color(theme.foreground)
                    .font_weight(gpui::FontWeight::MEDIUM)
            )
            .child(
                Label::new("Set the font size for the editor")
                    .text_sm()
                    .text_color(theme.muted_foreground)
            )
            .child(
                h_flex()
                    .gap_4()
                    .items_center()
                    .child(
                        Label::new(format!("{:.1}", self.settings.editor.font_size))
                            .text_sm()
                            .text_color(theme.foreground)
                            .bg(theme.background)
                            .border_1()
                            .border_color(theme.border)
                            .rounded(px(4.))
                            .p_2()
                    )
                    .child(
                        Button::new("save-font-size")
                            .primary()
                            .label("Save")
                            .on_click(cx.listener(|screen, _, _window, cx| {
                                screen.settings.save(&screen.config_path);
                                cx.notify();
                            }))
                    )
            )
            .into_any_element()
    }

    fn render_line_numbers_setting(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();

        v_flex()
            .gap_3()
            .pt_4()
            .border_t_1()
            .border_color(theme.border)
            .child(
                Label::new("Line Numbers")
                    .text_base()
                    .text_color(theme.foreground)
                    .font_weight(gpui::FontWeight::MEDIUM)
            )
            .child(
                Label::new("Show or hide line numbers in the editor")
                    .text_sm()
                    .text_color(theme.muted_foreground)
            )
            .child(
                h_flex()
                    .gap_4()
                    .items_center()
                    .child(
                        Switch::new("line-numbers-switch")
                            .checked(self.settings.editor.show_line_numbers)
                            .on_click(cx.listener(|screen, _, _window, cx| {
                                screen.settings.editor.show_line_numbers = !screen.settings.editor.show_line_numbers;
                                cx.notify();
                            }))
                    )
                    .child(
                        Label::new(if self.settings.editor.show_line_numbers { "Enabled" } else { "Disabled" })
                            .text_sm()
                            .text_color(theme.foreground)
                    )
                    .child(
                        Button::new("save-line-numbers")
                            .primary()
                            .label("Save")
                            .on_click(cx.listener(|screen, _, _window, cx| {
                                screen.settings.save(&screen.config_path);
                                cx.notify();
                            }))
                    )
            )
            .into_any_element()
    }

    fn render_word_wrap_setting(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();

        v_flex()
            .gap_3()
            .pt_4()
            .border_t_1()
            .border_color(theme.border)
            .child(
                Label::new("Word Wrap")
                    .text_base()
                    .text_color(theme.foreground)
                    .font_weight(gpui::FontWeight::MEDIUM)
            )
            .child(
                Label::new("Enable or disable automatic word wrapping")
                    .text_sm()
                    .text_color(theme.muted_foreground)
            )
            .child(
                h_flex()
                    .gap_4()
                    .items_center()
                    .child(
                        Switch::new("word-wrap-switch")
                            .checked(self.settings.editor.word_wrap)
                            .on_click(cx.listener(|screen, _, _window, cx| {
                                screen.settings.editor.word_wrap = !screen.settings.editor.word_wrap;
                                cx.notify();
                            }))
                    )
                    .child(
                        Label::new(if self.settings.editor.word_wrap { "Enabled" } else { "Disabled" })
                            .text_sm()
                            .text_color(theme.foreground)
                    )
                    .child(
                        Button::new("save-word-wrap")
                            .primary()
                            .label("Save")
                            .on_click(cx.listener(|screen, _, _window, cx| {
                                screen.settings.save(&screen.config_path);
                                cx.notify();
                            }))
                    )
            )
            .into_any_element()
    }

    fn render_default_project_path_setting(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();

        v_flex()
            .gap_3()
            .pt_4()
            .border_t_1()
            .border_color(theme.border)
            .child(
                Label::new("Default Project Path")
                    .text_base()
                    .text_color(theme.foreground)
                    .font_weight(gpui::FontWeight::MEDIUM)
            )
            .child(
                Label::new("Set the default directory for new projects")
                    .text_sm()
                    .text_color(theme.muted_foreground)
            )
            .child(
                h_flex()
                    .gap_4()
                    .items_center()
                    .child(
                        Label::new(self.settings.project.default_project_path.as_deref().unwrap_or("Not set").to_string())
                            .text_sm()
                            .text_color(theme.foreground)
                            .bg(theme.background)
                            .border_1()
                            .border_color(theme.border)
                            .rounded(px(4.))
                            .p_2()
                    )
                    .child(
                        Button::new("browse-project-path")
                            .ghost()
                            .label("Browse")
                            .icon(IconName::Folder)
                            .on_click(cx.listener(|_this, _, _window, cx| {
                                // TODO: Implement folder picker
                                cx.notify();
                            }))
                    )
                    .child(
                        Button::new("save-project-path")
                            .primary()
                            .label("Save")
                            .on_click(cx.listener(|screen, _, _window, cx| {
                                screen.settings.save(&screen.config_path);
                                cx.notify();
                            }))
                    )
            )
            .into_any_element()
    }

    fn render_auto_save_setting(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();

        v_flex()
            .gap_3()
            .pt_4()
            .border_t_1()
            .border_color(theme.border)
            .child(
                Label::new("Auto Save")
                    .text_base()
                    .text_color(theme.foreground)
                    .font_weight(gpui::FontWeight::MEDIUM)
            )
            .child(
                Label::new("Automatically save project changes")
                    .text_sm()
                    .text_color(theme.muted_foreground)
            )
            .child(
                h_flex()
                    .gap_4()
                    .items_center()
                    .child(
                        Switch::new("auto-save-switch")
                            .checked(self.settings.project.auto_save_interval > 0)
                            .on_click(cx.listener(|screen, _, _window, cx| {
                                if screen.settings.project.auto_save_interval > 0 {
                                    screen.settings.project.auto_save_interval = 0;
                                } else {
                                    screen.settings.project.auto_save_interval = 30; // Default 30 seconds
                                }
                                cx.notify();
                            }))
                    )
                    .child(
                        Label::new(if self.settings.project.auto_save_interval > 0 { "Enabled" } else { "Disabled" })
                            .text_sm()
                            .text_color(theme.foreground)
                    )
                    .child(
                        Label::new(format!("Interval: {} seconds", self.settings.project.auto_save_interval))
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .bg(theme.background)
                            .border_1()
                            .border_color(theme.border)
                            .rounded(px(4.))
                            .p_2()
                    )
                    .child(
                        Label::new("seconds")
                            .text_sm()
                            .text_color(theme.muted_foreground)
                    )
                    .child(
                        Button::new("save-auto-save")
                            .primary()
                            .label("Save")
                            .on_click(cx.listener(|screen, _, _window, cx| {
                                screen.settings.save(&screen.config_path);
                                cx.notify();
                            }))
                    )
            )
            .into_any_element()
    }

    fn render_backup_setting(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();

        v_flex()
            .gap_3()
            .pt_4()
            .border_t_1()
            .border_color(theme.border)
            .child(
                Label::new("Backup Settings")
                    .text_base()
                    .text_color(theme.foreground)
                    .font_weight(gpui::FontWeight::MEDIUM)
            )
            .child(
                Label::new("Configure automatic project backups")
                    .text_sm()
                    .text_color(theme.muted_foreground)
            )
            .child(
                h_flex()
                    .gap_4()
                    .items_center()
                    .child(
                        Switch::new("backup-enabled-switch")
                            .checked(self.settings.project.enable_backups)
                            .on_click(cx.listener(|screen, _, _window, cx| {
                                screen.settings.project.enable_backups = !screen.settings.project.enable_backups;
                                cx.notify();
                            }))
                    )
                    .child(
                        Label::new(if self.settings.project.enable_backups { "Enabled" } else { "Disabled" })
                            .text_sm()
                            .text_color(theme.foreground)
                    )
                    .child(
                        Label::new("Backups are automatically created when saving projects")
                            .text_sm()
                            .text_color(theme.muted_foreground)
                    )
                    .child(
                        Button::new("save-backup")
                            .primary()
                            .label("Save")
                            .on_click(cx.listener(|screen, _, _window, cx| {
                                screen.settings.save(&screen.config_path);
                                cx.notify();
                            }))
                    )
            )
            .into_any_element()
    }

    fn render_performance_setting(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        
        let fps_options = vec![30u32, 60, 120, 144, 240, 0];
        let current_fps = self.settings.advanced.max_viewport_fps;

        v_flex()
            .gap_3()
            .pt_4()
            .border_t_1()
            .border_color(theme.border)
            .child(
                Label::new("Performance Settings")
                    .text_base()
                    .text_color(theme.foreground)
                    .font_weight(gpui::FontWeight::MEDIUM)
            )
            .child(
                Label::new("Configure performance-related options")
                    .text_sm()
                    .text_color(theme.muted_foreground)
            )
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        Label::new(format!("Performance Level: {}", self.settings.advanced.performance_level))
                            .text_sm()
                            .text_color(theme.foreground)
                            .bg(theme.background)
                            .border_1()
                            .border_color(theme.border)
                            .rounded(px(4.))
                            .p_2()
                    )
                    .child(
                        Label::new("Higher levels may improve performance but use more resources")
                            .text_sm()
                            .text_color(theme.muted_foreground)
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                Label::new("Viewport Max FPS (Frame Pacing)")
                                    .text_sm()
                                    .text_color(theme.foreground)
                                    .font_weight(gpui::FontWeight::MEDIUM)
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .children(fps_options.iter().map(|&fps| {
                                        let label = if fps == 0 { "Unlimited".to_string() } else { format!("{} FPS", fps) };
                                        let is_selected = current_fps == fps;
                                        
                                        let mut btn = Button::new(SharedString::from(format!("fps-{}", fps)))
                                            .label(label);
                                        
                                        if is_selected {
                                            btn = btn.primary();
                                        } else {
                                            btn = btn.ghost();
                                        }
                                        
                                        btn.on_click(cx.listener(move |screen, _, _window, cx| {
                                            screen.settings.advanced.max_viewport_fps = fps;
                                            screen.settings.save(&screen.config_path);
                                            cx.notify();
                                        }))
                                    }))
                            )
                            .child(
                                Label::new("Controls viewport refresh rate for consistent frame pacing")
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                            )
                    )
                    .child(
                        Button::new("save-performance")
                            .primary()
                            .label("Save All Settings")
                            .on_click(cx.listener(|screen, _, _window, cx| {
                                screen.settings.save(&screen.config_path);
                                cx.notify();
                            }))
                    )
            )
            .into_any_element()
    }

    fn render_debugging_setting(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();

        v_flex()
            .gap_3()
            .pt_4()
            .border_t_1()
            .border_color(theme.border)
            .child(
                Label::new("Debugging Options")
                    .text_base()
                    .text_color(theme.foreground)
                    .font_weight(gpui::FontWeight::MEDIUM)
            )
            .child(
                Label::new("Configure debugging and development features")
                    .text_sm()
                    .text_color(theme.muted_foreground)
            )
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_4()
                            .items_center()
                            .child(
                                Switch::new("debug-logging-switch")
                                    .checked(self.settings.advanced.debug_logging)
                                    .on_click(cx.listener(|screen, _, _window, cx| {
                                        screen.settings.advanced.debug_logging = !screen.settings.advanced.debug_logging;
                                        cx.notify();
                                    }))
                            )
                            .child(
                                Label::new("Debug Logging")
                                    .text_sm()
                                    .text_color(theme.foreground)
                            )
                    )
                    .child(
                        h_flex()
                            .gap_4()
                            .items_center()
                            .child(
                                Switch::new("experimental-features-switch")
                                    .checked(self.settings.advanced.experimental_features)
                                    .on_click(cx.listener(|screen, _, _window, cx| {
                                        screen.settings.advanced.experimental_features = !screen.settings.advanced.experimental_features;
                                        cx.notify();
                                    }))
                            )
                            .child(
                                Label::new("Experimental Features")
                                    .text_sm()
                                    .text_color(theme.foreground)
                            )
                    )
                    .child(
                        Button::new("save-debugging")
                            .primary()
                            .label("Save")
                            .on_click(cx.listener(|screen, _, _window, cx| {
                                screen.settings.save(&screen.config_path);
                                cx.notify();
                            }))
                    )
            )
            .into_any_element()
    }

    fn render_extensions_setting(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();

        v_flex()
            .gap_3()
            .pt_4()
            .border_t_1()
            .border_color(theme.border)
            .child(
                Label::new("Extensions")
                    .text_base()
                    .text_color(theme.foreground)
                    .font_weight(gpui::FontWeight::MEDIUM)
            )
            .child(
                Label::new("Manage installed extensions and plugins")
                    .text_sm()
                    .text_color(theme.muted_foreground)
            )
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        Label::new("Extension management features are coming soon")
                            .text_sm()
                            .text_color(theme.muted_foreground)
                    )
            )
            .into_any_element()
    }

    fn render_placeholder_setting(&self, title: &str, description: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();

        v_flex()
            .gap_2()
            .pt_4()
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                Label::new(title.to_string())
                                    .text_base()
                                    .text_color(theme.foreground)
                                    .font_weight(gpui::FontWeight::MEDIUM)
                            )
                            .child(
                                Label::new(description.to_string())
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                            )
                    )
                    .child(
                        Button::new("configure-placeholder")
                            .ghost()
                            .label("Configure")
                            .on_click(cx.listener(move |_this, _, _window, cx| {
                                // TODO: Implement configuration for this setting
                                cx.notify();
                            }))
                    )
            )
            .into_any_element()
    }
}

#[derive(Clone, PartialEq, Eq, gpui::Action)]
#[action(namespace = ui, no_json)]
struct SelectThemeAction {
    theme_name: SharedString,
}

impl SelectThemeAction {
    pub fn new(theme_name: SharedString) -> Self {
        Self { theme_name }
    }
}
