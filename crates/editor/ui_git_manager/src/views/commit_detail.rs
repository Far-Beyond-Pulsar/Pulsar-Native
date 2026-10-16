//! Selected-commit detail: message header, changed files and the diff.

use super::common::{FONT_MONO, avatar, commit_file_menu, placeholder, relative_time, status_color, virtual_panel};
use super::diff::render_diff_panel;
use crate::history::DiffTarget;
use crate::tree::TreeItem;
use crate::{GitLayout, GitManager};
use gpui::{prelude::FluentBuilder as _, *};
use ui::{
    ActiveTheme as _, Icon, IconName,
    menu::context_menu::ContextMenuExt as _,
    h_flex, v_flex,
};

const ROW_HEIGHT: f32 = 26.0;
const INDENT: f32 = 12.0;
const FILES_WIDTH: f32 = 270.0;

pub(crate) fn render_commit_detail(
    gm: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> impl IntoElement + use<> {
    let border = cx.theme().border;

    if gm.selected_commit.is_none() {
        return placeholder(IconName::GitCommit, "Select a commit to see what it changed", cx)
            .into_any_element();
    }
    if let Some(error) = gm.commit_detail_error.clone() {
        return placeholder(IconName::CircleX, error, cx).into_any_element();
    }
    if gm.commit_detail.is_none() {
        return placeholder(IconName::Loader, "Loading commit…", cx).into_any_element();
    }

    let header = render_header(gm, cx);
    let files = render_files(gm, cx);
    let diff = render_diff_panel(gm, cx);

    v_flex()
        .size_full()
        .child(header)
        .child(
            h_flex()
                .flex_1()
                .min_h_0()
                .child(
                    v_flex()
                        .w(px(FILES_WIDTH))
                        .h_full()
                        .flex_shrink_0()
                        .border_r_1()
                        .border_color(border)
                        .child(files),
                )
                .child(div().flex_1().min_w_0().h_full().child(diff)),
        )
        .into_any_element()
}

fn render_header(gm: &mut GitManager, cx: &mut Context<GitManager>) -> AnyElement {
    let Some(detail) = gm.commit_detail.clone() else {
        return div().into_any_element();
    };
    gm.ensure_avatar_loaded(&detail.email, cx);
    let image = gm.avatar_cache.get(&detail.email).and_then(|v| v.clone());

    let theme = cx.theme();
    let (border, foreground, muted_fg, primary) =
        (theme.border, theme.foreground, theme.muted_foreground, theme.primary);

    let mut lines = detail.message.lines();
    let summary = lines.next().unwrap_or("").to_string();
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    let parents = detail
        .parents
        .iter()
        .map(|p| format!("{:.7}", p))
        .collect::<Vec<_>>()
        .join(", ");
    let hash = detail.hash.clone();

    v_flex()
        .w_full()
        .flex_shrink_0()
        .px_3()
        .py_2()
        .gap_1p5()
        .border_b_1()
        .border_color(border)
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(foreground)
                .child(summary),
        )
        .when(!body.is_empty(), |this| {
            this.child(
                div()
                    .id("commit-body")
                    .max_h(px(96.))
                    .overflow_y_scroll()
                    .text_xs()
                    .text_color(muted_fg)
                    .child(body),
            )
        })
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(avatar(image, &detail.author, 20.0))
                .child(
                    div()
                        .text_xs()
                        .text_color(foreground)
                        .child(detail.author.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(relative_time(detail.timestamp)),
                )
                .child(div().flex_1())
                .when(!parents.is_empty(), |this| {
                    this.child(
                        div()
                            .text_size(px(10.))
                            .text_color(muted_fg)
                            .child(format!("parent {}", parents)),
                    )
                })
                .child(
                    div()
                        .id("commit-hash")
                        .cursor_pointer()
                        .px(px(5.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(primary.opacity(0.12))
                        .font_family(FONT_MONO)
                        .text_size(px(10.))
                        .text_color(primary)
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(hash.clone()));
                        })
                        .child(format!("{:.10}", detail.hash)),
                ),
        )
        .into_any_element()
}

