//! Tree layout: sidebar tree (changes + branches), a virtualized commit
//! graph, and the commit / diff detail pane.

use super::branches::render_branch_tree;
use super::changes::render_changes_panel;
use super::commit_detail::render_commit_detail;
use super::commit_list::render_commit_list;
use super::diff::render_diff_panel;
use crate::{DetailFocus, GitManager};
use gpui::*;
use ui::{ActiveTheme as _, h_flex, v_flex};

const SIDEBAR_WIDTH: f32 = 300.0;
const BRANCHES_HEIGHT: f32 = 230.0;

fn section_title(text: &'static str, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .h(px(26.))
        .flex_shrink_0()
        .px_3()
        .flex()
        .items_center()
        .text_size(px(10.))
        .font_weight(FontWeight::BOLD)
        .text_color(theme.muted_foreground)
        .bg(theme.tab_bar)
        .border_b_1()
        .border_color(theme.border)
        .child(text)
}

pub(crate) fn render_tree_layout(
    gm: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> impl IntoElement + use<> {
    let border = cx.theme().border;
    let focus = gm.detail_focus;

    let changes = render_changes_panel(gm, cx);
    let branches = render_branch_tree(gm, cx);
    let history = render_commit_list(gm, true, cx);
    let detail: AnyElement = match focus {
        DetailFocus::Working => render_diff_panel(gm, cx).into_any_element(),
        DetailFocus::Commit => render_commit_detail(gm, cx).into_any_element(),
    };

    h_flex()
        .size_full()
        .child(
            v_flex()
                .w(px(SIDEBAR_WIDTH))
                .h_full()
                .flex_shrink_0()
                .border_r_1()
                .border_color(border)
                .child(section_title("CHANGES", cx))
                .child(div().flex_1().min_h_0().overflow_hidden().child(changes))
                .child(section_title("BRANCHES", cx))
                .child(
                    div()
                        .h(px(BRANCHES_HEIGHT))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .child(branches),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w(px(420.))
                .h_full()
                .border_r_1()
                .border_color(border)
                .child(section_title("HISTORY", cx))
                .child(div().flex_1().min_h_0().overflow_hidden().child(history)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(420.))
                .h_full()
                .overflow_hidden()
                .child(detail),
        )
}
