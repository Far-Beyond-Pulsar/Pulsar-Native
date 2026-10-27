//! Desktop layout: a GitHub Desktop style Changes / History tab strip on the
//! left, the diff (or the commit) on the right.

use super::changes::render_changes_panel;
use super::commit_detail::render_commit_detail;
use super::commit_list::render_commit_list;
use super::diff::render_diff_panel;
use crate::{DesktopTab, GitManager};
use gpui::*;
use ui::{ActiveTheme as _, h_flex, v_flex};

const SIDEBAR_WIDTH: f32 = 340.0;

pub(crate) fn render_desktop_layout(
    gm: &mut GitManager,
    cx: &mut Context<GitManager>,
) -> impl IntoElement + use<> {
    let border = cx.theme().border;
    let tab = gm.desktop_tab;

    let tabs = render_tabs(gm, cx);
    let (left, right): (AnyElement, AnyElement) = match tab {
        DesktopTab::Changes => (
            render_changes_panel(gm, cx).into_any_element(),
            render_diff_panel(gm, cx).into_any_element(),
        ),
        DesktopTab::History => (
            render_commit_list(gm, false, cx).into_any_element(),
            render_commit_detail(gm, cx).into_any_element(),
        ),
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
                .child(tabs)
                .child(div().flex_1().min_h_0().overflow_hidden().child(left)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_hidden()
                .child(right),
        )
}

fn render_tabs(gm: &mut GitManager, cx: &mut Context<GitManager>) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (border, primary, foreground, muted_fg, hover) = (
        theme.border,
        theme.primary,
        theme.foreground,
        theme.muted_foreground,
        theme.list_hover,
    );
    let changes = gm.changes_rows.len();
    let active = gm.desktop_tab;

    let tab = |id: &'static str, label: String, which: DesktopTab| {
        let selected = active == which;
        div()
            .id(id)
            .flex_1()
            .h(px(36.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_xs()
            .font_weight(if selected {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            })
            .text_color(if selected { foreground } else { muted_fg })
            .border_b_2()
            .border_color(if selected {
                primary
            } else {
                transparent_black()
            })
            .hover(move |s| s.bg(hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.set_desktop_tab(which, cx);
                }),
            )
            .child(label)
    };

    h_flex()
        .w_full()
        .flex_shrink_0()
        .border_b_1()
        .border_color(border)
        .child(tab(
            "tab-changes",
            if changes > 0 {
                format!("Changes ({})", changes)
            } else {
                "Changes".to_string()
            },
            DesktopTab::Changes,
        ))
        .child(tab(
            "tab-history",
            "History".to_string(),
            DesktopTab::History,
        ))
}
