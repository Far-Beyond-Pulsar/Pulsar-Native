use friends_engine::RelationStatus;
use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    skeleton::Skeleton,
    v_flex, ActiveTheme as _, Icon, IconName,
};

use crate::screen::FriendsScreen;
use crate::utils::types::{FriendEntry, FriendTab};

pub fn filtered_friends(screen: &FriendsScreen) -> Vec<&FriendEntry> {
    screen
        .friends
        .iter()
        .filter(|f| match screen.view {
            FriendTab::Online => f.online && f.relation_status == RelationStatus::Mutual,
            FriendTab::Pending => {
                f.relation_status == RelationStatus::PendingInbound
                    || f.relation_status == RelationStatus::PendingOutbound
            }
            FriendTab::All => true,
        })
        .collect()
}

pub fn render_friend_row(
    screen: &FriendsScreen,
    friend: &FriendEntry,
    cx: &mut Context<FriendsScreen>,
) -> AnyElement {
    if friend.is_self {
        return render_self_row(screen, cx);
    }
    let border_col = cx.theme().border;
    let bg_col = cx.theme().background;
    let fg = cx.theme().foreground;
    let muted_fg = cx.theme().muted_foreground;
    let success = cx.theme().success;
    let muted = cx.theme().muted;

    let dot_color = if friend.online {
        success
    } else {
        muted_fg.opacity(0.4)
    };

    v_flex()
        .w_full()
        .child(
            h_flex()
                .w_full()
                .gap_3()
                .items_center()
                .px_4()
                .py_3()
                .rounded_xl()
                .hover(|this| this.bg(muted.opacity(0.06)))
                .cursor_pointer()
                .child(
                    div()
                        .flex_shrink_0()
                        .relative()
                        .child(render_avatar(screen, friend, cx))
                        .child(
                            div()
                                .absolute()
                                .bottom(px(0.))
                                .right(px(0.))
                                .w(px(12.))
                                .h(px(12.))
                                .rounded_full()
                                .border_2()
                                .border_color(bg_col)
                                .bg(dot_color),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg)
                                .child(format!("@{}", friend.username)),
                        )
                        .when_some(friend.current_project.as_ref(), |this, project| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(format!("Working on: {}", project)),
                            )
                        })
                        .when(friend.current_project.is_none() && friend.online, |this| {
                            this.child(div().text_xs().text_color(muted_fg).child("Online"))
                        })
                        .when(friend.current_project.is_none() && !friend.online, |this| {
                            this.child(
                                div().text_xs().text_color(muted_fg.opacity(0.5)).child(
                                    friend
                                        .last_seen
                                        .clone()
                                        .map(|d| format!("Last seen: {}", d))
                                        .unwrap_or_else(|| "Offline".to_string()),
                                ),
                            )
                        }),
                )
                .child(match friend.relation_status {
                    RelationStatus::PendingInbound => render_pending_inbound_actions(friend, cx)
                        .into_any_element(),
                    RelationStatus::PendingOutbound => {
                        render_pending_outbound_state(cx).into_any_element()
                    }
                    RelationStatus::Mutual => render_mutual_actions(cx).into_any_element(),
                }),
        )
        .child(div().w_full().h(px(1.)).bg(border_col.opacity(0.4)).mx_4())
        .into_any_element()
}

pub fn render_avatar(
    screen: &FriendsScreen,
    friend: &FriendEntry,
    cx: &mut Context<FriendsScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let avatar = screen
        .avatar_cache
        .get(&friend.pfp_url)
        .and_then(|o| o.clone());

    div()
        .w(px(40.))
        .h(px(40.))
        .rounded_full()
        .bg(theme.muted.opacity(0.2))
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .child(if let Some(avatar_img) = avatar {
            img(ImageSource::Render(avatar_img))
                .w(px(40.))
                .h(px(40.))
                .rounded_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element()
        } else {
            let initial = friend
                .username
                .chars()
                .next()
                .map(|c| c.to_ascii_uppercase().to_string())
                .unwrap_or_else(|| "?".to_string());
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child(initial)
                .into_any_element()
        })
}

