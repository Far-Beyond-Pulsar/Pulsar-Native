use gpui::*;
use gpui::prelude::FluentBuilder as _;
use ui::{
    h_flex, v_flex, Icon, IconName, ActiveTheme as _,
    button::{Button, ButtonVariants as _},
    switch::Switch,
};
use crate::settings::SettingsScreen;
use super::components::*;

impl SettingsScreen {
    pub fn render_editor_view(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_4()
            .child(render_section_header("Editor", cx))
            .child(self.render_editor_appearance_card(cx))
            .child(self.render_editor_behavior_card(cx))
            .child(self.render_code_formatting_card(cx))
    }

    fn render_editor_appearance_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let border_color = cx.theme().border;
        let primary_color = cx.theme().primary;
        let muted_foreground = cx.theme().muted_foreground;
        let font_size = self.settings.editor.font_size;
        let show_line_numbers = self.settings.editor.show_line_numbers;

        SettingCard::new("Editor Appearance")
            .icon(IconName::Code)
            .description("Customize how your code editor looks")
            .child(
                v_flex()
                    .w_full()
                    .gap_4()
                    .pt_4()
                    .border_t_1()
                    .border_color(border_color)
                    .child(
                        SettingRow::new("Font Size")
                            .description("Set the font size for code")
                            .control(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(render_value_display(format!("{:.1}", font_size), cx))
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
                            .render(cx)
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(1.0))
                            .bg(border_color)
                    )
                    .child(
                        SettingRow::new("Show Line Numbers")
                            .description("Display line numbers in the gutter")
                            .control(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        Switch::new("line-numbers-switch")
                                            .checked(show_line_numbers)
                                            .on_click(cx.listener(|screen, _, _window, cx| {
                                                screen.settings.editor.show_line_numbers = !screen.settings.editor.show_line_numbers;
                                                cx.notify();
                                            }))
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(if show_line_numbers {
                                                primary_color
                                            } else {
                                                muted_foreground
                                            })
                                            .child(if show_line_numbers { "Enabled" } else { "Disabled" })
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
                            .render(cx)
                    )
            )
            .render(cx)
    }

    fn render_editor_behavior_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let border_color = cx.theme().border;
        let primary_color = cx.theme().primary;
        let muted_foreground = cx.theme().muted_foreground;
        let word_wrap = self.settings.editor.word_wrap;

        SettingCard::new("Editor Behavior")
            .icon(IconName::Settings)
            .description("Configure how the editor behaves")
            .child(
                v_flex()
                    .w_full()
                    .gap_4()
                    .pt_4()
                    .border_t_1()
                    .border_color(border_color)
                    .child(
                        SettingRow::new("Word Wrap")
                            .description("Automatically wrap long lines")
                            .control(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        Switch::new("word-wrap-switch")
                                            .checked(word_wrap)
                                            .on_click(cx.listener(|screen, _, _window, cx| {
                                                screen.settings.editor.word_wrap = !screen.settings.editor.word_wrap;
                                                cx.notify();
                                            }))
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(if word_wrap {
                                                primary_color
                                            } else {
                                                muted_foreground
                                            })
                                            .child(if word_wrap { "Enabled" } else { "Disabled" })
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
                            .render(cx)
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(1.0))
                            .bg(border_color)
                    )
                    .child(
                        SettingRow::new("Tab Size")
                            .description("Number of spaces per tab")
                            .control(
                                render_value_display("4 spaces", cx)
                            )
                            .render(cx)
                    )
            )
            .render(cx)
    }

    fn render_code_formatting_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        SettingCard::new("Code Formatting")
            .icon(IconName::Code)
            .description("Automatic code formatting options")
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
                                                    .child("Auto-formatting")
                                            )
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.muted_foreground)
                                            .child("Automatic code formatting on save with language-specific rules. Coming in a future update.")
                                    )
                            )
                    )
            )
            .render(cx)
    }
}
