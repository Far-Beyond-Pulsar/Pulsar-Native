//! Cloud Projects view — lists configured Pulsar Host servers and their projects.
//!
//! Layout flow:
//!  - Server list: grid of server cards + "Add Server" card/button.
//!  - Server detail (drill-down): header with back + refresh, then a project card grid.
//!  - Add-server panel: inline form shown over the server list.

use crate::entry_screen::{CloudProject, CloudProjectStatus, CloudServerStatus, EntryScreen};
use gpui::{prelude::*, *};
use ui::popup_menu::PopupMenuExt as _;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::TextInput,
    spinner::Spinner,
    tag::Tag,
    v_flex, ActiveTheme as _, Colorize as _, Icon, IconName, Sizable,
};

// ── Entry point ──────────────────────────────────────────────────────────────

pub fn render_cloud_projects(
    screen: &mut EntryScreen,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let show_add = screen.show_add_server;
    let show_create = screen.show_create_project;
    let selected = screen.selected_cloud_server;
    let srv_count = screen.cloud_servers.len();

    if show_add {
        render_add_server_panel(screen, cx)
    } else if show_create {
        if let Some(idx) = selected.filter(|&i| i < srv_count) {
            render_create_project_panel(screen, idx, cx)
        } else {
            render_server_list(screen, cx)
        }
    } else if let Some(idx) = selected.filter(|&i| i < srv_count) {
        render_server_detail(screen, idx, cx)
    } else {
        render_server_list(screen, cx)
    }
}

// ── Server list ───────────────────────────────────────────────────────────────

fn render_server_list(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) -> Div {
    let theme = cx.theme();
    let fg = theme.foreground;
    let muted_fg = theme.muted_foreground;

    let server_count = screen.cloud_servers.len();

    // Clone data needed inside closures before borrowing screen for sub-render
    let servers: Vec<(String, String, String, CloudServerStatus, usize)> = screen
        .cloud_servers
        .iter()
        .enumerate()
        .map(|(i, s)| {
            (
                s.alias.clone(),
                s.url.clone(),
                s.id.clone(),
                s.status.clone(),
                i,
            )
        })
        .collect();

    v_flex()
        .size_full()
        .gap_8()
        .p_8()
        // ── Page header ──
        .child(
            h_flex()
                .items_start()
                .gap_4()
                .child(
                    v_flex()
                        .flex_1()
                        .gap_1()
                        .child(
                            div()
                                .text_3xl()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(fg)
                                .child("Cloud Projects"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_fg)
                                .child(if server_count == 0 {
                                    "Connect to a Pulsar Host server to browse and prepare shared projects.".to_string()
                                } else {
                                    format!(
                                        "{} studio server{} configured",
                                        server_count,
                                        if server_count == 1 { "" } else { "s" }
                                    )
                                }),
                        ),
                )
                .child(
                    Button::new("add-server-btn")
                        .icon(IconName::Plus)
                        .label("Add Server")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_add_server = true;
                            cx.notify();
                        })),
                ),
        )
        // ── Empty state ──
        .when(server_count == 0, |this| {
            this.child(render_empty_state(cx))
        })
        // ── Server cards grid ──
        .when(server_count > 0, |this| {
            this.child(
                h_flex()
                    .flex_wrap()
                    .gap_5()
                    .children(
                        servers.into_iter().map(|(alias, url, _id, status, idx)| {
                            render_server_card(alias, url, status, idx, cx)
                        }),
                    ),
            )
        })
}

