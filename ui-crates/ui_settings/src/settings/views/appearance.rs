use gpui::*;
use gpui::prelude::FluentBuilder as _;
use ui::{
    h_flex, v_flex, Icon, IconName, ActiveTheme as _, Theme, ThemeRegistry,
    button::{Button, ButtonVariants as _},
    menu::popup_menu::PopupMenuExt,
};
use crate::settings::{SettingsScreen, SelectThemeAction};
use super::components::*;

impl SettingsScreen {
    pub fn render_appearance_view(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let theme_names: Vec<String> = ThemeRegistry::global(cx)
            .sorted_themes()
            .iter()
            .map(|t| t.name.to_string())
            .collect();

        v_flex()
            .w_full()
            .gap_4()
            .child(render_section_header("Appearance", cx))
            .child(self.render_theme_card(&theme_names, cx))
            .child(self.render_ui_scale_card(cx))
            .child(self.render_color_scheme_info_card(cx))
    }

    fn render_theme_card(&self, theme_names: &[String], cx: &mut Context<Self>) -> impl IntoElement {
        let border_color = cx.theme().border;
        let accent = cx.theme().accent;
        let foreground = cx.theme().foreground;

        SettingCard::new("Visual Theme")
            .icon(IconName::Palette)
            .description("Choose your preferred visual theme. Changes apply instantly to give you a preview.")
            .child(
                v_flex()
                    .w_full()
                    .gap_4()
                    .pt_4()
                    .border_t_1()
                    .border_color(border_color)
                    .child(
                        SettingRow::new("Current Theme")
                            .description("Select from available themes")
                            .control(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        Button::new("theme-dropdown")
                                            .label(&self.selected_theme)
                                            .icon(IconName::Palette)
                                            .popup_menu({
                                                let theme_names = theme_names.to_vec();
                                                let selected = self.selected_theme.clone();
                                                move |menu, _w: &mut Window, _cx| {
                                                    let mut menu = menu.scrollable().max_h(px(400.));
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
                                            .on_click(cx.listener(|screen, _, _window, cx| {
                                                screen.settings.active_theme = screen.selected_theme.clone();
                                                screen.settings.save(&screen.config_path);
                                                cx.notify();
                                            }))
                                    )
                            )
                            .render(cx)
                    )
                    .child(
                        div()
                            .p_4()
                            .rounded_lg()
                            .bg(hsla(accent.h, accent.s, accent.l, 0.1))
                            .border_1()
                            .border_color(hsla(accent.h, accent.s, accent.l, 0.2))
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_start()
                                    .child(
                                        Icon::new(IconName::Info)
                                            .size(px(20.0))
                                            .text_color(accent)
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(foreground)
                                            .child("Theme changes are previewed instantly. Click 'Save' to make your choice permanent.")
                                    )
                            )
                    )
            )
            .render(cx)
    }

    fn render_ui_scale_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        SettingCard::new("UI Scale")
            .icon(IconName::ZoomIn)
            .description("Adjust the overall size of the user interface")
            .child(
                v_flex()
                    .w_full()
                    .gap_4()
                    .pt_4()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        SettingRow::new("Scale Factor")
                            .description("Coming soon: Adjust UI scale for better visibility")
                            .control(
                                render_value_display("1.0x (Default)", cx)
                            )
                            .render(cx)
                    )
            )
            .render(cx)
    }

    fn render_color_scheme_info_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        SettingCard::new("Color Customization")
            .icon(IconName::Palette)
            .description("Advanced color customization options")
            .child(
                v_flex()
                    .w_full()
                    .gap_4()
                    .pt_4()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .p_5()
                            .rounded_lg()
                            .bg(theme.background)
                            .border_1()
                            .border_color(theme.border)
                            .child(
                                v_flex()
                                    .gap_3()
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap_3()
                                            .child(
                                                Icon::new(IconName::Star)
                                                    .size(px(24.0))
                                                    .text_color(theme.primary)
                                            )
                                            .child(
                                                div()
                                                    .text_base()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(theme.foreground)
                                                    .child("Custom Theme Editor")
                                            )
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.muted_foreground)
                                            .child("Create and customize your own themes with fine-grained color control. Coming in a future update.")
                                    )
                            )
                    )
            )
            .render(cx)
    }
}
