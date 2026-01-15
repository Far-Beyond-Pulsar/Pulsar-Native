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
    pub fn render_project_view(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_6()
            .child(render_section_header("Project Settings", cx))
            .child(self.render_project_defaults_card(cx))
            .child(self.render_auto_save_card(cx))
            .child(self.render_backup_card(cx))
    }

    fn render_project_defaults_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        SettingCard::new("Project Defaults")
            .icon(IconName::Folder)
            .description("Default settings for new projects")
            .child(
                v_flex()
                    .w_full()
                    .gap_4()
                    .pt_4()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        SettingRow::new("Default Project Path")
                            .description("Where new projects are created by default")
                            .control(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        render_value_display(
                                            self.settings.project.default_project_path
                                                .as_deref()
                                                .unwrap_or("Not set")
                                                .to_string(),
                                            cx
                                        )
                                    )
                                    .child(
                                        Button::new("browse-project-path")
                                            .ghost()
                                            .label("Browse")
                                            .icon(IconName::FolderOpen)
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
                            .render(cx)
                    )
            )
            .render(cx)
    }

    fn render_auto_save_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        SettingCard::new("Auto Save")
            .icon(IconName::Clock)
            .description("Automatically save your work")
            .child(
                v_flex()
                    .w_full()
                    .gap_4()
                    .pt_4()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        SettingRow::new("Enable Auto Save")
                            .description("Automatically save changes to files")
                            .control(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        Switch::new("auto-save-switch")
                                            .checked(self.settings.project.auto_save_interval > 0)
                                            .on_click(cx.listener(|screen, _, _window, cx| {
                                                if screen.settings.project.auto_save_interval > 0 {
                                                    screen.settings.project.auto_save_interval = 0;
                                                } else {
                                                    screen.settings.project.auto_save_interval = 30;
                                                }
                                                cx.notify();
                                            }))
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(if self.settings.project.auto_save_interval > 0 {
                                                theme.primary
                                            } else {
                                                theme.muted_foreground
                                            })
                                            .child(if self.settings.project.auto_save_interval > 0 { "Enabled" } else { "Disabled" })
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
                            .render(cx)
                    )
                    .when(self.settings.project.auto_save_interval > 0, |this| {
                        this.child(
                            div()
                                .w_full()
                                .h(px(1.0))
                                .bg(theme.border)
                        )
                        .child(
                            SettingRow::new("Save Interval")
                                .description("How often to auto-save (in seconds)")
                                .control(
                                    render_value_display(
                                        format!("{} seconds", self.settings.project.auto_save_interval),
                                        cx
                                    )
                                )
                                .render(cx)
                        )
                    })
                    .child(
                        div()
                            .p_4()
                            .rounded_lg()
                            .bg(hsla(theme.accent.h, theme.accent.s, theme.accent.l, 0.1))
                            .border_1()
                            .border_color(hsla(theme.accent.h, theme.accent.s, theme.accent.l, 0.2))
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_start()
                                    .child(
                                        Icon::new(IconName::Info)
                                            .size(px(20.0))
                                            .text_color(theme.accent)
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.foreground)
                                            .child("Auto-save will save all open files at the specified interval. This helps prevent data loss.")
                                    )
                            )
                    )
            )
            .render(cx)
    }

    fn render_backup_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        SettingCard::new("Backup Settings")
            .icon(IconName::Database)
            .description("Automatic project backup configuration")
            .child(
                v_flex()
                    .w_full()
                    .gap_4()
                    .pt_4()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        SettingRow::new("Enable Backups")
                            .description("Automatically create backups when saving projects")
                            .control(
                                h_flex()
                                    .gap_3()
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
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(if self.settings.project.enable_backups {
                                                theme.primary
                                            } else {
                                                theme.muted_foreground
                                            })
                                            .child(if self.settings.project.enable_backups { "Enabled" } else { "Disabled" })
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
                            .render(cx)
                    )
                    .when(self.settings.project.enable_backups, |this| {
                        this.child(
                            div()
                                .p_4()
                                .rounded_lg()
                                .bg(hsla(theme.primary.h, theme.primary.s, theme.primary.l, 0.1))
                                .border_1()
                                .border_color(hsla(theme.primary.h, theme.primary.s, theme.primary.l, 0.2))
                                .child(
                                    h_flex()
                                        .gap_3()
                                        .items_start()
                                        .child(
                                            Icon::new(IconName::CheckCircle)
                                                .size(px(20.0))
                                                .text_color(theme.primary)
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(theme.foreground)
                                                .child("Backups are stored in your project directory and are created automatically when you save.")
                                        )
                                )
                        )
                    })
            )
            .render(cx)
    }
}
