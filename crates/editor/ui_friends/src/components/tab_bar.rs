use gpui::{prelude::*, *};
use ui::{h_flex, ActiveTheme as _};

use crate::screen::FriendsScreen;
use crate::utils::types::FriendTab;

pub fn render_tabs(
    screen: &FriendsScreen,
    pending_count: usize,
    cx: &mut Context<FriendsScreen>,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .gap_1()
        .child(render_tab_button(screen, "Online", FriendTab::Online, None, cx))
        .child(render_tab_button(
            screen,
            "Pending",
            FriendTab::Pending,
            Some(pending_count),
            cx,
        ))
        .child(render_tab_button(screen, "All", FriendTab::All, None, cx))
}

pub fn render_tab_button(
    screen: &FriendsScreen,
    label: &'static str,
    tab: FriendTab,
    badge: Option<usize>,
    cx: &mut Context<FriendsScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let is_active = screen.view == tab;
    let accent = theme.accent;

    h_flex()
        .id(SharedString::from(format!("tab-{}", label.to_lowercase())))
        .gap_2()
        .items_center()
        .px_4()
        .py_2()
        .rounded_lg()
        .cursor_pointer()
        .when(is_active, |this| this.bg(accent.opacity(0.1)))
        .hover(|this| {
            this.bg(if is_active {
                accent.opacity(0.12)
            } else {
                theme.muted.opacity(0.08)
            })
        })
        .child(
            div()
                .text_sm()
                .font_weight(if is_active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if is_active {
                    accent
                } else {
                    theme.muted_foreground
                })
                .child(label),
        )
        .when_some(badge, |this, count| {
            this.child(
                div()
                    .min_w(px(18.))
                    .h(px(18.))
                    .px_1p5()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if is_active {
                        accent
                    } else {
                        theme.muted.opacity(0.2)
                    })
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(if is_active {
                                theme.accent_foreground
                            } else {
                                theme.muted_foreground
                            })
                            .child(format!("{}", count)),
                    ),
            )
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            this.view = tab;
            cx.notify();
        }))
}