fn render_files(gm: &mut GitManager, cx: &mut Context<GitManager>) -> AnyElement {
    let (added, removed, count) = gm
        .commit_detail
        .as_ref()
        .map(|d| {
            (
                d.files.iter().map(|f| f.additions).sum::<usize>(),
                d.files.iter().map(|f| f.deletions).sum::<usize>(),
                d.files.len(),
            )
        })
        .unwrap_or_default();
    let theme = cx.theme();
    let (border, muted_fg, success, danger) =
        (theme.border, theme.muted_foreground, theme.success, theme.danger);

    let summary = h_flex()
        .h(px(28.))
        .flex_shrink_0()
        .px_2()
        .gap_2()
        .items_center()
        .border_b_1()
        .border_color(border)
        .child(
            div()
                .flex_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(muted_fg)
                .child(format!("{} file{} changed", count, if count == 1 { "" } else { "s" })),
        )
        .child(div().text_size(px(10.)).text_color(success).child(format!("+{}", added)))
        .child(div().text_size(px(10.)).text_color(danger).child(format!("-{}", removed)));

    let body: AnyElement = if gm.commit_file_rows.is_empty() {
        placeholder(IconName::Check, "No file changes", cx).into_any_element()
    } else {
        let list = uniform_list(
            "git-commit-files",
            gm.commit_file_rows.len(),
            cx.processor(|gm, range: std::ops::Range<usize>, _window, cx| {
                let rows = gm.commit_file_rows.clone();
                range
                    .filter_map(|index| rows.get(index).cloned())
                    .map(|row| render_file_row(gm, row, cx))
                    .collect::<Vec<AnyElement>>()
            }),
        );
        virtual_panel(list, &gm.commit_files_list).into_any_element()
    };

    v_flex().size_full().child(summary).child(body).into_any_element()
}

fn render_file_row(
    gm: &mut GitManager,
    row: TreeItem<crate::FileChange>,
    cx: &mut Context<GitManager>,
) -> AnyElement {
    let theme = cx.theme();
    let (foreground, muted_fg, list_active, list_hover, accent, radius, success, danger) = (
        theme.foreground,
        theme.muted_foreground,
        theme.list_active,
        theme.list_hover,
        theme.list_active_border,
        theme.radius,
        theme.success,
        theme.danger,
    );
    let tree = gm.layout == GitLayout::Tree;

    match row {
        TreeItem::Folder {
            key,
            name,
            depth,
            collapsed,
            ..
        } => {
            let toggle = key.clone();
            h_flex()
                .id(SharedString::from(format!("commit-folder-{}", key)))
                .w_full()
                .h(px(ROW_HEIGHT))
                .pl(px(6.0 + INDENT * depth as f32))
                .pr_2()
                .gap_1()
                .items_center()
                .cursor_pointer()
                .hover(move |s| s.bg(list_hover))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.toggle_collapsed(&toggle, cx);
                    }),
                )
                .child(
                    Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                        .size(px(12.))
                        .text_color(muted_fg),
                )
                .child(
                    Icon::new(if collapsed { IconName::Folder } else { IconName::FolderOpen })
                        .size(px(13.))
                        .text_color(muted_fg),
                )
                .child(div().text_xs().text_color(foreground).child(name))
                .into_any_element()
        }
        TreeItem::Leaf { item: change, depth } => {
            let selected = matches!(&gm.target, Some(DiffTarget::Commit { path, .. }) if *path == change.path);
            let status_fg = status_color(change.status, cx);
            let for_select = change.clone();
            let menu_path = change.path.clone();
            let directory = change.directory().to_string();
            h_flex()
                .id(SharedString::from(format!("commit-file-{}", change.path)))
                .w_full()
                .h(px(ROW_HEIGHT))
                .pl(px(6.0 + INDENT * depth as f32 + if tree { 16.0 } else { 0.0 }))
                .pr_2()
                .gap_1p5()
                .items_center()
                .rounded(radius)
                .cursor_pointer()
                .bg(if selected { list_active } else { transparent_black() })
                .hover(move |s| s.bg(if selected { list_active } else { list_hover }))
                .when(selected, |this| this.border_l_2().border_color(accent))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.select_commit_file(&for_select, cx);
                    }),
                )
                .context_menu(commit_file_menu(menu_path))
                .child(
                    div()
                        .w(px(14.))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(status_fg)
                        .child(change.status.short_str()),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1p5()
                        .items_baseline()
                        .overflow_hidden()
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(foreground)
                                .child(change.file_name().to_string()),
                        )
                        .when(!tree, |this| {
                            this.child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(10.))
                                    .text_color(muted_fg)
                                    .child(directory),
                            )
                        }),
                )
                .when(change.additions + change.deletions > 0, |this| {
                    this.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(success)
                            .child(format!("+{}", change.additions)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(danger)
                            .child(format!("-{}", change.deletions)),
                    )
                })
                .into_any_element()
        }
    }
}
