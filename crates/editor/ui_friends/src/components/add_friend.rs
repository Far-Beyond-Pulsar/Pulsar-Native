use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    v_flex, ActiveTheme as _, Disableable, Icon, IconName,
};

use crate::screen::FriendsScreen;
use crate::utils::types::AddFriendState;

pub fn render_add_friend_bar(
    screen: &FriendsScreen,
    cx: &mut Context<FriendsScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let is_busy = matches!(
        screen.add_friend_state,
        AddFriendState::Sending | AddFriendState::CheckingGist
    );

    v_flex()
        .w_full()
        .gap_2()
        .child(
            h_flex()
                .w_full()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .h(px(40.))
                        .rounded_lg()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.popover)
                        .overflow_hidden()
                        .child(
                            h_flex()
                                .w_full()
                                .h_full()
                                .px_3()
                                .gap_2()
                                .items_center()
                                .child(
                                    Icon::new(IconName::Search)
                                        .size(px(15.))
                                        .text_color(theme.muted_foreground),
                                )
                                .child(div().flex_1().h_full().child(
                                    ui::input::TextInput::new(
                                        screen.add_friend_input.as_ref().unwrap(),
                                    ),
                                )),
                        ),
                )
                .child(
                    Button::new("send-friend-request")
                        .primary()
                        .label(
                            if matches!(screen.add_friend_state, AddFriendState::CheckingGist) {
                                "Checking"
                            } else {
                                "Add Friend"
                            },
                        )
                        .disabled(is_busy || screen.add_friend_username.trim().is_empty())
                        .on_click(cx.listener(|this, _, _, cx| {
                            crate::handlers::on_send_friend_request(this, cx);
                        })),
                ),
        )
        .when(
            matches!(screen.add_friend_state, AddFriendState::CheckingGist),
            |this| {
                this.child(
                    h_flex().gap_2().items_center().child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child("Checking if user has Pulsar Engine set up..."),
                    ),
                )
            },
        )
        .when(
            matches!(screen.add_friend_state, AddFriendState::Success),
            |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Icon::new(IconName::Check)
                                .size(px(13.))
                                .text_color(theme.success),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.success)
                                .child("Request Sent!"),
                        ),
                )
            },
        )
        .when(
            matches!(screen.add_friend_state, AddFriendState::GistNotFound),
            |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .size(px(13.))
                                .text_color(theme.warning),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.warning)
                                .child("This user hasn't set up Pulsar Engine friends yet"),
                        ),
                )
            },
        )
        .when(
            matches!(screen.add_friend_state, AddFriendState::SelfFriended),
            |this| {
                this.child(
                    h_flex().gap_2().items_center().child(
                        div()
                            .text_sm()
                            .text_color(theme.warning)
                            .child("You can't be friends with yourself... or can you? 🌟"),
                    ),
                )
            },
        )
        .when_some(
            match &screen.add_friend_state {
                AddFriendState::Error(msg) => Some(msg.clone()),
                _ => None,
            },
            |this, msg| {
                this.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .size(px(13.))
                                .text_color(theme.danger),
                        )
                        .child(div().text_sm().text_color(theme.danger).child(msg)),
                )
            },
        )
}
