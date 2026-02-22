//! Git Manager UI views

use crate::{GitManager, GitView, models::*, FileContentResult};
use gpui::*;
use gpui::prelude::FluentBuilder as _;
use ui::{
    h_flex, v_flex, Icon, IconName, ActiveTheme as _, StyledExt,
    button::{Button, ButtonVariants as _},
    scroll::ScrollbarAxis,
    input::TextInput,
};

/// Render the main changes view
pub fn render_changes_view(git_manager: &mut GitManager, cx: &mut Context<GitManager>) -> impl IntoElement {
    // Clone data we need
    let repo_state = git_manager.repo_state.read();
    let staged_files = repo_state.staged_files.clone();
    let unstaged_files = repo_state.unstaged_files.clone();
    let untracked_files = repo_state.untracked_files.clone();
    drop(repo_state);

    let has_no_changes = staged_files.is_empty() && unstaged_files.is_empty() && untracked_files.is_empty();

    let mut unstaged_all: Vec<FileChange> = unstaged_files;
    unstaged_all.extend(untracked_files.iter().map(|path| FileChange {
        path: path.clone(),
        status: ChangeStatus::Untracked,
        additions: 0,
        deletions: 0,
    }));

    // Build content
    let mut content = v_flex().flex_1().p_4().gap_4();

    if !staged_files.is_empty() {
        content = content.child(render_file_section("Staged Changes", &staged_files, true, cx));
    }
    if !unstaged_all.is_empty() {
        content = content.child(render_file_section("Changes", &unstaged_all, false, cx));
    }
    if has_no_changes {
        let theme = cx.theme();
        content = content.child(
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_4()
                .child(Icon::new(IconName::Check).size(px(48.)).text_color(theme.muted_foreground))
                .child(div().text_lg().text_color(theme.foreground).child("No changes"))
                .child(div().text_sm().text_color(theme.muted_foreground).child("Your working directory is clean"))
        );
    }

    let content = content.scrollable(ScrollbarAxis::Vertical);

    // Commit section
    let has_staged = !staged_files.is_empty();
    let theme = cx.theme();
    let commit_section = v_flex()
        .p_4()
        .gap_2()
        .border_t_1()
        .border_color(theme.border)
        .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(theme.foreground).child("Commit"))
        .child(TextInput::new(&git_manager.commit_message_input))
        .when(has_staged, |this| {
            this.child(
                h_flex().justify_end().child(
                    Button::new("commit-button")
                        .label("Commit")
                        .with_variant(ui::button::ButtonVariant::Primary)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.commit_changes(cx);
                        }))
                )
            )
        });

    let toolbar = render_toolbar(git_manager, cx);

    v_flex()
        .size_full()
        .child(toolbar)
        .child(v_flex().flex_1().overflow_hidden().child(content))
        .child(commit_section)
}

fn render_toolbar(git_manager: &GitManager, cx: &mut Context<GitManager>) -> impl IntoElement {
    let theme = cx.theme();
    let repo_state = git_manager.repo_state.read();
    let current_view = git_manager.current_view;
    let ahead = repo_state.ahead;
    let behind = repo_state.behind;
    drop(repo_state);

    let mut actions = h_flex().flex_1().justify_end().gap_2()
        .child(Button::new("refresh").icon(IconName::ArrowUp).tooltip("Refresh")
            .with_variant(ui::button::ButtonVariant::Ghost)
            .on_click(cx.listener(|this, _, _, cx| this.refresh_state(cx))));

    if ahead > 0 {
        actions = actions.child(Button::new("push").icon(IconName::ArrowUp)
            .label(format!("Push ({})", ahead))
            .with_variant(ui::button::ButtonVariant::Primary)
            .on_click(cx.listener(|this, _, _, cx| this.push(cx))));
    }
    if behind > 0 {
        actions = actions.child(Button::new("pull").icon(IconName::ArrowDown)
            .label(format!("Pull ({})", behind))
            .with_variant(ui::button::ButtonVariant::Primary)
            .on_click(cx.listener(|this, _, _, cx| this.pull(cx))));
    }

    h_flex()
        .p_4()
        .gap_2()
        .border_b_1()
        .border_color(theme.border)
        .child(h_flex().gap_2()
            .child(Button::new("tab-changes").label("Changes")
                .with_variant(if current_view == GitView::Changes {
                    ui::button::ButtonVariant::Primary
                } else {
                    ui::button::ButtonVariant::Ghost
                })
                .on_click(cx.listener(|this, _, _, cx| this.switch_view(GitView::Changes, cx))))
            .child(Button::new("tab-history").label("History")
                .with_variant(if current_view == GitView::History {
                    ui::button::ButtonVariant::Primary
                } else {
                    ui::button::ButtonVariant::Ghost
                })
                .on_click(cx.listener(|this, _, _, cx| this.switch_view(GitView::History, cx))))
            .child(Button::new("tab-branches").label("Branches")
                .with_variant(if current_view == GitView::Branches {
                    ui::button::ButtonVariant::Primary
                } else {
                    ui::button::ButtonVariant::Ghost
                })
                .on_click(cx.listener(|this, _, _, cx| this.switch_view(GitView::Branches, cx))))
        )
        .child(actions)
}

