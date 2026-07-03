use gpui::prelude::*;
use gpui::*;
use ui::{button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme as _, Disableable, Icon, IconName};

use crate::screen::EntryScreen;
use crate::core::types::*;
use crate::util::formatters::format_size;

pub fn render_cloud_projects(screen: &mut EntryScreen, _window: &mut Window, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();

    if screen.state.ui.show_add_server {
        return render_add_server_form(screen, cx).into_any_element();
    }

    if let Some(server_idx) = screen.state.selected_cloud_server {
        if screen.state.ui.show_create_project {
            return render_create_project_form(screen, server_idx, cx).into_any_element();
        }
        return render_server_detail(screen, server_idx, cx).into_any_element();
    }

    render_server_list(screen, cx).into_any_element()
}

fn render_server_list(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let has_servers = !screen.state.cloud_servers.is_empty();

    v_flex()
        .flex_1()
        .h_full()
                .overflow_hidden()
        .px_8()
        .pt_6()
        .gap_6()
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .child(
                            div()
                                .text_xl()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(theme.foreground)
                                .child("Cloud Projects"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Connect to a remote Pulsar Host server"),
                        ),
                )
                .child(
                    Button::new("add-server-btn")
                        .label("Add Server")
                        .primary()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.ui.show_add_server = true;
                            cx.notify();
                        })),
                ),
        )
        .child(
            v_flex()
                .max_w(px(800.))
                .gap_4()
                .when(!has_servers, |this| {
                    this.child(
                        v_flex()
                            .items_center()
                            .justify_center()
                            .py_16()
                            .gap_3()
                            .child(
                                Icon::new(IconName::Cloud)
                                    .size(px(48.))
                                    .text_color(theme.muted_foreground.opacity(0.4)),
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .text_color(theme.muted_foreground)
                                    .child("No servers configured"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground.opacity(0.7))
                                    .child("Add a Pulsar Host server to start collaborating"),
                            ),
                    )
                })
                .children(
                    screen.state.cloud_servers.clone().iter().enumerate().map(|(idx, server)| {
                        render_server_card(screen, idx, server, cx)
                    })
                ),
        )
}

fn render_server_card(
    screen: &mut EntryScreen,
    idx: usize,
    server: &CloudServer,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let alias = server.alias.clone();
    let url = server.url.clone();
    let (status_color, status_label) = match &server.status {
        CloudServerStatus::Online { latency_ms, .. } => {
            (theme.success_foreground, format!("Online (\u{2264}{}ms)", latency_ms))
        }
        CloudServerStatus::Connecting => (theme.accent, "Connecting...".to_string()),
        CloudServerStatus::Offline => (theme.muted_foreground, "Offline".to_string()),
        CloudServerStatus::Unauthorized => (theme.warning, "Unauthorized".to_string()),
        CloudServerStatus::Unknown => (theme.muted_foreground, "Unknown".to_string()),
    };
    let active_users = match &server.status {
        CloudServerStatus::Online { active_users, .. } => *active_users,
        _ => 0,
    };
    let active_projects = match &server.status {
        CloudServerStatus::Online { active_projects, .. } => *active_projects,
        _ => 0,
    };

    v_flex()
        .id(SharedString::from(format!("cloud-server-{idx}")))
        .w_full()
        .p_4()
        .gap_3()
        .rounded_xl()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary.opacity(0.08))
        .cursor_pointer()
        .hover(|this| this.bg(theme.secondary.opacity(0.15)))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.select_cloud_server(idx, cx);
        }))
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    div().w(px(8.)).h(px(8.)).rounded_full().bg(status_color),
                )
                .child(
                    div()
                        .flex_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(alias),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(url),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(status_color)
                        .child(status_label),
                ),
        )
        .child(
            h_flex()
                .gap_4()
                .px_2()
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            Icon::new(IconName::Group)
                                .size(px(12.))
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("{} active", active_users)),
                        ),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            Icon::new(IconName::FolderClosed)
                                .size(px(12.))
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("{} projects", active_projects)),
                        ),
                )
                .child(div().flex_1())
                .child(
                    Button::new(SharedString::from(format!("remove-server-{idx}")))
                        .compact()
                        .ghost()
                        .icon(IconName::Trash)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remove_cloud_server(idx, cx);
                        })),
                ),
        )
}

