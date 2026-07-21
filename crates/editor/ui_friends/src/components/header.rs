use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    dropdown::SearchableList,
    h_flex,
    popover::Popover,
    ActiveTheme as _, Disableable, Icon, IconName,
};

use crate::screen::FriendsScreen;

pub fn render_header(
    screen: &FriendsScreen,
    total: usize,
    online_count: usize,
    _pending_count: usize,
    cx: &mut Context<FriendsScreen>,
) -> impl IntoElement {
    let theme = cx.theme();

    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .text_2xl()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child("Friends"),
                )
                .child(
                    div()
                        .px_2p5()
                        .py_1()
                        .rounded_full()
                        .bg(theme.accent.opacity(0.12))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(format!("{} online", online_count)),
                        ),
                )
                .child(
                    div()
                        .px_2p5()
                        .py_1()
                        .rounded_full()
                        .bg(theme.muted.opacity(0.15))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("{} total", total)),
                        ),
                ),
        )
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("refresh-friends")
                        .ghost()
                        .icon(Icon::new(IconName::Refresh).size(px(13.)))
                        .tooltip("Refresh friends list")
                        .on_click(cx.listener(|this, _, _, cx| {
                            crate::handlers::on_refresh_friends(this, cx);
                        })),
                )
                .child(
                    Button::new("fetch-friend-homes")
                        .ghost()
                        .icon(Icon::new(IconName::Globe).size(px(13.)))
                        .label(if screen.fetching_homes {
                            "Fetching..."
                        } else {
                            "Fetch homes"
                        })
                        .disabled(screen.fetching_homes)
                        .tooltip("Fetch home server URLs for non-mutual friends")
                        .on_click(cx.listener(|this, _, _, cx| {
                            crate::handlers::on_fetch_friend_homes(this, cx);
                        })),
                )
                .child(
                    Popover::<SearchableList<String>>::new("friends-popover")
                        .anchor(Corner::BottomRight)
                        .trigger(
                            Button::new("friends-list-btn")
                                .ghost()
                                .icon(Icon::new(IconName::Group).size(px(13.)))
                                .label("Friends"),
                        )
                        .content({
                            let list = screen.friends_list.clone();
                            move |_window, _cx| list.clone()
                        }),
                ),
        )
}