fn render_file_section(
    title: &str,
    files: &[FileChange],
    is_staged: bool,
    cx: &mut Context<GitManager>,
) -> impl IntoElement {
    let theme = cx.theme();

    let mut file_list = v_flex().gap_1();
    for file in files.iter() {
        let file_path = file.path.clone();
        let file_path_for_select = file.path.clone();
        let file_status = file.status.short_str();
        let file_name = file.path.clone();

        file_list = file_list.child(
            h_flex()
                .p_2()
                .rounded(theme.radius)
                .gap_3()
                .items_center()
                .hover(|this| this.bg(theme.muted.opacity(0.3)))
                .cursor_pointer()
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                    this.select_file(file_path_for_select.clone(), cx);
                }))
                .child(div().w(px(24.)).h(px(20.)).flex().items_center().justify_center()
                    .rounded(theme.radius).bg(theme.primary.opacity(0.2))
                    .text_xs().font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.primary).child(file_status))
                .child(div().flex_1().text_sm().text_color(theme.foreground).child(file_name))
                .child(Button::new(SharedString::from(format!("stage-{}", file.path)))
                    .icon(if is_staged { IconName::Trash } else { IconName::Plus })
                    .tooltip(if is_staged { "Unstage" } else { "Stage" })
                    .with_variant(ui::button::ButtonVariant::Ghost)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if is_staged {
                            this.unstage_file(file_path.clone(), cx);
                        } else {
                            this.stage_file(file_path.clone(), cx);
                        }
                    })))
        );
    }

    v_flex()
        .gap_2()
        .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(theme.foreground).child(format!("{} ({})", title, files.len())))
        .child(file_list)
}

/// Render the commit history view
pub fn render_history_view(git_manager: &mut GitManager, cx: &mut Context<GitManager>) -> impl IntoElement {
    let repo_state = git_manager.repo_state.read();
    let commits = repo_state.commits.clone();
    let selected_commit = git_manager.selected_commit.clone();
    drop(repo_state);

    let theme = cx.theme();
    let mut commit_list = v_flex().flex_1().scrollable(ScrollbarAxis::Vertical).p_4().gap_2();

    for commit in commits {
        let is_selected = selected_commit.as_ref() == Some(&commit.hash);
        let commit_msg = commit.message.lines().next().unwrap_or("").to_string();
        let short_hash = commit.short_hash.clone();
        let author = commit.author.clone();
        let timestamp_str = format!("{}", commit.timestamp.format("%Y-%m-%d %H:%M"));
        let files_changed_str = format!("{} file{} changed", commit.files_changed,
            if commit.files_changed == 1 { "" } else { "s" });
        let commit_hash = commit.hash.clone();

        commit_list = commit_list.child(
            h_flex()
                .p_3()
                .rounded(theme.radius)
                .gap_3()
                .border_1()
                .border_color(if is_selected { theme.primary } else { theme.border })
                .bg(if is_selected { theme.muted.opacity(0.5) } else { theme.background })
                .hover(|this| this.border_color(theme.primary).bg(theme.muted.opacity(0.3)))
                .cursor_pointer()
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                    this.select_commit(commit_hash.clone(), cx);
                }))
                .child(v_flex().flex_1().gap_2()
                    .child(h_flex().gap_2().items_center()
                        .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(commit_msg))
                        .child(div().px_2().py_1().rounded(theme.radius)
                            .bg(theme.muted).text_xs().text_color(theme.muted_foreground)
                            .child(short_hash)))
                    .child(h_flex().gap_4().text_xs().text_color(theme.muted_foreground)
                        .child(format!("by {}", author))
                        .child(timestamp_str)
                        .child(files_changed_str)))
        );
    }

    let toolbar = render_toolbar(git_manager, cx);

    v_flex().size_full().child(toolbar).child(commit_list)
}