fn render_empty_state(cx: &mut Context<EntryScreen>) -> Div {
    let theme = cx.theme();
    let muted_fg = theme.muted_foreground;
    let border = theme.border;

    v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_4()
        .p_16()
        .child(
            div()
                .w(px(64.))
                .h(px(64.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_2xl()
                .bg(theme.sidebar)
                .border_1()
                .border_color(border)
                .child(Icon::new(IconName::Server).size(px(28.)).text_color(muted_fg)),
        )
        .child(
            div()
                .text_xl()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child("No servers yet"),
        )
        .child(
            div()
                .text_sm()
                .text_color(muted_fg)
                .max_w(px(400.))
                .text_center()
                .child("Add a Pulsar Host server to browse shared projects and collaborate in real time with your team."),
        )
}

fn render_server_card(
    alias: String,
    url: String,
    status: CloudServerStatus,
    idx: usize,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let fg = theme.foreground;
    let muted_fg = theme.muted_foreground;
    let border = theme.border;
    let sidebar = theme.sidebar;
    let accent = theme.accent;

    let hover_bg = sidebar.lighten(0.04);

    // Status bar colour and indicator dot colour
    let (bar_color, dot_color, status_label): (Hsla, Hsla, &'static str) = match &status {
        CloudServerStatus::Online { .. } => (theme.success, theme.success, "Online"),
        CloudServerStatus::Offline => (theme.danger, theme.danger, "Offline"),
        CloudServerStatus::Unauthorized => (theme.warning, theme.warning, "Auth Error"),
        CloudServerStatus::Connecting => (theme.info, theme.info, "Connecting…"),
        CloudServerStatus::Unknown => (muted_fg, muted_fg, "Unknown"),
    };

    // Stats from Online variant
    let (latency, version, active_users, active_projects) = match &status {
        CloudServerStatus::Online {
            latency_ms,
            version,
            active_users,
            active_projects,
        } => (
            Some(*latency_ms),
            Some(version.clone()),
            *active_users,
            *active_projects,
        ),
        _ => (None, None, 0, 0),
    };

    let is_connecting = matches!(status, CloudServerStatus::Connecting);

    div()
        .id(SharedString::from(format!("cloud-srv-{}", idx)))
        .w(px(280.))
        .relative()
        .overflow_hidden()
        .rounded_xl()
        .border_1()
        .border_color(border)
        .bg(sidebar)
        .shadow_sm()
        .cursor_pointer()
        .hover(|this| this.bg(hover_bg).shadow_md())
        // Accent bar at top
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h(px(3.))
                .bg(bar_color),
        )
        // Card content
        .child(
            v_flex()
                .w_full()
                .p_4()
                .pt_5()
                .gap_3()
                // ── Header row: alias + remove button ──
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        // Status dot
                        .child(
                            div()
                                .flex_shrink_0()
                                .w(px(8.))
                                .h(px(8.))
                                .rounded_full()
                                .bg(dot_color),
                        )
                        .when(is_connecting, |this| {
                            this.child(Spinner::new().small().color(dot_color))
                        })
                        // Alias
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(fg)
                                .overflow_hidden()
                                .child(alias),
                        )
                        // Remove ×
                        .child(
                            div()
                                .id(SharedString::from(format!("remove-srv-{}", idx)))
                                .flex_shrink_0()
                                .w(px(20.))
                                .h(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .text_color(muted_fg)
                                .hover(|this| {
                                    this.bg(theme.danger.opacity(0.12)).text_color(theme.danger)
                                })
                                .cursor_pointer()
                                .child(Icon::new(IconName::Xmark).size(px(12.)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_cloud_server(idx, cx);
                                })),
                        ),
                )
                // ── URL ──
                .child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .child(
                            Icon::new(IconName::Globe)
                                .size(px(11.))
                                .text_color(muted_fg),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_fg)
                                .overflow_hidden()
                                .child(url),
                        ),
                )
                // ── Stats row (only when online) ──
                .when(matches!(status, CloudServerStatus::Online { .. }), |this| {
                    this.child(
                        h_flex()
                            .gap_3()
                            .pt_1()
                            .border_t_1()
                            .border_color(border)
                            .child(stat_chip(
                                IconName::Group,
                                &format!("{} users", active_users),
                                muted_fg,
                                cx,
                            ))
                            .child(stat_chip(
                                IconName::Database,
                                &format!("{} projects", active_projects),
                                muted_fg,
                                cx,
                            )),
                    )
                })
                // ── Footer: latency + version ──
                .when(latency.is_some() || version.is_some(), |this| {
                    this.child(
                        h_flex()
                            .items_center()
                            .gap_3()
                            .when_some(latency, |t, ms| {
                                t.child(
                                    h_flex()
                                        .gap_1()
                                        .items_center()
                                        .child(
                                            Icon::new(IconName::Activity)
                                                .size(px(11.))
                                                .text_color(muted_fg),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(muted_fg)
                                                .child(format!("{}ms", ms)),
                                        ),
                                )
                            })
                            .when_some(version, |t, v| {
                                t.child(
                                    div()
                                        .text_xs()
                                        .text_color(muted_fg)
                                        .child(format!("v{}", v)),
                                )
                            }),
                    )
                })
                // ── Status badge ──
                .child(
                    h_flex().child(match &status {
                        CloudServerStatus::Online { .. } => Tag::success()
                            .xsmall()
                            .rounded_full()
                            .child(status_label)
                            .into_any_element(),
                        CloudServerStatus::Offline => Tag::danger()
                            .xsmall()
                            .rounded_full()
                            .child(status_label)
                            .into_any_element(),
                        CloudServerStatus::Unauthorized => Tag::warning()
                            .xsmall()
                            .rounded_full()
                            .child(status_label)
                            .into_any_element(),
                        CloudServerStatus::Connecting => Tag::info()
                            .xsmall()
                            .rounded_full()
                            .child(status_label)
                            .into_any_element(),
                        CloudServerStatus::Unknown => Tag::secondary()
                            .xsmall()
                            .rounded_full()
                            .child(status_label)
                            .into_any_element(),
                    }),
                ),
        )
        // Entire card navigates to detail view (registers on the outer div)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.selected_cloud_server = Some(idx);
            this.refresh_cloud_server(idx, cx);
            cx.notify();
        }))
}

