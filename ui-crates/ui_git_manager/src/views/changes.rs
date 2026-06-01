//! Changes view: staged / unstaged file lists + commit section

use super::toolbar::render_toolbar;
use crate::{
    ChangesRow, CopyFullPath, CopyRelativePath, DiscardFileChanges, GitManager, IgnoreExtension,
    IgnoreFile, IgnoreFolder, OpenInExplorer, models::*,
};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::rc::Rc;
use ui::{
    ActiveTheme as _, Icon, IconName,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::TextInput,
    menu::context_menu::ContextMenuExt as _,
    scroll::Scrollbar,
    v_flex, v_virtual_list,
};

/// Fixed row heights used for item_sizes pre-computation.
const HEADER_ROW_HEIGHT: f32 = 28.0;
const FILE_ROW_HEIGHT: f32 = 32.0;

pub fn render_changes_view(
    git_manager: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> impl IntoElement {
    let has_no_changes = git_manager.changes_rows.is_empty();

    let border = cx.theme().border;
    let muted_fg = cx.theme().muted_foreground;

    let has_staged = git_manager.changes_rows.iter().any(|r| {
        matches!(
            r,
            ChangesRow::File {
                is_staged: true,
                ..
            }
        )
    });

    // Build item_sizes for the virtual list (one entry per row).
    let item_sizes: Rc<Vec<Size<Pixels>>> = Rc::new(
        git_manager
            .changes_rows
            .iter()
            .map(|r| match r {
                ChangesRow::Header { .. } => size(px(0.0), px(HEADER_ROW_HEIGHT)),
                ChangesRow::File { .. } => size(px(0.0), px(FILE_ROW_HEIGHT)),
            })
            .collect(),
    );

    let entity = cx.entity().clone();
    let scroll_handle = git_manager.changes_scroll.clone();
    let scrollbar_state = git_manager.changes_scrollbar_state.clone();

    let list = v_virtual_list(entity, "git-changes-list", item_sizes, {
        move |git_manager: &mut GitManager,
              range: std::ops::Range<usize>,
              _window: &mut Window,
              cx: &mut Context<GitManager>| {
            let radius = cx.theme().radius;
            let muted_fg = cx.theme().muted_foreground;
            let foreground = cx.theme().foreground;
            let muted_bg = cx.theme().muted;
            let success = cx.theme().success;
            let danger = cx.theme().danger;
            let warning = cx.theme().warning;

            range
                .map(|i| -> AnyElement {
                    match &git_manager.changes_rows[i].clone() {
                        ChangesRow::Header {
                            title,
                            count,
                            is_staged,
                        } => {
                            let title = title.clone();
                            let count = *count;
                            let is_staged = *is_staged;
                            h_flex()
                                .px_1()
                                .h(px(HEADER_ROW_HEIGHT))
                                .items_center()
                                .w_full()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted_fg)
                                        .child(format!("{} ({})", title, count)),
                                )
                                .when(is_staged, |this| {
                                    this.child(
                                        Button::new("unstage-all-button")
                                            .icon(Icon::new(IconName::Minus).text_color(warning))
                                            .ghost()
                                            .compact()
                                            .tooltip("Unstage All".to_string())
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.unstage_all(cx)),
                                            ),
                                    )
                                })
                                .when(!is_staged, |this| {
                                    this.child(
                                        Button::new("stage-all-button")
                                            .icon(Icon::new(IconName::Plus).text_color(warning))
                                            .ghost()
                                            .compact()
                                            .tooltip("Stage All".to_string())
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.stage_all(cx)),
                                            ),
                                    )
                                })
                                .into_any_element()
                        }
                        ChangesRow::File { change, is_staged } => {
                            let file = change.clone();
                            let is_staged = *is_staged;
                            let file_path = file.path.clone();
                            let file_path_for_select = file.path.clone();
                            let file_path_for_ctx = file.path.clone();
                            let file_path_for_stage = file.path.clone();
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
                            let folder_paths: Vec<String> = {
                                let norm = file_path_for_ctx.replace('\\', "/");
                                let mut parts: Vec<&str> = norm.split('/').collect();
                                parts.pop();
                                let mut folders = Vec::new();
                                while !parts.is_empty() {
                                    folders.push(parts.join("/"));
                                    parts.pop();
                                }
                                folders
                            };
                            let has_folders = !folder_paths.is_empty();
                            let row_id = SharedString::from(format!("git-file-{}", file.path));

                            h_flex()
                                .id(row_id)
                                .px_1()
                                .w_full()
                                .h(px(FILE_ROW_HEIGHT))
                                .rounded(radius)
                                .gap_1()
                                .items_center()
                                .hover(move |s| s.bg(muted_hover))
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
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
                                        .menu(
                                            "Ignore File",
                                            Box::new(IgnoreFile { path: fp.clone() }),
                                        );
                                    let menu = if has_folders {
                                        let folders_for_sub = folder_paths.clone();
                                        menu.submenu(
                                            "Ignore Folder",
                                            window,
                                            cx,
                                            move |mut sub, _w, _c| {
                                                for folder in &folders_for_sub {
                                                    sub = sub.menu(
                                                        SharedString::from(folder.clone()),
                                                        Box::new(IgnoreFolder {
                                                            folder: folder.clone(),
                                                        }),
                                                    );
                                                }
                                                sub
                                            },
                                        )
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
                                        .menu(
                                            "Show in Explorer",
                                            Box::new(OpenInExplorer { path: fp }),
                                        )
                                })
                                .child(
                                    div()
                                        .w(px(14.))
                                        .h(px(14.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(radius)
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(status_color)
                                        .child(file_status),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .text_xs()
                                        .text_color(foreground)
                                        .overflow_hidden()
                                        .child(display_name),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("stg-{}", file_path)))
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
                                                this.unstage_file(file_path_for_stage.clone(), cx);
                                            } else {
                                                this.stage_file(file_path_for_stage.clone(), cx);
                                            }
                                        })),
                                )
                                .into_any_element()
                        }
                    }
                })
                .collect()
        }
    })
    .track_scroll(&scroll_handle);

    let changes_container = div()
        .relative()
        .flex_1()
        .overflow_hidden()
        .child(list)
        .child(
            div()
                .absolute()
                .inset_0()
                .child(Scrollbar::vertical(&scrollbar_state, &scroll_handle)),
        );

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
        .when(has_no_changes, |this| {
            this.child(
                v_flex()
                    .flex_1()
                    .py_8()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::Check)
                            .size(px(24.))
                            .text_color(muted_fg),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child("Working directory clean"),
                    ),
            )
        })
        .when(!has_no_changes, |this| this.child(changes_container))
        .child(commit_section)
}
