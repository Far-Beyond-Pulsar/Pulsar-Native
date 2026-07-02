use gpui::prelude::*;
use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, IconName, TitleBar,
};

use crate::screen::EntryScreen;
use crate::core::types::EntryScreenView;
use crate::screen::views::project_settings::ProjectSettingsTab;

pub fn render_layout(screen: &mut EntryScreen, window: &mut Window, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    if screen.state.ui.show_onboarding {
        return crate::screen::views::render_onboarding(screen, window, cx).into_any_element();
    }

    if screen.state.ui.show_dependency_setup {
        return crate::screen::views::render_dependency_setup(screen, window, cx).into_any_element();
    }

    if screen.state.ui.show_git_upstream_prompt.is_some() {
        return crate::screen::views::render_upstream_prompt(screen, window, cx).into_any_element();
    }

    if let Some(ref _settings) = screen.state.ui.project_settings {
        return crate::screen::views::render_project_settings(screen, window, cx).into_any_element();
    }

    let view = screen.state.ui.view;

    if view == EntryScreenView::Recent && !screen.state.is_fetching_updates {
        screen.start_git_fetch_all(cx);
    }

    v_flex()
        .size_full()
        .bg(cx.theme().background)
        .child(
            h_flex()
                .flex_1()
                .w_full()
                .overflow_hidden()
                .child(crate::screen::views::render_sidebar(screen, cx))
                .child(
                    v_flex()
                        .flex_1()
                        .h_full()
                        .overflow_hidden()
                        .bg(cx.theme().background)
                        .child(
                            TitleBar::new()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .px_2()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(cx.theme().muted_foreground)
                                                .child(view_title(view)),
                                        ),
                                ),
                        )
                        .child(match view {
                            EntryScreenView::Recent => {
                                let bounds = window.viewport_size();
                                let width: f32 = f32::from(bounds.width);
                                let available_width: f32 = (width - 220.0 - 64.0).max(0.0);
                                crate::screen::views::render_recent_projects(screen, available_width, cx)
                                    .into_any_element()
                            }
                            EntryScreenView::Templates => {
                                let bounds = window.viewport_size();
                                let width: f32 = f32::from(bounds.width);
                                let available_width: f32 = (width - 220.0 - 64.0).max(0.0);
                                crate::screen::views::render_templates(screen, available_width, cx)
                                    .into_any_element()
                            }
                            EntryScreenView::NewProject => {
                                crate::screen::views::render_new_project(screen, window, cx)
                                    .into_any_element()
                            }
                            EntryScreenView::CloneGit => {
                                crate::screen::views::render_clone_git(screen, window, cx)
                                    .into_any_element()
                            }
                            EntryScreenView::CloudProjects => {
                                crate::screen::views::render_cloud_projects(screen, window, cx)
                                    .into_any_element()
                            }
                            EntryScreenView::Friends => {
                                screen.state.friends_screen.clone().into_any_element()
                            }
                        }),
                ),
        )
        .when(screen.state.ui.auth_device_modal_visible, |this| {
            this.child(render_github_code_modal(screen, cx))
        })
        .into_any_element()
}

fn view_title(view: EntryScreenView) -> &'static str {
    match view {
        EntryScreenView::Recent => "Recent Projects",
        EntryScreenView::Templates => "Templates",
        EntryScreenView::NewProject => "New Project",
        EntryScreenView::CloneGit => "Clone Repository",
        EntryScreenView::CloudProjects => "Cloud Projects",
        EntryScreenView::Friends => "Friends",
    }
}

fn render_github_code_modal(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let Some(code) = screen.state.auth.device_code.clone() else {
        return div().into_any_element();
    };
    let verification_url = screen.state.auth.device_verification_url.clone();
    div()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(cx.theme().background.opacity(0.86))
        .child(
            v_flex()
                .w_full()
                .max_w(px(460.))
                .p_6()
                .gap_4()
                .rounded_xl()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().background)
                .shadow_lg()
                .child(
                    div()
                        .text_lg()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(cx.theme().foreground)
                        .child("GitHub Device Code"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Paste this 8-digit code in the browser window GitHub opened."),
                )
                .child(
                    div()
                        .w_full()
                        .py_3()
                        .rounded_lg()
                        .bg(cx.theme().accent.opacity(0.12))
                        .border_1()
                        .border_color(cx.theme().accent.opacity(0.35))
                        .text_center()
                        .text_2xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(cx.theme().foreground)
                        .child(code.clone()),
                )
                .when_some(screen.state.ui.auth_device_copy_notice.clone(), |this, notice| {
                    this.child(div().text_xs().text_color(cx.theme().success).child(notice))
                })
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("github-device-code-close")
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.ui.auth_device_modal_visible = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("github-device-code-open")
                                .ghost()
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    if let Some(url) = verification_url.clone() {
                                        cx.open_url(&url);
                                    }
                                })),
                        )
                        .child(
                            Button::new("github-device-code-copy")
                                .primary()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        code.clone(),
                                    ));
                                    this.state.ui.auth_device_copy_notice =
                                        Some("Code copied.".to_string());
                                    cx.notify();
                                })),
                        ),
                ),
        )
        .into_any_element()
}
