use std::collections::HashMap;
use std::sync::Arc;

use friends_engine::{FriendInfo, RelationStatus};
use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    dropdown::{SearchableList, SearchableListEvent},
    h_flex,
    input::{InputEvent, InputState},
    popover::Popover,
    skeleton::Skeleton,
    v_flex, ActiveTheme as _, Disableable, Icon, IconName,
    StyledExt,
};

use crate::components::{render_add_friend_bar, render_empty_state, render_friend_row, render_header, render_loading_state, render_not_authenticated, render_tabs, filtered_friends};
use crate::utils::types::{AddFriendState, FriendEntry, FriendTab};

pub struct FriendsScreen {
    pub view: FriendTab,
    pub friends: Vec<FriendEntry>,
    pub loading: bool,
    pub fetching_homes: bool,
    pub show_invite: bool,
    pub friends_list: Entity<SearchableList<String>>,
    pub add_friend_input: Option<Entity<InputState>>,
    pub add_friend_username: String,
    pub add_friend_state: AddFriendState,
    pub avatar_cache: HashMap<String, Option<Arc<RenderImage>>>,
    pub _subscriptions: Vec<Subscription>,
    pub focus_handle: FocusHandle,
}

impl EventEmitter<DismissEvent> for FriendsScreen {}

impl Focusable for FriendsScreen {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl FriendsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_options(window, cx, true)
    }

    pub fn new_without_invite(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_options(window, cx, false)
    }

    fn new_with_options(window: &mut Window, cx: &mut Context<Self>, show_invite: bool) -> Self {
        let friends_list = cx.new(|cx| {
            SearchableList::<String>::new(window, cx, Vec::<String>::new(), |u: &String| u.clone())
                .with_empty_text("No friends yet")
                .with_max_width(px(280.0))
                .with_max_height(px(360.0))
        });

        let mut subscriptions = vec![cx.subscribe(
            &friends_list,
            |_this: &mut Self, _list, event: &SearchableListEvent<String>, cx| {
                if let SearchableListEvent::Select(_) = event {
                    cx.emit(DismissEvent);
                }
            },
        )];

        let add_friend_input;
        if show_invite {
            add_friend_input =
                Some(cx.new(|cx| InputState::new(window, cx).placeholder("GitHub username")));
            if let Some(ref input) = add_friend_input {
                let input = input.clone();
                subscriptions.push(cx.subscribe(
                    &input,
                    |this: &mut Self, _input, event: &InputEvent, cx| match event {
                        InputEvent::Change => {
                            this.add_friend_username = this
                                .add_friend_input
                                .as_ref()
                                .unwrap()
                                .read(cx)
                                .text()
                                .to_string();
                            if matches!(
                                this.add_friend_state,
                                AddFriendState::Success
                                    | AddFriendState::GistNotFound
                                    | AddFriendState::Error(_)
                            ) {
                                this.add_friend_state = AddFriendState::Idle;
                            }
                            cx.notify();
                        }
                        InputEvent::PressEnter { .. } => {
                            if !this.add_friend_username.trim().is_empty() {
                                crate::handlers::on_send_friend_request(this, cx);
                            }
                        }
                        _ => {}
                    },
                ));
            }
        } else {
            add_friend_input = None;
        }

        let mut screen = Self {
            view: FriendTab::Online,
            friends: Vec::new(),
            loading: false,
            fetching_homes: false,
            show_invite,
            friends_list,
            add_friend_input,
            add_friend_username: String::new(),
            add_friend_state: AddFriendState::Idle,
            avatar_cache: HashMap::new(),
            _subscriptions: subscriptions,
            focus_handle: cx.focus_handle(),
        };

        if friends_engine::is_authenticated() {
            screen.loading = true;
        }

        screen
    }
}

impl Render for FriendsScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let total = self.friends.len();
        let online_count = self
            .friends
            .iter()
            .filter(|f| f.online && f.relation_status == RelationStatus::Mutual)
            .count();
        let pending_count = self
            .friends
            .iter()
            .filter(|f| {
                f.relation_status == RelationStatus::PendingInbound
                    || f.relation_status == RelationStatus::PendingOutbound
            })
            .count();

        let filtered = filtered_friends(self);

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                v_flex()
                    .w_full()
                    .px_8()
                    .pt_8()
                    .pb_6()
                    .gap_6()
                    .child(render_header(self, total, online_count, pending_count, cx))
                    .when(self.show_invite, |this| {
                        this.child(render_add_friend_bar(self, cx))
                    })
                    .child(render_tabs(self, pending_count, cx)),
            )
            .child(div().w_full().h(px(1.)).bg(cx.theme().border))
            .child(
                div()
                    .id("friends-list")
                    .flex_1()
                    .w_full()
                    .scrollable(gpui::Axis::Vertical)
                    .child(
                        v_flex()
                            .w_full()
                            .px_8()
                            .py_6()
                            .gap_3()
                            .children(if self.loading {
                                vec![render_loading_state(cx).into_any_element()]
                            } else if !friends_engine::is_authenticated() {
                                vec![render_not_authenticated(cx).into_any_element()]
                            } else if filtered.is_empty() {
                                vec![render_empty_state(self, cx).into_any_element()]
                            } else {
                                filtered
                                    .into_iter()
                                    .map(|friend| {
                                        render_friend_row(self, friend, cx).into_any_element()
                                    })
                                    .collect()
                            }),
                    ),
            )
    }
}
