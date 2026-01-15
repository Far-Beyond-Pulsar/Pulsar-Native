use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, Icon, IconName, ActiveTheme as _, StyledExt, divider::Divider,
    scroll::ScrollbarAxis,
};
use crate::entry_screen::{EntryScreen, GitFetchStatus, recent_projects::RecentProjectsList};

pub fn render_recent_projects(screen: &mut EntryScreen, cols: usize, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();

    v_flex()
        .size_full()
        .scrollable(ScrollbarAxis::Vertical)
        .p_12()
        .gap_8()
        .child(
            v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .child(
                            h_flex()
                                .gap_3()
                                .items_center()
                                .child(
                                    div()
                                        .text_3xl()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(theme.foreground)
                                        .child("Recent Projects")
                                )
                                .when(screen.is_fetching_updates, |this| {
                                    this.child(
                                        Icon::new(IconName::ArrowUp)
                                            .size(px(18.))
                                            .text_color(theme.accent)
                                    )
                                })
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("refresh-btn")
                                        .label("Refresh")
                                        .icon(IconName::ArrowUp)
                                        .with_variant(ui::button::ButtonVariant::Secondary)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let path = this.recent_projects_path.clone();
                                            this.recent_projects = RecentProjectsList::load(&path);
                                            this.start_git_fetch_all(cx);
                                            cx.notify();
                                        }))
                                )
                                .child(
                                    Button::new("open-folder-btn")
                                        .label("Open Folder")
                                        .icon(IconName::FolderOpen)
                                        .with_variant(ui::button::ButtonVariant::Primary)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_folder_dialog(window, cx);
                                        }))
                                )
                        )
                        )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Browse and launch your Pulsar Engine projects")
                )
        )
        .child({
            if screen.recent_projects.projects.is_empty() {
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_6()
                    .p_12()
                    .child(
                        div()
                            .w(px(120.))
                            .h(px(120.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(theme.sidebar)
                            .border_2()
                            .border_color(theme.border)
                            .child(
                                Icon::new(IconName::FolderOpen)
                                    .size(px(56.))
                                    .text_color(theme.muted_foreground)
                            )
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.foreground)
                                    .child("No Projects Yet")
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child("Get started by creating a new project or opening an existing one")
                            )
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                Button::new("empty-new-project")
                                    .label("Create Project")
                                    .icon(IconName::Plus)
                                    .with_variant(ui::button::ButtonVariant::Primary)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.view = crate::entry_screen::types::EntryScreenView::NewProject;
                                        cx.notify();
                                    }))
                            )
                            .child(
                                Button::new("empty-open-folder")
                                    .label("Open Folder")
                                    .icon(IconName::FolderOpen)
                                    .with_variant(ui::button::ButtonVariant::Secondary)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_folder_dialog(window, cx);
                                    }))
                            )
                    )
                    .into_any_element()
            } else {
                render_project_grid(screen, cols, cx).into_any_element()
            }
        })
}

