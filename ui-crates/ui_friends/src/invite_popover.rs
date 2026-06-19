use crate::types::AddFriendState;
use friends_engine::FriendsError;
use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputEvent, InputState},
    v_flex, ActiveTheme as _, Disableable, Icon, IconName,
};

pub struct InvitePopover {
    add_friend_input: Entity<InputState>,
    add_friend_username: String,
    add_friend_state: AddFriendState,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DismissEvent> for InvitePopover {}

impl InvitePopover {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let add_friend_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("GitHub username..."));

        let subscriptions = vec![cx.subscribe(
            &add_friend_input,
            |this: &mut Self, _input, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    this.add_friend_username = this.add_friend_input.read(cx).text().to_string();
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
                        this.do_send_friend_request(cx);
                    }
                }
                _ => {}
            },
        )];

        Self {
            add_friend_input,
            add_friend_username: String::new(),
            add_friend_state: AddFriendState::Idle,
            _subscriptions: subscriptions,
        }
    }

    pub fn do_send_friend_request(&mut self, cx: &mut Context<Self>) {
        let username = self.add_friend_username.trim().to_string();
        if username.is_empty() {
            return;
        }

        let is_self =
            friends_engine::get_own_username().ok().as_deref() == Some(&username);

        if is_self {
            self.add_friend_state = AddFriendState::Sending;
            cx.notify();
            self.do_complete_invite(&username, cx);
            return;
        }

        self.add_friend_state = AddFriendState::CheckingGist;
        cx.notify();

        let target = username.clone();
        let (tx, rx) = smol::channel::bounded::<Result<bool, FriendsError>>(1);
        std::thread::spawn(move || {
            let result = friends_engine::check_user_has_gist(&target);
            let _ = smol::block_on(tx.send(result));
        });

        let target_for_complete = username.clone();
        cx.spawn(async move |this, cx| {
            let has_gist = rx.recv().await.unwrap_or(Err(FriendsError::Network(
                "Channel closed".to_string(),
            )));
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    match has_gist {
                        Ok(true) => {
                            screen.add_friend_state = AddFriendState::Sending;
                            cx.notify();
                            screen.do_complete_invite(&target_for_complete, cx);
                        }
                        Ok(false) => {
                            screen.add_friend_state = AddFriendState::GistNotFound;
                            cx.notify();
                        }
                        Err(e) => {
                            screen.add_friend_state =
                                AddFriendState::Error(format!("{:?}", e));
                            cx.notify();
                        }
                    }
                });
            });
        })
        .detach();
    }

    fn do_complete_invite(&mut self, username: &str, cx: &mut Context<Self>) {
        let target = username.to_string();
        let is_self =
            friends_engine::get_own_username().ok().as_deref() == Some(&target);

        let (tx, rx) = smol::channel::bounded::<Result<(), FriendsError>>(1);
        let target_for_thread = target.clone();
        std::thread::spawn(move || {
            let result = friends_engine::send_friend_request(&target_for_thread);
            let _ = smol::block_on(tx.send(result));
        });

        cx.spawn(async move |this, cx| {
            let result = rx.recv().await.unwrap_or(Err(FriendsError::Network(
                "Channel closed".to_string(),
            )));
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    match result {
                        Ok(()) => {
                            if is_self {
                                screen.add_friend_state = AddFriendState::SelfFriended;
                            } else {
                                screen.add_friend_state = AddFriendState::Success;
                            }
                            screen.add_friend_username.clear();
                        }
                        Err(FriendsError::NotAuthenticated) => {
                            screen.add_friend_state =
                                AddFriendState::Error("Sign in with GitHub first".to_string());
                        }
                        Err(FriendsError::NotFound) => {
                            screen.add_friend_state =
                                AddFriendState::Error("User not found".to_string());
                        }
                        Err(e) => {
                            screen.add_friend_state =
                                AddFriendState::Error(format!("Error: {:?}", e));
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}

impl Render for InvitePopover {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let is_busy = matches!(
            self.add_friend_state,
            AddFriendState::Sending | AddFriendState::CheckingGist
        );

        v_flex()
            .w(px(320.))
            .p_4()
            .gap_3()
            .rounded_xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_xl()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::Group)
                            .size(px(16.))
                            .text_color(theme.foreground),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child("Invite to Multiplayer"),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .h(px(36.))
                            .rounded_lg()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.background)
                            .overflow_hidden()
                            .child(
                                h_flex()
                                    .w_full()
                                    .h_full()
                                    .px_2()
                                    .gap_1p5()
                                    .items_center()
                                    .child(
                                        Icon::new(IconName::Search)
                                            .size(px(14.))
                                            .text_color(theme.muted_foreground),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .h_full()
                                            .child(ui::input::TextInput::new(
                                                &self.add_friend_input,
                                            )),
                                    ),
                            ),
                    )
                    .child(
                        Button::new("send-invite")
                            .primary()
                            .label(if matches!(self.add_friend_state, AddFriendState::CheckingGist)
                            {
                                "Checking"
                            } else {
                                "Send"
                            })
                            .disabled(is_busy || self.add_friend_username.trim().is_empty())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.do_send_friend_request(cx);
                            })),
                    ),
            )
            .when(
                matches!(self.add_friend_state, AddFriendState::CheckingGist),
                |this| {
                    this.child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Checking if user has Pulsar Engine set up..."),
                            ),
                    )
                },
            )
            .when(
                matches!(self.add_friend_state, AddFriendState::Success),
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
                                    .text_xs()
                                    .text_color(theme.success)
                                    .child("Invite Sent!"),
                            ),
                    )
                },
            )
            .when(
                matches!(self.add_friend_state, AddFriendState::GistNotFound),
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
                                    .text_xs()
                                    .text_color(theme.warning)
                                    .child("This user hasn't set up Pulsar Engine friends yet"),
                            ),
                    )
                },
            )
            .when(
                matches!(self.add_friend_state, AddFriendState::SelfFriended),
                |this| {
                    this.child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.warning)
                                    .child("You can't be friends with yourself... or can you?"),
                            ),
                    )
                },
            )
            .when_some(
                match &self.add_friend_state {
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
                            .child(div().text_xs().text_color(theme.danger).child(msg)),
                    )
                },
            )
    }
}
