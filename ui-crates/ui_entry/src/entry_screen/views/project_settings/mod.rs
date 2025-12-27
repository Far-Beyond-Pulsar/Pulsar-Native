pub mod types;
pub mod helpers;
pub mod general;
pub mod git_info;
pub mod git_ci;
pub mod metadata;
pub mod disk_info;
pub mod performance;
pub mod integrations;

pub use types::{ProjectSettings, ProjectSettingsTab};
use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, Icon, IconName, ActiveTheme as _, StyledExt, divider::Divider,
    scroll::ScrollbarAxis,
};
use crate::entry_screen::EntryScreen;

pub fn render_project_settings(screen: &EntryScreen, settings: &ProjectSettings, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    
    div()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.background.opacity(0.95))
        .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
            // Close modal when clicking on background
            this.close_project_settings(cx);
        }))
        .child(
            h_flex()
                .w(px(1200.))
                .h(px(800.))
                .bg(theme.background)
                .rounded_xl()
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                    // Stop propagation for mouse down too
                    cx.stop_propagation();
                })
                .child(render_settings_sidebar(settings, cx))
                .child(render_settings_content(settings, cx))
        )
}

fn render_settings_sidebar(settings: &ProjectSettings, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let active_tab = settings.active_tab.clone();
    
    v_flex()
        .w(px(250.))
        .h_full()
        .bg(theme.sidebar)
        .border_r_1()
        .border_color(theme.border)
        .p_4()
        .gap_2()
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .mb_4()
                .child(
                    Icon::new(IconName::Settings)
                        .size(px(20.))
                        .text_color(theme.primary)
                )
                .child(
                    div()
                        .text_lg()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child("Project Settings")
                )
        )
        .child(Divider::horizontal())
        .child(
            v_flex()
                .gap_1()
                .mt_2()
                .child(render_sidebar_item("General", IconName::Folder, ProjectSettingsTab::General, &active_tab, cx))
                .child(render_sidebar_item("Git Info", IconName::GitHub, ProjectSettingsTab::GitInfo, &active_tab, cx))
                .child(render_sidebar_item("Git CI/CD", IconName::Settings, ProjectSettingsTab::GitCI, &active_tab, cx))
                .child(render_sidebar_item("Metadata", IconName::Folder, ProjectSettingsTab::Metadata, &active_tab, cx))
                .child(render_sidebar_item("Disk Info", IconName::HardDrive, ProjectSettingsTab::DiskInfo, &active_tab, cx))
                .child(render_sidebar_item("Performance", IconName::Activity, ProjectSettingsTab::Performance, &active_tab, cx))
                .child(render_sidebar_item("Integrations", IconName::Link, ProjectSettingsTab::Integrations, &active_tab, cx))
        )
        .child(
            v_flex()
                .flex_1()
                .justify_end()
                .child(
                    Button::new("close-settings")
                        .label("Close")
                        .w_full()
                        .with_variant(ui::button::ButtonVariant::Secondary)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_project_settings(cx);
                        }))
                )
        )
}

fn render_sidebar_item(label: &str, icon: IconName, tab: ProjectSettingsTab, active_tab: &ProjectSettingsTab, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let is_active = *active_tab == tab;
    let label_str = label.to_string();
    
    div()
        .w_full()
        .px_3()
        .py_2()
        .gap_2()
        .flex()
        .items_center()
        .rounded_md()
        .bg(if is_active { theme.primary.opacity(0.1) } else { gpui::transparent_black() })
        .border_1()
        .border_color(if is_active { theme.primary } else { gpui::transparent_black() })
        .hover(|this| {
            if !is_active {
                this.bg(theme.muted.opacity(0.1))
            } else {
                this
            }
        })
        .cursor_pointer()
        .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _, cx| {
            this.change_project_settings_tab(tab.clone(), cx);
        }))
        .child(
            Icon::new(icon)
                .size(px(16.))
                .text_color(if is_active { theme.primary } else { theme.muted_foreground })
        )
        .child(
            div()
                .text_sm()
                .font_weight(if is_active { gpui::FontWeight::SEMIBOLD } else { gpui::FontWeight::NORMAL })
                .text_color(if is_active { theme.primary } else { theme.foreground })
                .child(label_str)
        )
}

fn render_settings_content(settings: &ProjectSettings, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    v_flex()
        .flex_1()
        .h_full()
        .scrollable(ScrollbarAxis::Vertical)
        .p_8()
        .child(
            match settings.active_tab {
                ProjectSettingsTab::General => general::render_general_tab(settings, cx).into_any_element(),
                ProjectSettingsTab::GitInfo => git_info::render_git_info_tab(settings, cx).into_any_element(),
                ProjectSettingsTab::GitCI => git_ci::render_git_ci_tab(settings, cx).into_any_element(),
                ProjectSettingsTab::Metadata => metadata::render_metadata_tab(settings, cx).into_any_element(),
                ProjectSettingsTab::DiskInfo => disk_info::render_disk_info_tab(settings, cx).into_any_element(),
                ProjectSettingsTab::Performance => performance::render_performance_tab(settings, cx).into_any_element(),
                ProjectSettingsTab::Integrations => integrations::render_integrations_tab(settings, cx).into_any_element(),
            }
        )
}
