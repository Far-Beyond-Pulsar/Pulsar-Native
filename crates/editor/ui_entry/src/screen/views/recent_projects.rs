use gpui::prelude::*;
use gpui::*;
use std::path::PathBuf;
use ui::{
    button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme as _, Icon, IconName,
    StyledExt,
};

use crate::core::types::{EntryScreenView, GitFetchStatus};
use crate::screen::EntryScreen;
use crate::util::formatters::format_timestamp;
use crate::util::path_helpers::normalize_project_path;

pub fn render_recent_projects(
    screen: &mut EntryScreen,
    available_width: f32,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let columns = screen.calculate_columns(px(available_width + 220.0 + 64.0));

    let project_count = screen.state.recent_projects.projects.len();
    let is_empty = project_count == 0;

    v_flex()
        .flex_1()
        .h_full()
        .overflow_hidden()
        .child(
            h_flex()
                .w_full()
                .px_8()
                .pt_6()
                .pb_4()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .text_2xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child(format!("Recent Projects")),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "{} project{}",
                            project_count,
                            if project_count == 1 { "" } else { "s" }
                        )),
                )
                .child(
                    Button::new("refresh-projects")
                        .icon(IconName::Refresh)
                        .compact()
                        .ghost()
                        .tooltip("Check for git updates")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.start_git_fetch_all(cx);
                        })),
                )
                .child(
                    Button::new("open-folder-btn")
                        .label("Open Folder")
                        .primary()
                        .compact()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_folder_dialog(cx);
                        })),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .when(is_empty, |this| {
                    this.child(
                        v_flex()
                            .size_full()
                            .items_center()
                            .justify_center()
                            .gap_3()
                            .child(
                                Icon::new(IconName::Folder)
                                    .size(px(48.))
                                    .text_color(theme.muted_foreground.opacity(0.4)),
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .text_color(theme.muted_foreground)
                                    .child("No recent projects"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground.opacity(0.7))
                                    .child("Open a folder or create a new project to get started"),
                            ),
                    )
                })
                .when(!is_empty, |this| {
                    this.child(
                    v_flex()
                        .id("recent-projects-scroll")
                        .flex_1()
                        .min_h_0()
                        .scrollable(gpui::Axis::Vertical)
                        .px_8()
                        .pb_6()
                            .child(h_flex().flex_wrap().gap_6().children(
                                screen.state.recent_projects.projects.clone().iter().map(
                                    |project| render_project_card(screen, project, columns, cx),
                                ),
                            )),
                    )
                }),
        )
}