fn render_project_grid(screen: &mut EntryScreen, cols: usize, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let mut container = v_flex().gap_8();
    let mut row = h_flex().gap_8();
    let mut count = 0;
    
    for project in screen.recent_projects.projects.clone() {
        let proj_path = project.path.clone();
        let is_git = project.is_git;
        let proj_name = project.name.clone();
        let proj_name_for_settings = proj_name.clone();
        let last_opened = project.last_opened.clone()
            .map(|ts| format_timestamp(&ts))
            .unwrap_or_else(|| "Never opened".to_string());
        
        // Get git fetch status
        let git_status = screen.git_fetch_statuses.lock().get(&proj_path).cloned()
            .unwrap_or(GitFetchStatus::NotStarted);
        
        // Load tool preferences for this project
        let (preferred_editor, preferred_git_tool) = crate::entry_screen::views::load_project_tool_preferences(&std::path::PathBuf::from(&proj_path));
        
        let card = v_flex()
            .id(SharedString::from(format!("project-{}", proj_path)))
            .w(px(340.))
            .h(px(200.))
            .gap_4()
            .p_5()
            .border_1()
            .border_color(theme.border)
            .rounded_xl()
            .bg(theme.sidebar)
            .shadow_sm()
            .hover(|this| {
                this.border_color(theme.primary)
                    .shadow_lg()
                    .bg(hsla(
                        theme.sidebar.h,
                        theme.sidebar.s,
                        theme.sidebar.l * 1.05,
                        theme.sidebar.a
                    ))
            })
            .cursor_pointer()
            .on_click(cx.listener({
                let path = proj_path.clone();
                move |this, _, _, cx| {
                    let mut path_buf = std::path::PathBuf::from(&path);
                    
                    //TODO: Why is this happening?
                    // Fix doubled project folder name (e.g., blank_project/blank_project -> blank_project)
                    if let (Some(file_name), Some(parent)) = (path_buf.file_name(), path_buf.parent()) {
                        if let Some(parent_name) = parent.file_name() {
                            if file_name == parent_name {
                                // Path has doubled folder name, use parent instead
                                path_buf = parent.to_path_buf();
                                tracing::debug!("[RECENT_PROJECTS] Fixed doubled path: {} -> {}", path, path_buf.display());
                            }
                        }
                    }
                    
                    this.launch_project(path_buf, cx);
                }
            }))
            .child(
                h_flex()
                    .items_start()
                    .gap_3()
                    .w_full()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_shrink_0()
                            .w(px(48.))
                            .h(px(48.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_lg()
                            .bg(hsla(
                                theme.primary.h,
                                theme.primary.s,
                                theme.primary.l,
                                0.15
                            ))
                            .child(
                                Icon::new(IconName::Folder)
                                    .size(px(28.))
                                    .text_color(theme.primary)
                            )
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.foreground)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(proj_name)
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(proj_path.clone())
                            )
                    )
                    .when(is_git, |this| {
                        this.child(
                            match &git_status {
                                GitFetchStatus::Fetching => {
                                    Icon::new(IconName::ArrowUp)
                                        .size(px(14.))
                                        .text_color(theme.muted_foreground)
                                        .into_any_element()
                                }
                                GitFetchStatus::UpdatesAvailable(_) => {
                                    Icon::new(IconName::ArrowUp)
                                        .size(px(14.))
                                        .text_color(theme.accent)
                                        .into_any_element()
                                }
                                _ => {
                                    Icon::new(IconName::GitHub)
                                        .size(px(14.))
                                        .text_color(theme.muted_foreground)
                                        .into_any_element()
                                }
                            }
                        )
                    })
            )
            .child(
                v_flex()
                    .flex_1()
                    .justify_end()
                    .gap_2()
                    .when(is_git, |this| {
                        match &git_status {
                            GitFetchStatus::UpdatesAvailable(count) => {
                                this.child(
                                    div()
                                        .px_3()
                                        .py_1p5()
                                        .rounded_md()
                                        .bg(hsla(
                                            theme.accent_foreground.h,
                                            theme.accent_foreground.s,
                                            theme.accent_foreground.l,
                                            0.3
                                        ))
                                        .child(
                                            h_flex()
                                                .gap_1p5()
                                                .items_center()
                                                .child(
                                                    Icon::new(IconName::ArrowUp)
                                                        .size(px(12.))
                                                        .text_color(theme.accent)
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_weight(gpui::FontWeight::MEDIUM)
                                                        .text_color(theme.accent)
                                                        .child(format!("{} update{} available", count, if *count == 1 { "" } else { "s" }))
                                                )
                                        )
                                )
                            }
                            _ => this
                        }
                    })
            )
            .child(
                div()
                    .w_full()
                    .h(px(1.))
                    .bg(theme.border)
            )
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(
                                Icon::new(IconName::Clock)
                                    .size(px(12.))
                                    .text_color(theme.muted_foreground)
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(last_opened)
                            )
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            // Add integration buttons if defaults are set
                            .when_some(preferred_editor.clone(), |this, editor: String| {
                                this.child(
                                    Button::new(SharedString::from(format!("open-editor-{}", proj_path)))
                                        .icon(IconName::Code)
                                        .tooltip(format!("Open in {}", get_tool_display_name(&editor)))
                                        .with_variant(ui::button::ButtonVariant::Ghost)
                                        .on_click({
                                            let cmd = editor.clone();
                                            let path = std::path::PathBuf::from(proj_path.clone());
                                            move |_, _, _| {
                                                use crate::entry_screen::integration_launcher;
                                                let _ = integration_launcher::launch_editor(&cmd, &path);
                                            }
                                        })
                                )
                            })
                            .when(is_git, |this| {
                                this.when_some(preferred_git_tool.clone(), |this2, git_tool: String| {
                                    this2.child(
                                        Button::new(SharedString::from(format!("open-git-{}", proj_path)))
                                            .icon(IconName::GitHub)
                                            .tooltip(format!("Open in {}", get_tool_display_name(&git_tool)))
                                            .with_variant(ui::button::ButtonVariant::Ghost)
                                            .on_click({
                                                let cmd = git_tool.clone();
                                                let path = std::path::PathBuf::from(proj_path.clone());
                                                move |_, _, _| {
                                                    use crate::entry_screen::integration_launcher;
                                                    let _ = integration_launcher::launch_git_tool(&cmd, &path);
                                                }
                                            })
                                    )
                                })
                            })
                            .when(is_git, |this| {
                                match &git_status {
                                    GitFetchStatus::UpdatesAvailable(count) => {
                                        this.child(
                                            Button::new(SharedString::from(format!("update-{}", proj_path)))
                                                .label(format!("Pull {} update{}", count, if *count == 1 { "" } else { "s" }))
                                                .icon(IconName::ArrowUp)
                                                .with_variant(ui::button::ButtonVariant::Primary)
                                                .on_click(cx.listener({
                                                    let path = proj_path.clone();
                                                    move |this, _, _, cx| {
                                                        this.pull_project_updates(path.clone(), cx);
                                                    }
                                                }))
                                        )
                                    }
                                    _ => this
                                }
                            })
                            .child(
                                Button::new(SharedString::from(format!("settings-{}", proj_path)))
                                    .icon(IconName::Settings)
                                    .tooltip("Project settings")
                                    .with_variant(ui::button::ButtonVariant::Ghost)
                                    .on_click(cx.listener({
                                        let path = proj_path.clone();
                                        let name = proj_name_for_settings.clone();
                                        move |this, _, _, cx| {
                                            this.open_project_settings(std::path::PathBuf::from(&path), name.clone(), cx);
                                        }
                                    }))
                            )
                            .child(
                                Button::new(SharedString::from(format!("location-{}", proj_path)))
                                    .icon(IconName::FolderOpen)
                                    .tooltip("Open in file manager")
                                    .with_variant(ui::button::ButtonVariant::Ghost)
                                    .on_click({
                                        let path = std::path::PathBuf::from(proj_path.clone());
                                        move |_, _, _| {
                                            use crate::entry_screen::integration_launcher;
                                            let _ = integration_launcher::launch_file_manager(&path);
                                        }
                                    })
                            )
                            .child(
                                Button::new(SharedString::from(format!("remove-{}", proj_path)))
                                    .icon(IconName::Trash)
                                    .tooltip("Remove from recent")
                                    .with_variant(ui::button::ButtonVariant::Ghost)
                                    .on_click(cx.listener({
                                        let path = proj_path.clone();
                                        move |this, _, _, cx| {
                                            this.remove_recent_project(path.clone(), cx);
                                        }
                                    }))
                            )
                    )
            );
        
        row = row.child(card);
        count += 1;


        if count >= cols {
            container = container.child(row);
            row = h_flex().gap_8();
            count = 0;
        }
    }

    if count > 0 {
        container = container.child(row);
    }

    container
}

