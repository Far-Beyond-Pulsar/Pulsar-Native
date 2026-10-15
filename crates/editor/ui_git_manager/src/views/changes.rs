//! Working-tree changes: the virtualized file list and the commit box.

use super::common::{file_menu, placeholder, status_color, virtual_panel};
use crate::history::DiffTarget;
use crate::{ChangeRow, GitLayout, GitManager};
use gpui::{prelude::FluentBuilder as _, *};
use std::rc::Rc;
use ui::{
    ActiveTheme as _, Disableable as _, Icon, IconName,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::TextInput,
    menu::context_menu::ContextMenuExt as _,
    v_flex, v_virtual_list,
};

const ROW_HEIGHT: f32 = 28.0;
const INDENT: f32 = 12.0;

pub(crate) fn render_changes_panel(
    gm: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (border, muted_fg) = (theme.border, theme.muted_foreground);
    let flat = gm.layout == GitLayout::Desktop;

    let staged_count = gm.repo_state.staged_files.len();
    let changed_count = gm.changes_rows.len();

    let body: AnyElement = if gm.changes_rows.is_empty() {
        placeholder(IconName::Check, "No local changes", cx).into_any_element()
    } else {
        let sizes: Rc<Vec<Size<Pixels>>> = Rc::new(
            gm.changes_rows
                .iter()
                .map(|_| size(px(0.0), px(ROW_HEIGHT)))
                .collect(),
        );
        let entity = cx.entity().clone();
        let list = v_virtual_list(
            entity,
            "git-changes-list",
            sizes,
            move |gm: &mut GitManager, range: std::ops::Range<usize>, _window, cx| {
                let rows: Vec<ChangeRow> = range
                    .filter_map(|index| gm.changes_rows.get(index).cloned())
                    .collect();
                rows.into_iter()
                    .map(|row| render_row(gm, row, cx))
                    .collect::<Vec<AnyElement>>()
            },
        )
        .track_scroll(&gm.changes_list.scroll);
        virtual_panel(list, &gm.changes_list).into_any_element()
    };

    let select_all = flat.then(|| {
        let all_staged = changed_count > 0 && gm.changes_rows.iter().all(|row| {
            matches!(row, ChangeRow::Flat { staged: true, unstaged: false, .. })
        });
        h_flex()
            .h(px(30.))
            .flex_shrink_0()
            .px_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(border)
            .child(
                Checkbox::new("select-all-changes")
                    .checked(all_staged)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        if *checked {
                            this.stage_all(cx);
                        } else {
                            this.unstage_all(cx);
                        }
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted_fg)
                    .child(format!(
                        "{} changed file{}",
                        changed_count,
                        if changed_count == 1 { "" } else { "s" }
                    )),
            )
    });

    v_flex()
        .size_full()
        .children(select_all)
        .child(body)
        .child(render_commit_box(gm, staged_count, cx))
}

