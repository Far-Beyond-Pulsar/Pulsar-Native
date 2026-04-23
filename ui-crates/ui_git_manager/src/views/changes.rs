//! Changes view: staged / unstaged file lists + commit section

use super::toolbar::render_toolbar;
use crate::{
    CopyFullPath, CopyRelativePath, DiscardFileChanges, GitManager,
    IgnoreExtension, IgnoreFile, IgnoreFolder, OpenInExplorer, models::*,
};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use ui::{
    ActiveTheme as _, Icon, IconName, StyledExt,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::TextInput,
    menu::context_menu::ContextMenuExt as _,
    scroll::ScrollbarAxis,
    v_flex,
};

pub fn render_changes_view(
    git_manager: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> impl IntoElement {
    let repo_state = git_manager.repo_state.read();
    let staged_files = repo_state.staged_files.clone();
    let unstaged_files = repo_state.unstaged_files.clone();
    let untracked_files = repo_state.untracked_files.clone();
    drop(repo_state);

    let has_no_changes =
        staged_files.is_empty() && unstaged_files.is_empty() && untracked_files.is_empty();

    let mut unstaged_all: Vec<FileChange> = unstaged_files;
    unstaged_all.extend(untracked_files.iter().map(|path| FileChange {
        path: path.clone(),
        status: ChangeStatus::Untracked,
        additions: 0,
        deletions: 0,
    }));

    // Copy theme values before any listener() calls
    let border = cx.theme().border;
    let muted_fg = cx.theme().muted_foreground;

    let mut scroll_content = v_flex().id("git-changes-scroll").w_full().gap_2().p_2();

    if !staged_files.is_empty() {
        scroll_content =
            scroll_content.child(render_file_section("Staged", &staged_files, true, cx));
    }
    if !unstaged_all.is_empty() {
        scroll_content =
            scroll_content.child(render_file_section("Changes", &unstaged_all, false, cx));
    }
    if has_no_changes {
        let check_icon = Icon::new(IconName::Check)
            .size(px(24.))
            .text_color(muted_fg);
        scroll_content = scroll_content.child(
            v_flex()
                .py_8()
                .items_center()
                .justify_center()
                .gap_2()
                .child(check_icon)
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_fg)
                        .child("Working directory clean"),
                ),
        );
    }

    let has_staged = !staged_files.is_empty();
    let commit_section = v_flex()
        .px_2()
        .py_2()
        .gap_1()
        .border_t_1()
        .border_color(border)
        .child(TextInput::new(&git_manager.commit_message_input))
        .child(TextInput::new(&git_manager.commit_description_input))
        .when(has_staged, |this| {
            this.child(
                Button::new("commit-button")
                    .label("Commit to branch")
                    .primary()
                    .on_click(cx.listener(|this, _, _, cx| this.commit_changes(cx))),
            )
        });

    v_flex()
        .size_full()
        .child(render_toolbar(git_manager, cx))
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .child(scroll_content.scrollable(ScrollbarAxis::Vertical)),
        )
        .child(commit_section)
}