/// Tiny icon + text chip used for stats.
fn stat_chip(icon: IconName, label: &str, color: Hsla, _cx: &mut Context<EntryScreen>) -> Div {
    h_flex()
        .gap_1()
        .items_center()
        .child(Icon::new(icon).size(px(11.)).text_color(color))
        .child(div().text_xs().text_color(color).child(label.to_string()))
}

// ── Server detail (project list) ──────────────────────────────────────────────

fn render_server_detail(
    screen: &mut EntryScreen,
    server_idx: usize,
    cx: &mut Context<EntryScreen>,
) -> Div {
    let theme = cx.theme();
    let fg = theme.foreground;
    let muted_fg = theme.muted_foreground;
    let border = theme.border;
    let accent = theme.accent;

    // Snapshot all data we need before handing cx to closures
    let alias = screen.cloud_servers[server_idx].alias.clone();
    let url = screen.cloud_servers[server_idx].url.clone();
    let status = screen.cloud_servers[server_idx].status.clone();
    let projects: Vec<CloudProject> = screen.cloud_servers[server_idx].projects.clone();

    let is_online = matches!(status, CloudServerStatus::Online { .. });
    let is_connecting = matches!(status, CloudServerStatus::Connecting);

    let (active_users, active_projects) = match &status {
        CloudServerStatus::Online {
            active_users,
            active_projects,
            ..
        } => (*active_users, *active_projects),
        _ => (0, 0),
    };

    v_flex()
        .size_full()
        .gap_6()
        .p_8()
        // ── Breadcrumb / back ──
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    h_flex()
                        .id("back-to-servers")
                        .gap_1p5()
                        .items_center()
                        .cursor_pointer()
                        .text_color(muted_fg)
                        .hover(|this| this.text_color(fg))
                        .child(
                            Icon::new(IconName::NavArrowLeft)
                                .size(px(14.))
                                .text_color(muted_fg),
                        )
                        .child(div().text_sm().child("Cloud Servers"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.selected_cloud_server = None;
                            cx.notify();
                        })),
                )
                .child(div().text_sm().text_color(muted_fg).child("/"))
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(fg)
                        .child(alias.clone()),
                ),
        )
        // ── Server status bar ──
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap_4()
                .p_4()
                .rounded_xl()
                .bg(theme.sidebar)
                .border_1()
                .border_color(border)
                .child(
                    div().flex_1().child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(fg)
                                    .child(alias.clone()),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        Icon::new(IconName::Globe)
                                            .size(px(11.))
                                            .text_color(muted_fg),
                                    )
                                    .child(div().text_xs().text_color(muted_fg).child(url)),
                            ),
                    ),
                )
                // Status badge + stats
                .child(
                    h_flex()
                        .gap_4()
                        .items_center()
                        .when(is_connecting, |this| {
                            this.child(Spinner::new().small().color(theme.info))
                        })
                        .child(match &status {
                            CloudServerStatus::Online { .. } => Tag::success()
                                .xsmall()
                                .rounded_full()
                                .child("Online")
                                .into_any_element(),
                            CloudServerStatus::Offline => Tag::danger()
                                .xsmall()
                                .rounded_full()
                                .child("Offline")
                                .into_any_element(),
                            CloudServerStatus::Unauthorized => Tag::warning()
                                .xsmall()
                                .rounded_full()
                                .child("Auth Error")
                                .into_any_element(),
                            CloudServerStatus::Connecting => Tag::info()
                                .xsmall()
                                .rounded_full()
                                .child("Connecting")
                                .into_any_element(),
                            CloudServerStatus::Unknown => Tag::secondary()
                                .xsmall()
                                .rounded_full()
                                .child("Unknown")
                                .into_any_element(),
                        })
                        .when(is_online, |this| {
                            this.child(stat_chip(
                                IconName::Group,
                                &format!("{} users", active_users),
                                muted_fg,
                                cx,
                            ))
                            .child(stat_chip(
                                IconName::Database,
                                &format!("{} projects", active_projects),
                                muted_fg,
                                cx,
                            ))
                        }),
                )
                // New Project button — primary action
                .child(
                    Button::new("new-project-btn")
                        .icon(IconName::Plus)
                        .label("New Project")
                        .primary()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.show_create_project = true;
                            cx.notify();
                        })),
                )
                // Refresh button
                .child(
                    Button::new("refresh-server")
                        .icon(IconName::Refresh)
                        .label("Refresh")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.refresh_cloud_server(server_idx, cx);
                        })),
                ),
        )
        // ── Empty projects state ──
        .when(projects.is_empty() && !is_connecting, |this| {
            this.child(
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .p_12()
                    .child(
                        Icon::new(IconName::FolderClosed)
                            .size(px(32.))
                            .text_color(muted_fg),
                    )
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(muted_fg)
                            .child(if is_online {
                                "No projects on this server"
                            } else {
                                "Could not connect to server"
                            }),
                    ),
            )
        })
        // ── Project cards ──
        .when(!projects.is_empty(), |this| {
            this.child(
                h_flex().flex_wrap().gap_5().children(
                    projects
                        .into_iter()
                        .map(|project| render_project_card(project, server_idx, cx)),
                ),
            )
        })
}

