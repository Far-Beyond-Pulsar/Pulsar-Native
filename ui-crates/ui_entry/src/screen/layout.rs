use gpui::prelude::*;
use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Icon, IconName, TitleBar,
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
                                .child(div().flex_1())
                                .child({
                                    let theme_picker = screen.state.theme_picker.clone();
                                    h_flex()
                                        .flex()
                                        .items_center()
                                        .px_2()
                                        .gap_2()
                                        .child(screen.state.auth.profile_dropdown.clone())
                                        .child(
                                            ui::popover::Popover::<ui_common::ThemePicker>::new("titlebar-theme-popover")
                                                .anchor(Corner::TopRight)
                                                .trigger(
                                                    Button::new("titlebar-theme-toggle")
                                                        .icon(IconName::Palette)
                                                        .compact()
                                                        .ghost()
                                                        .tooltip("Switch theme"),
                                                )
                                                .content(move |_, _| theme_picker.clone()),
                                        )
                                }),
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