fn render_file_section(
    title: &str,
    files: &[FileChange],
    is_staged: bool,
    cx: &mut Context<GitManager>,
) -> impl IntoElement {
    // Copy theme values (Copy types) before any listener() calls
    let radius = cx.theme().radius;
    let muted_fg = cx.theme().muted_foreground;
    let foreground = cx.theme().foreground;
    let muted_bg = cx.theme().muted;
    let success = cx.theme().success;
    let danger = cx.theme().danger;
    let warning = cx.theme().warning;

    let mut file_list = v_flex().gap_px();

    for file in files.iter() {
        let file_path = file.path.clone();
        let file_path_for_select = file.path.clone();
        let file_path_for_ctx = file.path.clone();
        let file_status = file.status.short_str();
        let display_name = file
            .path
            .rsplit('/')
            .next()
            .or_else(|| file.path.rsplit('\\').next())
            .unwrap_or(file.path.as_str())
            .to_string();
        let status_color = match file.status {
            ChangeStatus::Added | ChangeStatus::Untracked => success,
            ChangeStatus::Deleted => danger,
            _ => warning,
        };
        let muted_hover = muted_bg.opacity(0.3);
        let ext = std::path::Path::new(&file_path_for_ctx)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();

        // Build all ancestor folder paths (relative to repo root) from innermost to outermost.
        // e.g. "src/foo/bar.rs" → ["src/foo", "src"]
        let folder_paths: Vec<String> = {
            let norm = file_path_for_ctx.replace('\\', "/");
            let mut parts: Vec<&str> = norm.split('/').collect();
            parts.pop(); // remove filename
            let mut folders = Vec::new();
            while !parts.is_empty() {
                folders.push(parts.join("/"));
                parts.pop();
            }
            folders
        };
        let has_folders = !folder_paths.is_empty();

        let row_id = SharedString::from(format!("git-file-{}", file.path));
        file_list = file_list.child(
            h_flex()
                .id(row_id)
                .px_1()
                .py_1()
                .rounded(radius)
                .gap_1()
                .items_center()
                .hover(move |s| s.bg(muted_hover))
                .cursor_pointer()
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                        this.select_file(file_path_for_select.clone(), cx);
                    }),
                )
                .context_menu(move |menu, window, cx| {
                    let fp = file_path_for_ctx.clone();
                    let menu = menu
                        .menu(
                            "Discard File Changes",
                            Box::new(DiscardFileChanges { path: fp.clone() }),
                        )
                        .separator()
                        .menu("Ignore File", Box::new(IgnoreFile { path: fp.clone() }));

                    let menu = if has_folders {
                        let folders_for_sub = folder_paths.clone();
                        menu.submenu("Ignore Folder", window, cx, move |mut sub, _w, _c| {
                            for folder in &folders_for_sub {
                                sub = sub.menu(
                                    SharedString::from(folder.clone()),
                                    Box::new(IgnoreFolder {
                                        folder: folder.clone(),
                                    }),
                                );
                            }
                            sub
                        })
                    } else {
                        menu
                    };

                    let menu = if !ext.is_empty() {
                        menu.menu(
                            SharedString::from(format!("Ignore *.{}", ext)),
                            Box::new(IgnoreExtension { path: fp.clone() }),
                        )
                    } else {
                        menu
                    };

                    menu.separator()
                        .menu(
                            "Copy Relative Path",
                            Box::new(CopyRelativePath { path: fp.clone() }),
                        )
                        .menu(
                            "Copy Full Path",
                            Box::new(CopyFullPath { path: fp.clone() }),
                        )
                        .separator()
                        .menu("Show in Explorer", Box::new(OpenInExplorer { path: fp }))
                })
                // Status letter badge
                .child(
                    div()
                        .w(px(14.))
                        .h(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius)
                        .text_xs()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(status_color)
                        .child(file_status),
                )
                // Filename (filename only, not full path)
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(foreground)
                        .overflow_hidden()
                        .child(display_name),
                )
                // Stage / unstage button
                .child(
                    Button::new(SharedString::from(format!("stg-{}", file.path)))
                        .icon(if is_staged {
                            IconName::Minus
                        } else {
                            IconName::Plus
                        })
                        .ghost()
                        .compact()
                        .tooltip(if is_staged {
                            "Unstage".to_string()
                        } else {
                            "Stage".to_string()
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if is_staged {
                                this.unstage_file(file_path.clone(), cx);
                            } else {
                                this.stage_file(file_path.clone(), cx);
                            }
                        })),
                ),
        );
    }

    v_flex()
        .gap_1()
        .child(
            h_flex()
                .px_1()
                .items_center()
                .w_full()
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(muted_fg)
                        .child(format!("{} ({})", title, files.len())),
                )
                .when(is_staged, |this| {
                    this.child(
                        Button::new("unstage-all-button")
                            .icon(Icon::new(IconName::Minus).text_color(warning))
                            .ghost()
                            .compact()
                            .tooltip("Unstage All".to_string())
                            .on_click(cx.listener(|this, _, _, cx| this.unstage_all(cx))),
                    )
                })
                .when(!is_staged, |this| {
                    this.child(
                        Button::new("stage-all-button")
                            .icon(Icon::new(IconName::Plus).text_color(warning))
                            .ghost()
                            .compact()
                            .tooltip("Stage All".to_string())
                            .on_click(cx.listener(|this, _, _, cx| this.stage_all(cx))),
                    )
                }),
        )
        .child(file_list)
}