fn render_self_row(screen: &FriendsScreen, cx: &Context<FriendsScreen>) -> AnyElement {
    let theme = cx.theme();

    let self_entry = screen.friends.iter().find(|f| f.is_self);
    let avatar = self_entry
        .and_then(|f| screen.avatar_cache.get(&f.pfp_url).and_then(|o| o.clone()));

    h_flex()
        .w_full()
        .px_3()
        .py_2()
        .rounded_lg()
        .gap_3()
        .items_center()
        .border_1()
        .border_color(theme.warning.opacity(0.3))
        .bg(theme.warning.opacity(0.05))
        .child(
            div()
                .w(px(36.))
                .h(px(36.))
                .rounded_full()
                .bg(theme.muted.opacity(0.2))
                .overflow_hidden()
                .flex()
                .items_center()
                .justify_center()
                .child(if let Some(avatar_img) = avatar {
                    img(ImageSource::Render(avatar_img))
                        .w(px(36.))
                        .h(px(36.))
                        .rounded_full()
                        .object_fit(ObjectFit::Cover)
                        .into_any_element()
                } else {
                    Icon::new(IconName::Heart)
                        .size(px(16.))
                        .text_color(theme.warning)
                        .into_any_element()
                }),
        )
        .child(
            v_flex()
                .flex_1()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child("Yourself"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.warning)
                        .child("Self-love is the best love 💫"),
                ),
        )
        .child(
            div()
                .px_2()
                .py_0p5()
                .rounded_full()
                .bg(theme.warning.opacity(0.15))
                .border_1()
                .border_color(theme.warning.opacity(0.3))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.warning)
                        .child("Friend"),
                ),
        )
        .into_any_element()
}

pub fn render_pending_inbound_actions(
    friend: &FriendEntry,
    cx: &mut Context<FriendsScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let username = friend.username.clone();

    h_flex()
        .gap_2()
        .child(
            Button::new(format!("accept-{}", username))
                .ghost()
                .icon(Icon::new(IconName::Check).size(px(15.)))
                .tooltip("Accept")
                .on_click({
                    let uname = username.clone();
                    cx.listener(move |this, _, _, cx| {
                        crate::handlers::on_accept_request(this, &uname, cx);
                    })
                }),
        )
        .child(
            Button::new(format!("decline-{}", username))
                .ghost()
                .icon(
                    Icon::new(IconName::Close)
                        .size(px(15.))
                        .text_color(theme.danger),
                )
                .tooltip("Decline")
                .on_click({
                    let uname = username.clone();
                    cx.listener(move |this, _, _, cx| {
                        crate::handlers::on_decline_request(this, &uname, cx);
                    })
                }),
        )
}

pub fn render_pending_outbound_state(cx: &mut Context<FriendsScreen>) -> impl IntoElement {
    let theme = cx.theme();
    h_flex().gap_2().items_center().child(
        div()
            .px_3()
            .py_1()
            .rounded_full()
            .bg(theme.muted.opacity(0.1))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .font_weight(FontWeight::MEDIUM)
                    .child("Pending"),
            ),
    )
}

pub fn render_mutual_actions(cx: &mut Context<FriendsScreen>) -> impl IntoElement {
    h_flex().gap_1().child(
        Button::new("friend-actions")
            .ghost()
            .icon(Icon::new(IconName::ArrowRight).size(px(15.)))
            .tooltip("Inspect project"),
    )
}

pub fn render_loading_state(cx: &mut Context<FriendsScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let border_col = theme.border;

    v_flex().w_full().gap_3().children((0..5).map(|i| {
        h_flex()
            .id(SharedString::from(format!("friend-skel-{}", i)))
            .w_full()
            .gap_3()
            .items_center()
            .px_4()
            .py_3()
            .rounded_xl()
            .border_1()
            .border_color(border_col)
            .child(Skeleton::new().w(px(40.)).h(px(40.)).rounded(px(40.)))
            .child(
                v_flex()
                    .flex_1()
                    .gap_2()
                    .child(Skeleton::new().w(px(140.)).h_4())
                    .child(Skeleton::new().secondary(true).w(px(90.)).h_3()),
            )
            .child(Skeleton::new().w(px(70.)).h(px(28.)).rounded(px(6.)))
    }))
}

pub fn render_not_authenticated(cx: &mut Context<FriendsScreen>) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .w_full()
        .items_center()
        .justify_center()
        .py_16()
        .gap_4()
        .child(
            Icon::new(IconName::Github)
                .size(px(48.))
                .text_color(theme.muted_foreground.opacity(0.3)),
        )
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child("Sign in with GitHub"),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("Connect your GitHub account to find and add friends."),
        )
}

pub fn render_empty_state(
    screen: &FriendsScreen,
    cx: &mut Context<FriendsScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let (icon, title, subtitle) = match screen.view {
        FriendTab::Online => (
            IconName::User,
            "No friends online",
            "When your friends come online, they'll appear here.",
        ),
        FriendTab::Pending => (
            IconName::Send,
            "No pending requests",
            "Friend requests you send or receive will show up here.",
        ),
        FriendTab::All => (
            IconName::Group,
            "No friends yet",
            "Type a GitHub username above and click Add Friend to get started.",
        ),
    };

    v_flex()
        .w_full()
        .items_center()
        .justify_center()
        .py_16()
        .gap_4()
        .child(
            Icon::new(icon)
                .size(px(40.))
                .text_color(theme.muted_foreground.opacity(0.25)),
        )
        .child(
            div()
                .text_base()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child(title),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground.opacity(0.6))
                .child(subtitle),
        )
}