fn render_project_card(
    project: CloudProject,
    server_idx: usize,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let fg = theme.foreground;
    let muted_fg = theme.muted_foreground;
    let border = theme.border;
    let sidebar = theme.sidebar;

    let hover_bg = sidebar.lighten(0.04);

    let project_id = project.id.clone();
    let is_running = matches!(&project.status, CloudProjectStatus::Running { .. });
    let is_preparing = matches!(&project.status, CloudProjectStatus::Preparing);

    // Weak handle for popup-menu closures (must be 'static).
    let weak = cx.entity().downgrade();

    // Status colours
    let (bar_color, status_tag): (Hsla, AnyElement) = match &project.status {
        CloudProjectStatus::Running { user_count } => (
            theme.success,
            Tag::success()
                .xsmall()
                .rounded_full()
                .child(format!(
                    "Running  •  {} editor{}",
                    user_count,
                    if *user_count == 1 { "" } else { "s" }
                ))
                .into_any_element(),
        ),
        CloudProjectStatus::Preparing => (
            theme.info,
            h_flex()
                .gap_1p5()
                .items_center()
                .child(Spinner::new().small().color(theme.info))
                .child(Tag::info().xsmall().rounded_full().child("Preparing"))
                .into_any_element(),
        ),
        CloudProjectStatus::Error(msg) => (
            theme.danger,
            Tag::danger()
                .xsmall()
                .rounded_full()
                .child(format!("Error: {}", msg))
                .into_any_element(),
        ),
        CloudProjectStatus::Idle => (
            theme.muted,
            Tag::secondary()
                .xsmall()
                .rounded_full()
                .child("Idle")
                .into_any_element(),
        ),
    };

    let size_str = format_bytes(project.size_bytes);

    // ── "⋯" overflow menu ────────────────────────────────────────────────────
    let pid_menu = project_id.clone();
    let weak_menu = weak.clone();
    let more_button = Button::new(SharedString::from(format!("proj-more-{}", project.id)))
        .icon(IconName::MoreHoriz)
        .ghost()
        .xsmall()
        .popup_menu_with_anchor(Corner::BottomRight, move |menu, _window, _cx| {
            let pid = pid_menu.clone();
            let w = weak_menu.clone();

            // Open / Prepare — mutually exclusive depending on status
            let menu = if is_running {
                let (w2, pid2) = (w.clone(), pid.clone());
                menu.menu_handler_with_icon(
                    "Open in Editor",
                    IconName::OpenInWindow,
                    move |_, app| {
                        if let Some(e) = w2.upgrade() {
                            e.update(app, |this, cx| {
                                this.open_cloud_project(server_idx, pid2.clone(), cx)
                            });
                        }
                    },
                )
            } else {
                let (w2, pid2) = (w.clone(), pid.clone());
                menu.menu_handler_with_icon("Prepare / Warm Up", IconName::Play, move |_, app| {
                    if let Some(e) = w2.upgrade() {
                        e.update(app, |this, cx| {
                            this.prepare_cloud_project(server_idx, pid2.clone(), cx)
                        });
                    }
                })
            };

            // Stop — only when active
            let menu = if is_running || is_preparing {
                let (w2, pid2) = (w.clone(), pid.clone());
                menu.menu_handler_with_icon("Stop Project", IconName::SystemShut, move |_, app| {
                    if let Some(e) = w2.upgrade() {
                        e.update(app, |this, cx| {
                            this.stop_cloud_project(server_idx, pid2.clone(), cx)
                        });
                    }
                })
            } else {
                menu
            };

            // Separator + Delete (always available)
            let (w2, pid2) = (w.clone(), pid.clone());
            menu.separator().menu_handler_with_icon(
                "Delete Project",
                IconName::Trash,
                move |_, app| {
                    if let Some(e) = w2.upgrade() {
                        e.update(app, |this, cx| {
                            this.delete_cloud_project(server_idx, pid2.clone(), cx)
                        });
                    }
                },
            )
        });

    // ── Card root ─────────────────────────────────────────────────────────────
    let pid_click = project_id.clone();
    div()
        .id(SharedString::from(format!("cloud-proj-{}", project.id)))
        .w(px(300.))
        .relative()
        .overflow_hidden()
        .rounded_xl()
        .border_1()
        .border_color(border)
        .bg(sidebar)
        .shadow_sm()
        .when(!is_preparing, |this| this.cursor_pointer())
        .when(is_preparing, |this| this.cursor_default())
        .hover(|this| this.bg(hover_bg).shadow_md())
        // Accent bar
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h(px(3.))
                .bg(bar_color),
        )
        // ⋯ button — top-right corner
        .child(div().absolute().top_1().right_1().child(more_button))
        .child(
            v_flex()
                .w_full()
                .p_4()
                .pt_5()
                .gap_3()
                // ── Project name ──
                .child(
                    div()
                        .text_base()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(fg)
                        .child(project.name.clone()),
                )
                // ── Description ──
                .when(!project.description.is_empty(), |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(muted_fg)
                            .overflow_hidden()
                            .child(project.description.clone()),
                    )
                })
                // ── Status badge ──
                .child(status_tag)
                // ── Meta row ──
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .pt_1()
                        .border_t_1()
                        .border_color(border)
                        .when(!project.last_modified.is_empty(), |this| {
                            this.child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        Icon::new(IconName::Clock)
                                            .size(px(11.))
                                            .text_color(muted_fg),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_fg)
                                            .child(project.last_modified.clone()),
                                    ),
                            )
                        })
                        .when(project.size_bytes > 0, |this| {
                            this.child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        Icon::new(IconName::HardDrive)
                                            .size(px(11.))
                                            .text_color(muted_fg),
                                    )
                                    .child(div().text_xs().text_color(muted_fg).child(size_str)),
                            )
                        }),
                )
                // ── Owner ──
                .when(!project.owner.is_empty(), |this| {
                    this.child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(
                                Icon::new(IconName::UserCircle)
                                    .size(px(11.))
                                    .text_color(muted_fg),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child(project.owner.clone()),
                            ),
                    )
                }),
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            if is_preparing {
                return;
            }
            if is_running {
                this.open_cloud_project(server_idx, pid_click.clone(), cx);
            } else {
                this.prepare_cloud_project(server_idx, pid_click.clone(), cx);
            }
        }))
}