fn render_project_card(
    screen: &mut EntryScreen,
    project: &crate::service::project_service::RecentProject,
    _columns: usize,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let path = project.path.clone();
    let path_open = path.clone();
    let path_git = path.clone();
    let path_settings = path.clone();
    let path_remove = path.clone();
    let name = project.name.clone();
    let normalized = normalize_project_path(&path);
    let timestamp = project.last_opened.as_deref().unwrap_or("").to_string();
    let formatted_time = format_timestamp(&timestamp);
    let is_git = project.is_git;

    let fetch_status = {
        let statuses = screen.state.git_fetch_statuses.lock();
        statuses
            .get(&path)
            .cloned()
            .unwrap_or(GitFetchStatus::NotStarted)
    };

    let thumbnail = screen
        .state
        .project_thumbnails
        .get(&path)
        .and_then(|t| t.clone());

    v_flex()
        .id(SharedString::from(format!("project-card-{}", path)))
        .w(px(320.))
        .rounded_xl()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary.opacity(0.08))
        .overflow_hidden()
        .cursor_pointer()
        .hover(|this| {
            this.bg(theme.secondary.opacity(0.15))
                .border_color(theme.accent.opacity(0.4))
        })
        .on_click(cx.listener(move |this, _, window, cx| {
            if !window.default_prevented() {
                this.launch_project(std::path::PathBuf::from(&path_open), cx);
            }
        }))
        .child(
            div()
                .w_full()
                .h(px(140.))
                .relative()
                .overflow_hidden()
                .bg(theme.secondary.opacity(0.2))
                .rounded_t_xl()
                .group("card-image")
                .when_some(thumbnail.clone(), |this, render_img| {
                    this.child(
                        gpui::img(ImageSource::Render(render_img))
                            .w_full()
                            .h_full()
                            .rounded_t_xl()
                            .object_fit(gpui::ObjectFit::Cover),
                    )
                })
                .when(thumbnail.is_none(), |this| {
                    this.child(
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::Folder)
                                    .size(px(36.))
                                    .text_color(theme.muted_foreground.opacity(0.3)),
                            ),
                    )
                })
                .child(
                    h_flex()
                        .absolute()
                        .top_2()
                        .right_2()
                        .gap_1()
                        .opacity(0.0)
                        .group_hover("card-image", |this| this.opacity(1.0))
                        .capture_any_mouse_up(|_, window, _| {
                            window.prevent_default();
                        })
                        .child(
                            Button::new(SharedString::from(format!("git-{}", path)))
                                .icon(IconName::GitBranch)
                                .compact()
                                .ghost()
                                .tooltip("Git Manager")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_git_manager(std::path::PathBuf::from(&path_git), cx);
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("settings-{}", path)))
                                .icon(IconName::Settings)
                                .compact()
                                .ghost()
                                .tooltip("Project Settings")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_project_settings(
                                        std::path::PathBuf::from(&path_settings),
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("remove-{}", path)))
                                .icon(IconName::Close)
                                .compact()
                                .ghost()
                                .tooltip("Remove from recent")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_recent_project(&path_remove, cx);
                                })),
                        ),
                ),
        )
        .child(
            v_flex()
                .p_4()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .truncate()
                        .child(name),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .truncate()
                        .child(normalized),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground.opacity(0.7))
                                .child(formatted_time),
                        )
                        .when(is_git, |this| {
                            this.child(
                                h_flex()
                                    .flex_1()
                                    .justify_end()
                                    .gap_1()
                                    .items_center()
                                    .child(render_git_status(
                                        fetch_status,
                                        path.clone(),
                                        screen,
                                        cx,
                                    )),
                            )
                        }),
                ),
        )
}

fn render_git_status(
    status: GitFetchStatus,
    path: String,
    screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    h_flex().gap_2().children(match status {
        GitFetchStatus::Fetching => {
            vec![
                Icon::new(IconName::Refresh)
                    .size(px(12.))
                    .into_any_element(),
                div()
                    .text_xs()
                    .text_color(theme.foreground)
                    .child("fetching")
                    .into_any_element(),
            ]
        }
        GitFetchStatus::UpToDate => {
            vec![Icon::new(IconName::Check)
                .size(px(12.))
                .text_color(theme.success_foreground)
                .into_any_element()]
        }
        GitFetchStatus::UpdatesAvailable(count) => {
            vec![
                Icon::new(IconName::ArrowUp)
                    .size(px(12.))
                    .text_color(theme.warning)
                    .into_any_element(),
                div()
                    .text_xs()
                    .text_color(theme.warning)
                    .child(format!(
                        "{} update{}",
                        count,
                        if count == 1 { "" } else { "s" }
                    ))
                    .into_any_element(),
                Button::new(SharedString::from(format!("pull-{}", path)))
                    .compact()
                    .ghost()
                    .icon(IconName::Download)
                    .tooltip("Pull updates")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pull_project_updates(PathBuf::from(path.clone()), cx);
                    }))
                    .into_any_element(),
            ]
        }
        GitFetchStatus::NotStarted => {
            vec![]
        }
        GitFetchStatus::Error(e) => {
            vec![
                Icon::new(IconName::WarningTriangle)
                    .size(px(12.))
                    .text_color(gpui::red())
                    .into_any_element(),
                div()
                    .text_xs()
                    .text_color(gpui::red())
                    .child(div().child(e))
                    .into_any_element(),
            ]
        }
    })
}