fn get_tool_display_name(command: &str) -> String {
    match command {
        "code" => "VS Code".to_string(),
        "devenv" => "Visual Studio".to_string(),
        "subl" => "Sublime Text".to_string(),
        "vim" => "Vim".to_string(),
        "nvim" => "Neovim".to_string(),
        "emacs" => "Emacs".to_string(),
        "idea" => "IntelliJ IDEA".to_string(),
        "clion" => "CLion".to_string(),
        "notepad++" => "Notepad++".to_string(),
        "git" => "Git GUI".to_string(),
        "github" => "GitHub Desktop".to_string(),
        "gitkraken" => "GitKraken".to_string(),
        "sourcetree" => "SourceTree".to_string(),
        "git-cola" => "Git Cola".to_string(),
        "lazygit" => "Lazygit".to_string(),
        _ => command.to_string(),
    }
}

fn format_timestamp(timestamp: &str) -> String {
    use chrono::{DateTime, Utc};

    // Try to parse the timestamp
    let parsed = DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|dt| dt.with_timezone(&Utc));

    if let Some(dt) = parsed {
        let now = Utc::now();
        let duration = now.signed_duration_since(dt);

        let seconds = duration.num_seconds();
        let minutes = duration.num_minutes();
        let hours = duration.num_hours();
        let days = duration.num_days();

        if seconds < 60 {
            "Just now".to_string()
        } else if minutes < 60 {
            format!("{} min ago", minutes)
        } else if hours < 24 {
            format!("{} hr ago", hours)
        } else if days == 1 {
            "Yesterday".to_string()
        } else if days < 7 {
            format!("{} days ago", days)
        } else if days < 30 {
            let weeks = days / 7;
            format!("{} week{} ago", weeks, if weeks == 1 { "" } else { "s" })
        } else if days < 365 {
            let months = days / 30;
            format!("{} month{} ago", months, if months == 1 { "" } else { "s" })
        } else {
            let years = days / 365;
            format!("{} year{} ago", years, if years == 1 { "" } else { "s" })
        }
    } else {
        "Unknown".to_string()
    }
}