// ── Create project panel ─────────────────────────────────────────────────────

fn render_create_project_panel(
    screen: &mut EntryScreen,
    server_idx: usize,
    cx: &mut Context<EntryScreen>,
) -> Div {
    let theme = cx.theme();
    let fg = theme.foreground;
    let muted_fg = theme.muted_foreground;
    let border = theme.border;

    let alias = screen.cloud_servers[server_idx].alias.clone();

    v_flex()
        .size_full()
        .p_8()
        .gap_8()
        // ── Header / breadcrumb ──
        .child(
            h_flex()
                .items_center()
                .gap_3()
                .child(
                    h_flex()
                        .id("back-from-create-project")
                        .gap_1p5()
                        .items_center()
                        .cursor_pointer()
                        .text_color(muted_fg)
                        .hover(|this| this.text_color(fg))
                        .child(
                            Icon::new(IconName::NavArrowLeft)
                                .size(px(14.))
                                .text_color(muted_fg),
                        )
                        .child(div().text_sm().child(alias))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_create_project = false;
                            cx.notify();
                        })),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(fg)
                        .child("Create New Project"),
                ),
        )
        // ── Form card ──
        .child(
            v_flex()
                .w(px(520.))
                .gap_6()
                .p_8()
                .rounded_2xl()
                .border_1()
                .border_color(border)
                .bg(theme.sidebar)
                .shadow_sm()
                // Project Name
                .child(
                    v_flex()
                        .gap_1p5()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(fg)
                                .child("Project Name"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_fg)
                                .child("A unique name for the project on this server"),
                        )
                        .child(TextInput::new(&screen.create_project_name_input)),
                )
                // Description (optional)
                .child(
                    v_flex()
                        .gap_1p5()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(fg)
                                        .child("Description"),
                                )
                                .child(Tag::secondary().xsmall().rounded_full().child("Optional")),
                        )
                        .child(TextInput::new(&screen.create_project_description_input)),
                )
                // ── Actions ──
                .child(
                    h_flex()
                        .pt_2()
                        .gap_3()
                        .justify_end()
                        .child(
                            Button::new("cancel-create-project")
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_create_project = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("confirm-create-project")
                                .icon(IconName::Plus)
                                .label("Create Project")
                                .primary()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let name = this.create_project_name.trim().to_string();
                                    let desc = this.create_project_description.trim().to_string();
                                    this.create_cloud_project(server_idx, name, desc, cx);
                                })),
                        ),
                ),
        )
}

