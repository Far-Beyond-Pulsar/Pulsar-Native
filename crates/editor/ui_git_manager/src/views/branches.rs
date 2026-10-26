//! Branch tree (Tree layout sidebar) and the branch menu (both layouts).

use super::chrome::TOP_BAR_HEIGHT;
use super::common::{pill, placeholder, virtual_panel};
use crate::{BranchRow, GitManager, ListState};
use gpui::{prelude::FluentBuilder as _, *};
use ui::{ActiveTheme as _, Icon, IconName, h_flex, input::TextInput, v_flex};

const ROW_HEIGHT: f32 = 26.0;
const INDENT: f32 = 12.0;

fn virtual_rows(
    id: &'static str,
    rows_of: fn(&GitManager) -> &std::rc::Rc<Vec<BranchRow>>,
    list_of: fn(&GitManager) -> &ListState,
    gm: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> Div {
    let list = uniform_list(
        id,
        rows_of(gm).len(),
        cx.processor(move |gm, range: std::ops::Range<usize>, _window, cx| {
            let rows = rows_of(gm).clone();
            range
                .filter_map(|index| rows.get(index).cloned())
                .map(|row| render_row(row, cx))
                .collect::<Vec<AnyElement>>()
        }),
    );
    virtual_panel(list, list_of(gm))
}

/// Sidebar tree of local and remote branches.
pub(crate) fn render_branch_tree(
    gm: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> impl IntoElement + use<> {
    if gm.branch_rows.is_empty() {
        return placeholder(IconName::GitBranch, "No branches yet", cx).into_any_element();
    }
    virtual_rows(
        "git-branch-tree",
        |gm| &gm.branch_rows,
        |gm| &gm.branch_list,
        gm,
        cx,
    )
    .into_any_element()
}

/// The branch switcher dropped from the top bar: filter, switch, create.
pub(crate) fn render_branch_menu(
    gm: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (background, border, muted_fg, radius) = (
        theme.background,
        theme.border,
        theme.muted_foreground,
        theme.radius,
    );

    let body: AnyElement = if gm.menu_rows.is_empty() {
        div()
            .p_4()
            .text_xs()
            .text_color(muted_fg)
            .child("No matching branches")
            .into_any_element()
    } else {
        virtual_rows(
            "git-branch-menu",
            |gm| &gm.menu_rows,
            |gm| &gm.menu_list,
            gm,
            cx,
        )
        .into_any_element()
    };

    // A transparent backdrop closes the menu on an outside click.
    div()
        .absolute()
        .inset_0()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_branch_menu(cx)),
        )
        .child(
            v_flex()
                .absolute()
                .top(px(TOP_BAR_HEIGHT + 34.0))
                .left(px(130.))
                .w(px(340.))
                .h(px(420.))
                .rounded(radius)
                .border_1()
                .border_color(border)
                .bg(background)
                .shadow_lg()
                .overflow_hidden()
                .occlude()
                .child(
                    div()
                        .p_2()
                        .border_b_1()
                        .border_color(border)
                        .child(TextInput::new(&gm.branch_filter_input)),
                )
                .child(body),
        )
}

fn render_row(row: BranchRow, cx: &mut Context<GitManager>) -> AnyElement {
    let theme = cx.theme();
    let (foreground, muted_fg, list_hover, list_active, primary, success) = (
        theme.foreground,
        theme.muted_foreground,
        theme.list_hover,
        theme.list_active,
        theme.primary,
        theme.success,
    );

    match row {
        BranchRow::Section {
            key,
            title,
            count,
            collapsed,
        } => {
            let toggle = key.clone();
            let collapsible = key.starts_with("branches:");
            h_flex()
                .id(SharedString::from(format!("branch-section-{}", key)))
                .w_full()
                .h(px(ROW_HEIGHT))
                .px_2()
                .gap_1()
                .items_center()
                .when(collapsible, |this| {
                    this.cursor_pointer()
                        .hover(move |s| s.bg(list_hover))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                this.toggle_collapsed(&toggle, cx);
                            }),
                        )
                        .child(
                            Icon::new(if collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            })
                            .size(px(12.))
                            .text_color(muted_fg),
                        )
                })
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(muted_fg)
                        .child(format!("{} ({})", title, count)),
                )
                .into_any_element()
        }
        BranchRow::Folder {
            key,
            name,
            depth,
            collapsed,
        } => {
            let toggle = key.clone();
            h_flex()
                .id(SharedString::from(format!("branch-folder-{}", key)))
                .w_full()
                .h(px(ROW_HEIGHT))
                .pl(px(8.0 + INDENT * depth as f32))
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
                    Icon::new(if collapsed {
                        IconName::ChevronRight
                    } else {
                        IconName::ChevronDown
                    })
                    .size(px(12.))
                    .text_color(muted_fg),
                )
                .child(
                    Icon::new(if collapsed {
                        IconName::Folder
                    } else {
                        IconName::FolderOpen
                    })
                    .size(px(13.))
                    .text_color(muted_fg),
                )
                .child(div().text_xs().text_color(foreground).child(name))
                .into_any_element()
        }
        BranchRow::Branch {
            branch,
            label,
            depth,
        } => {
            let target = branch.name.clone();
            let current = branch.is_current;
            h_flex()
                .id(SharedString::from(format!(
                    "branch-{}-{}",
                    branch.is_remote, branch.name
                )))
                .w_full()
                .h(px(ROW_HEIGHT))
                .pl(px(8.0 + INDENT * depth as f32 + 16.0))
                .pr_2()
                .gap_1p5()
                .items_center()
                .cursor_pointer()
                .bg(if current {
                    list_active
                } else {
                    transparent_black()
                })
                .hover(move |s| s.bg(if current { list_active } else { list_hover }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        if !current {
                            this.switch_branch(target.clone(), cx);
                        } else {
                            this.close_branch_menu(cx);
                        }
                    }),
                )
                .child(
                    Icon::new(if current {
                        IconName::Check
                    } else {
                        IconName::GitBranch
                    })
                    .size(px(12.))
                    .text_color(if current { success } else { muted_fg }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_xs()
                        .font_weight(if current {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(foreground)
                        .child(label),
                )
                .when(current, |this| this.child(pill("current", primary, false)))
                .into_any_element()
        }
        BranchRow::Create { name } => {
            let create = name.clone();
            h_flex()
                .id("branch-create")
                .w_full()
                .h(px(ROW_HEIGHT))
                .px_2()
                .gap_1p5()
                .items_center()
                .cursor_pointer()
                .hover(move |s| s.bg(list_hover))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.create_branch(create.clone(), cx);
                    }),
                )
                .child(Icon::new(IconName::Plus).size(px(12.)).text_color(primary))
                .child(
                    div()
                        .text_xs()
                        .text_color(primary)
                        .child(format!("Create branch \"{}\"", name)),
                )
                .into_any_element()
        }
    }
}