fn render_row(gm: &mut GitManager, row: ChangeRow, cx: &mut Context<GitManager>) -> AnyElement {
    let theme = cx.theme();
    let (foreground, muted_fg, list_active, list_hover, accent, radius, warning) = (
        theme.foreground,
        theme.muted_foreground,
        theme.list_active,
        theme.list_hover,
        theme.list_active_border,
        theme.radius,
        theme.warning,
    );

    match row {
        ChangeRow::Section {
            staged,
            count,
            collapsed,
        } => {
            let key = if staged { "section:staged" } else { "section:changes" };
            h_flex()
                .id(SharedString::from(key))
                .w_full()
                .h(px(ROW_HEIGHT))
                .px_1()
                .gap_1()
                .items_center()
                .cursor_pointer()
                .hover(move |s| s.bg(list_hover))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.toggle_collapsed(key, cx);
                    }),
                )
                .child(
                    Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                        .size(px(12.))
                        .text_color(muted_fg),
                )
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(muted_fg)
                        .child(format!("{} ({})", if staged { "Staged" } else { "Changes" }, count)),
                )
                .child(
                    Button::new(SharedString::from(format!("section-action-{}", staged)))
                        .icon(Icon::new(if staged { IconName::Minus } else { IconName::Plus }).text_color(warning))
                        .ghost()
                        .compact()
                        .tooltip(if staged { "Unstage all" } else { "Stage all" })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if staged {
                                this.unstage_all(cx);
                            } else {
                                this.stage_all(cx);
                            }
                        })),
                )
                .into_any_element()
        }
        ChangeRow::Folder {
            key,
            name,
            depth,
            count,
            collapsed,
        } => {
            let toggle = key.clone();
            h_flex()
                .id(SharedString::from(format!("folder-{}", key)))
                .w_full()
                .h(px(ROW_HEIGHT))
                .pl(px(4.0 + INDENT * depth as f32))
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
                .child(div().flex_1().text_xs().text_color(foreground).child(name))
                .child(div().text_size(px(10.)).text_color(muted_fg).child(count.to_string()))
                .into_any_element()
        }
        ChangeRow::File {
            change,
            staged,
            depth,
        } => {
            let selected = match &gm.target {
                Some(DiffTarget::Staged { path, .. }) => staged && *path == change.path,
                Some(DiffTarget::Unstaged { path }) => !staged && *path == change.path,
                _ => false,
            };
            let status_fg = status_color(change.status, cx);
            let path = change.path.clone();
            let for_select = change.clone();
            let for_stage = change.path.clone();
            h_flex()
                .id(SharedString::from(format!("file-{}-{}", staged, change.path)))
                .w_full()
                .h(px(ROW_HEIGHT))
                .pl(px(4.0 + INDENT * depth as f32 + 16.0))
                .pr_1()
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
                        this.select_working_file(&for_select, staged, cx);
                    }),
                )
                .context_menu(file_menu(path))
                .child(
                    div()
                        .w(px(14.))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(status_fg)
                        .child(change.status.short_str()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_xs()
                        .text_color(foreground)
                        .child(match &change.old_path {
                            Some(old) => format!("{} → {}", old.rsplit('/').next().unwrap_or(old), change.file_name()),
                            None => change.file_name().to_string(),
                        }),
                )
                .child(
                    Button::new(SharedString::from(format!("stage-{}-{}", staged, change.path)))
                        .icon(if staged { IconName::Minus } else { IconName::Plus })
                        .ghost()
                        .compact()
                        .tooltip(if staged { "Unstage" } else { "Stage" })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if staged {
                                this.unstage_file(for_stage.clone(), cx);
                            } else {
                                this.stage_file(for_stage.clone(), cx);
                            }
                        })),
                )
                .into_any_element()
        }
        ChangeRow::Flat {
            change,
            staged,
            unstaged,
        } => {
            let selected = match &gm.target {
                Some(DiffTarget::Staged { path, .. }) | Some(DiffTarget::Unstaged { path }) => {
                    *path == change.path
                }
                _ => false,
            };
            let status_fg = status_color(change.status, cx);
            let path = change.path.clone();
            let for_select = change.clone();
            let for_toggle = change.path.clone();
            let directory = change.directory().to_string();
            h_flex()
                .id(SharedString::from(format!("flat-{}", change.path)))
                .w_full()
                .h(px(ROW_HEIGHT))
                .px_2()
                .gap_2()
                .items_center()
                .rounded(radius)
                .cursor_pointer()
                .bg(if selected { list_active } else { transparent_black() })
                .hover(move |s| s.bg(if selected { list_active } else { list_hover }))
                .when(selected, |this| this.border_l_2().border_color(accent))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        // A file with unstaged edits is diffed against the
                        // index; a fully staged one against HEAD.
                        this.select_working_file(&for_select, staged && !unstaged, cx);
                    }),
                )
                .context_menu(file_menu(path))
                .child(
                    Checkbox::new(SharedString::from(format!("check-{}", change.path)))
                        .checked(staged)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            if *checked {
                                this.stage_file(for_toggle.clone(), cx);
                            } else {
                                this.unstage_file(for_toggle.clone(), cx);
                            }
                        })),
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
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(10.))
                                .text_color(muted_fg)
                                .child(directory),
                        ),
                )
                .child(
                    div()
                        .w(px(14.))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(status_fg)
                        .child(change.status.short_str()),
                )
                .into_any_element()
        }
    }
}

/// Summary + description inputs and the commit button.
fn render_commit_box(
    gm: &mut GitManager,
    staged_count: usize,
    cx: &mut Context<GitManager>,
) -> impl IntoElement + use<> {
    let border = cx.theme().border;
    let muted_fg = cx.theme().muted_foreground;
    let branch = gm.repo_state.current_branch.clone();
    let busy = gm.commit_busy;
    let merging = gm.repo_state.operation.is_some();
    let can_commit = (staged_count > 0 || merging) && !busy;

    let label = if busy {
        "Committing…".to_string()
    } else if merging {
        format!("Complete merge on {}", branch)
    } else if staged_count > 0 {
        format!(
            "Commit {} file{} to {}",
            staged_count,
            if staged_count == 1 { "" } else { "s" },
            branch
        )
    } else {
        format!("Commit to {}", branch)
    };

    v_flex()
        .flex_shrink_0()
        .p_2()
        .gap_1p5()
        .border_t_1()
        .border_color(border)
        .child(TextInput::new(&gm.commit_message_input))
        .child(TextInput::new(&gm.commit_description_input))
        .child(
            Button::new("commit-button")
                .label(label)
                .primary()
                .w_full()
                .disabled(!can_commit)
                .on_click(cx.listener(|this, _, _, cx| this.commit_changes(cx))),
        )
        .when(staged_count == 0 && !merging, |this| {
            this.child(
                div()
                    .text_size(px(10.))
                    .text_color(muted_fg)
                    .child("Stage files to commit them."),
            )
        })
}