// ── Add server panel ──────────────────────────────────────────────────────────

fn render_add_server_panel(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) -> Div {
    let theme = cx.theme();
    let fg = theme.foreground;
    let muted_fg = theme.muted_foreground;
    let border = theme.border;
    let primary = theme.primary;

    v_flex()
        .size_full()
        .p_8()
        .gap_8()
        // ── Header ──
        .child(
            h_flex()
                .items_center()
                .gap_3()
                .child(
                    h_flex()
                        .id("back-from-add-server")
                        .gap_1p5()
                        .items_center()
                        .cursor_pointer()
                        .text_color(muted_fg)
                        .hover(|this| this.text_color(fg))
                        .child(
                            Icon::new(IconName::NavArrowLeft)
                                .size(px(14.))
                                .text_color(muted_fg),
                        )
                        .child(div().text_sm().child("Cancel"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_add_server = false;
                            cx.notify();
                        })),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(fg)
                        .child("Add Cloud Server"),
                ),
        )
        // ── Form card ──
        .child(
            v_flex()
                .w(px(520.))
                .gap_6()
                .p_8()
                .rounded_2xl()
                .border_1()
                .border_color(border)
                .bg(theme.sidebar)
                .shadow_sm()
                // Server Alias
                .child(
                    v_flex()
                        .gap_1p5()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(fg)
                                .child("Server Alias"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_fg)
                                .child("A friendly name to identify this server"),
                        )
                        .child(TextInput::new(&screen.add_server_alias_input)),
                )
                // Server URL
                .child(
                    v_flex()
                        .gap_1p5()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(fg)
                                .child("Server URL"),
                        )
                        .child(div().text_xs().text_color(muted_fg).child(
                            "Base URL of the Pulsar Host server, e.g. https://studio.example.com",
                        ))
                        .child(TextInput::new(&screen.add_server_url_input)),
                )
                // Auth Token (optional)
                .child(
                    v_flex()
                        .gap_1p5()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(fg)
                                        .child("Auth Token"),
                                )
                                .child(Tag::secondary().xsmall().rounded_full().child("Optional")),
                        )
                        .child(div().text_xs().text_color(muted_fg).child(
                            "Bearer token for authenticated servers. Leave blank for open servers.",
                        ))
                        .child(TextInput::new(&screen.add_server_token_input)),
                )
                // ── Actions ──
                .child(
                    h_flex()
                        .pt_2()
                        .gap_3()
                        .justify_end()
                        .child(
                            Button::new("cancel-add")
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_add_server = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("confirm-add")
                                .icon(IconName::Plus)
                                .label("Add Server")
                                .primary()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let alias = this.add_server_alias.trim().to_string();
                                    let url = this.add_server_url.trim().to_string();
                                    let token = this.add_server_token.trim().to_string();
                                    this.add_cloud_server(alias, url, token, cx);
                                })),
                        ),
                ),
        )
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn format_bytes(bytes: u64) -> String {
    const GB: u64 = 1_073_741_824;
    const MB: u64 = 1_048_576;
    const KB: u64 = 1_024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.0} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