/// Render the branches view
pub fn render_branches_view(git_manager: &mut GitManager, cx: &mut Context<GitManager>) -> impl IntoElement {
    let repo_state = git_manager.repo_state.read();
    let branches = repo_state.branches.clone();
    drop(repo_state);

    let local_branches: Vec<_> = branches.iter().filter(|b| !b.is_remote).cloned().collect();
    let remote_branches: Vec<_> = branches.iter().filter(|b| b.is_remote).cloned().collect();

    let branch_sections = v_flex().flex_1().scrollable(ScrollbarAxis::Vertical).p_4().gap_4()
        .child(render_branch_section("Local Branches", &local_branches, cx))
        .child(render_branch_section("Remote Branches", &remote_branches, cx));

    let toolbar = render_toolbar(git_manager, cx);

    v_flex()
        .size_full()
        .child(toolbar)
        .child(branch_sections)
}

fn render_branch_section(
    title: &str,
    branches: &[Branch],
    cx: &mut Context<GitManager>,
) -> impl IntoElement {
    let theme = cx.theme();
    let mut branch_list = v_flex().gap_1();

    for branch in branches {
        let branch_name = branch.name.clone();
        let is_current = branch.is_current;
        let is_remote = branch.is_remote;
        let branch_name_for_switch = branch_name.clone();

        let mut item = h_flex()
            .p_3()
            .rounded(theme.radius)
            .gap_3()
            .items_center()
            .border_1()
            .border_color(if is_current { theme.primary } else { theme.border })
            .bg(if is_current { theme.muted.opacity(0.5) } else { theme.background })
            .hover(|this| this.border_color(theme.primary).bg(theme.muted.opacity(0.3)))
            .child(Icon::new(IconName::GitBranch).size(px(16.))
                .text_color(if is_current { theme.primary } else { theme.muted_foreground }))
            .child(div().flex_1().text_sm().text_color(theme.foreground).child(branch_name));

        // Only make local branches clickable if they're not current
        if !is_current && !is_remote {
            item = item.cursor_pointer().on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                this.switch_branch(branch_name_for_switch.clone(), cx);
            }));
        }

        if is_current {
            item = item.child(div().px_2().py_1().rounded(theme.radius)
                .bg(theme.primary.opacity(0.2)).text_xs().text_color(theme.primary).child("current"));
        }

        branch_list = branch_list.child(item);
    }

    v_flex()
        .gap_2()
        .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(theme.foreground).child(format!("{} ({})", title, branches.len())))
        .child(branch_list)
}

/// Render the right-hand file content panel
pub fn render_file_panel(git_manager: &GitManager, cx: &mut Context<GitManager>) -> impl IntoElement {
    let theme = cx.theme();

    let header = h_flex()
        .p_3()
        .border_b_1()
        .border_color(theme.border)
        .child(
            div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(match &git_manager.selected_file {
                    Some(path) => path.clone(),
                    None => "No file selected".to_string(),
                })
        );

    let body: AnyElement = match (&git_manager.selected_file, &git_manager.file_content) {
        (None, _) => {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .child(Icon::new(IconName::Page).size(px(40.)).text_color(theme.muted_foreground))
                .child(div().text_sm().text_color(theme.muted_foreground)
                    .child("Select a file to view its contents"))
                .into_any_element()
        }
        (Some(_), None) => {
            // Loading
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(div().text_sm().text_color(theme.muted_foreground).child("Loading…"))
                .into_any_element()
        }
        (Some(_), Some(FileContentResult::Text(text))) => {
            div()
                .flex_1()
                .overflow_hidden()
                .scrollable(ScrollbarAxis::Both)
                .p_4()
                .child(
                    div()
                        .font_family("monospace")
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(text.clone())
                )
                .into_any_element()
        }
        (Some(_), Some(FileContentResult::Binary)) => {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .child(Icon::new(IconName::Page).size(px(40.)).text_color(theme.muted_foreground))
                .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.foreground).child("Binary file"))
                .child(div().text_xs().text_color(theme.muted_foreground)
                    .child("This file cannot be displayed as text"))
                .into_any_element()
        }
        (Some(_), Some(FileContentResult::TooLong(lines))) => {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .child(Icon::new(IconName::Page).size(px(40.)).text_color(theme.muted_foreground))
                .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.foreground).child("File too long"))
                .child(div().text_xs().text_color(theme.muted_foreground)
                    .child(format!("{} lines — only files under 1 000 lines are shown", lines)))
                .into_any_element()
        }
        (Some(_), Some(FileContentResult::Error(msg))) => {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .child(Icon::new(IconName::Close).size(px(40.)).text_color(theme.danger_active))
                .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.foreground).child("Could not read file"))
                .child(div().text_xs().text_color(theme.muted_foreground).child(msg.clone()))
                .into_any_element()
        }
    };

    v_flex()
        .size_full()
        .child(header)
        .child(body)
}