fn render_add_server_form(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let alias_input = screen.inputs().add_server_alias.clone();
    let url_input = screen.inputs().add_server_url.clone();
    let email_input = screen.inputs().add_server_email.clone();
    let password_input = screen.inputs().add_server_password.clone();
    let is_logging_in = screen.state.add_server_logging_in;
    let error = screen.state.add_server_error.clone();

    v_flex()
        .flex_1()
        .h_full()
                .overflow_hidden()
        .px_8()
        .pt_6()
        .gap_6()
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    Button::new("back-to-servers")
                        .compact()
                        .ghost()
                        .icon(IconName::ChevronLeft)
                        .label("Back")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.ui.show_add_server = false;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child("Add Server"),
                ),
        )
        .child(
            v_flex()
                .max_w(px(600.))
                .gap_4()
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().font_weight(gpui::FontWeight::MEDIUM).text_color(theme.foreground).child("Alias"))
                        .child(ui::input::Input::new(&alias_input).w_full()),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().font_weight(gpui::FontWeight::MEDIUM).text_color(theme.foreground).child("Server URL"))
                        .child(ui::input::Input::new(&url_input).w_full()),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().font_weight(gpui::FontWeight::MEDIUM).text_color(theme.foreground).child("Email"))
                        .child(ui::input::Input::new(&email_input).w_full()),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().font_weight(gpui::FontWeight::MEDIUM).text_color(theme.foreground).child("Password"))
                        .child(ui::input::Input::new(&password_input).w_full()),
                )
                .when_some(error, |this, err| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(gpui::red())
                            .child(err),
                    )
                })
                .child(
                    Button::new("login-server-btn")
                        .label("Login & Add Server")
                        .primary()
                        .disabled(is_logging_in)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.add_cloud_server(cx);
                                })),
                ),
        )
}

fn render_server_detail(
    screen: &mut EntryScreen,
    server_idx: usize,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let server = &screen.state.cloud_servers[server_idx];
    let alias = server.alias.clone();
    let url = server.url.clone();
    let projects = server.projects.clone();

    v_flex()
        .flex_1()
        .h_full()
                .overflow_hidden()
        .px_8()
        .pt_6()
        .gap_6()
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    Button::new("back-to-server-list")
                        .compact()
                        .ghost()
                        .icon(IconName::ChevronLeft)
                        .label("Servers")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.selected_cloud_server = None;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .child(
                            div()
                                .text_xl()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(theme.foreground)
                                .child(alias),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(url),
                        ),
                )
                .child(
                    Button::new("create-cloud-project")
                        .label("New Project")
                        .primary()
                        .compact()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.ui.show_create_project = true;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("refresh-server")
                        .icon(IconName::Refresh)
                        .compact()
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.refresh_cloud_server(server_idx, cx);
                        })),
                ),
        )
        .child(
            v_flex()
                .max_w(px(800.))
                .gap_4()
                .when(projects.is_empty(), |this| {
                    this.child(
                        v_flex()
                            .items_center()
                            .py_16()
                            .gap_3()
                            .child(
                                Icon::new(IconName::Box)
                                    .size(px(48.))
                                    .text_color(theme.muted_foreground.opacity(0.4)),
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .text_color(theme.muted_foreground)
                                    .child("No projects"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground.opacity(0.7))
                                    .child("Create a new project on this server"),
                            ),
                    )
                })
                .children(
                    projects.into_iter().map(|project| {
                        render_project_card(screen, server_idx, project, cx)
                    }),
                ),
        )
}

fn render_project_card(
    screen: &mut EntryScreen,
    server_idx: usize,
    project: CloudProject,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let id = project.id;
    let open_id = id.clone();
    let prepare_id = id.clone();
    let stop_id = id.clone();
    let delete_id = id;
    let name = project.name.clone();
    let description = project.description.clone();
    let owner = project.owner.clone();
    let size = format_size(project.size_bytes);
    let last_modified = crate::util::formatters::format_timestamp(&project.last_modified);

    let (status_color, status_label) = match &project.status {
        CloudProjectStatus::Idle => (theme.muted_foreground, "Idle".to_string()),
        CloudProjectStatus::Preparing => (theme.accent, "Preparing...".to_string()),
        CloudProjectStatus::Running { user_count } => (theme.success_foreground, format!("Running ({})", user_count)),
        CloudProjectStatus::Error(e) => (gpui::red(), format!("Error: {}", e)),
    };

    v_flex()
        .id(SharedString::from(format!("cloud-project-{open_id}")))
        .w_full()
        .p_4()
        .gap_3()
        .rounded_xl()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary.opacity(0.08))
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(div().w(px(8.)).h(px(8.)).rounded_full().bg(status_color))
                .child(
                    div()
                        .flex_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(name),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(if description.is_empty() { owner } else { format!("{} \u{00B7} {}", description, owner) }),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .px_2()
                        .py(px(2.))
                        .rounded_full()
                        .bg(status_color.opacity(0.15))
                        .text_color(status_color)
                        .child(status_label),
                ),
        )
        .child(
            h_flex()
                .gap_4()
                .items_center()
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            Icon::new(IconName::HardDrive)
                                .size(px(12.))
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(size),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(last_modified),
                )
                .child(div().flex_1())
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new(SharedString::from(format!("open-cp-{open_id}")))
                                .compact()
                                .label("Open")
                                .primary()
                                .disabled(!matches!(project.status, CloudProjectStatus::Running { .. }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(proj_idx) = this.state.cloud_servers.get(server_idx)
                                        .and_then(|s| s.projects.iter().position(|p| p.id == open_id))
                                    {
                                        this.open_cloud_project(server_idx, proj_idx, cx);
                                    }
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("prepare-cp-{prepare_id}")))
                                .compact()
                                .label("Prepare")
                                .ghost()
                                .disabled(matches!(project.status, CloudProjectStatus::Preparing | CloudProjectStatus::Running { .. }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(proj_idx) = this.state.cloud_servers.get(server_idx)
                                        .and_then(|s| s.projects.iter().position(|p| p.id == prepare_id))
                                    {
                                        this.prepare_cloud_project(server_idx, proj_idx, cx);
                                    }
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("stop-cp-{stop_id}")))
                                .compact()
                                .label("Stop")
                                .ghost()
                                .disabled(!matches!(project.status, CloudProjectStatus::Running { .. }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(proj_idx) = this.state.cloud_servers.get(server_idx)
                                        .and_then(|s| s.projects.iter().position(|p| p.id == stop_id))
                                    {
                                        this.stop_cloud_project(server_idx, proj_idx, cx);
                                    }
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("delete-cp-{delete_id}")))
                                .compact()
                                .ghost()
                                .icon(IconName::Trash)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(proj_idx) = this.state.cloud_servers.get(server_idx)
                                        .and_then(|s| s.projects.iter().position(|p| p.id == delete_id))
                                    {
                                        this.delete_cloud_project(server_idx, proj_idx, cx);
                                    }
                                })),
                        ),
                ),
        )
}

fn render_create_project_form(screen: &mut EntryScreen, server_idx: usize, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let name_input = screen.inputs().create_project_name.clone();
    let desc_input = screen.inputs().create_project_description.clone();

    v_flex()
        .flex_1()
        .h_full()
                .overflow_hidden()
        .px_8()
        .pt_6()
        .gap_6()
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    Button::new("back-to-server-projects")
                        .compact()
                        .ghost()
                        .icon(IconName::ChevronLeft)
                        .label("Back")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.ui.show_create_project = false;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child("Create Cloud Project"),
                ),
        )
        .child(
            v_flex()
                .max_w(px(600.))
                .gap_4()
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().font_weight(gpui::FontWeight::MEDIUM).text_color(theme.foreground).child("Project Name"))
                        .child(ui::input::Input::new(&name_input).w_full()),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().font_weight(gpui::FontWeight::MEDIUM).text_color(theme.foreground).child("Description (optional)"))
                        .child(ui::input::Input::new(&desc_input).w_full()),
                )
                .child(
                    Button::new("create-cloud-project-btn")
                        .label("Create Project")
                        .primary()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.create_cloud_project(cx);
                        })),
                ),
        )
}
