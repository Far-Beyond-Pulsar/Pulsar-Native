use crate::types::{AddFriendState, FriendEntry, FriendTab};
use friends_engine::{FriendInfo, FriendsError, RelationStatus};
use gpui::{prelude::*, *};
use std::collections::HashMap;
use std::sync::Arc;
use ui::StyledExt as _;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputEvent, InputState},
    v_flex, ActiveTheme as _, Disableable, Icon, IconName,
};

pub struct FriendsScreen {
    pub view: FriendTab,
    pub friends: Vec<FriendEntry>,
    pub loading: bool,
    pub add_friend_input: Entity<InputState>,
    pub add_friend_username: String,
    pub add_friend_state: AddFriendState,
    pub authenticated: bool,
    pub avatar_cache: HashMap<String, Option<Arc<RenderImage>>>,
}

impl FriendsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let add_friend_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("GitHub username"));

        let mut screen = Self {
            view: FriendTab::Online,
            friends: Vec::new(),
            loading: true,
            add_friend_input: add_friend_input.clone(),
            add_friend_username: String::new(),
            add_friend_state: AddFriendState::Idle,
            authenticated: friends_engine::is_authenticated(),
            avatar_cache: HashMap::new(),
        };

        cx.subscribe(
            &add_friend_input,
            |this: &mut Self, _input, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    this.add_friend_username = this.add_friend_input.read(cx).text().to_string();
                    if matches!(
                        this.add_friend_state,
                        AddFriendState::Success | AddFriendState::Error(_)
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
        )
        .detach();

        screen.refresh_friends(cx);
        screen
    }

    pub fn refresh_friends(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();

        let authenticated = self.authenticated;
        cx.spawn(async move |this, cx| {
            if !authenticated {
                cx.update(|cx| {
                    this.update(cx, |screen, cx| {
                        screen.loading = false;
                        cx.notify();
                    });
                });
                return;
            }

            let result = std::thread::spawn(|| friends_engine::get_friends_list()).join();

            cx.update(|cx| {
                this.update(cx, |screen, cx| {
                    screen.loading = false;
                    match result {
                        Ok(Ok(list)) => {
                            screen.friends =
                                list.into_iter().map(Self::friend_info_to_entry).collect();
                        }
                        Ok(Err(e)) => {
                            tracing::error!("[FriendsScreen] Failed to load friends: {:?}", e);
                            screen.friends = Vec::new();
                        }
                        Err(_) => {
                            tracing::error!("[FriendsScreen] Thread panicked loading friends");
                            screen.friends = Vec::new();
                        }
                    }
                    let urls: Vec<String> =
                        screen.friends.iter().map(|f| f.pfp_url.clone()).collect();
                    for url in &urls {
                        screen.ensure_avatar_loaded(url, cx);
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn ensure_avatar_loaded(&mut self, url: &str, cx: &mut Context<Self>) {
        if self.avatar_cache.contains_key(url) {
            return;
        }
        self.avatar_cache.insert(url.to_string(), None);

        let url = url.to_string();
        let url_for_thread = url.clone();
        let (tx, rx) = smol::channel::bounded::<Option<Arc<RenderImage>>>(1);
        std::thread::spawn(move || {
            let image = fetch_avatar_image(&url_for_thread).ok();
            let _ = smol::block_on(tx.send(image));
        });

        cx.spawn(async move |this, cx| {
            if let Ok(maybe_image) = rx.recv().await {
                cx.update(|cx| {
                    this.update(cx, |screen, cx| {
                        screen.avatar_cache.insert(url, maybe_image);
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    fn friend_info_to_entry(info: FriendInfo) -> FriendEntry {
        FriendEntry {
            username: info.username.clone(),
            pfp_url: info.pfp.clone(),
            relation_status: info.relation_status,
            current_project: info.current_project,
            current_project_version: None,
            online: false,
            last_seen: info.last_seen,
        }
    }

    pub fn do_send_friend_request(&mut self, cx: &mut Context<Self>) {
        let username = self.add_friend_username.trim().to_string();
        if username.is_empty() {
            return;
        }

        self.add_friend_state = AddFriendState::Sending;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let target = username.clone();
            let result =
                std::thread::spawn(move || friends_engine::send_friend_request(&target)).join();

            cx.update(|cx| {
                this.update(cx, |screen, cx| {
                    match result {
                        Ok(Ok(())) => {
                            screen.add_friend_state = AddFriendState::Success;
                            screen.add_friend_username.clear();
                            screen.refresh_friends(cx);
                        }
                        Ok(Err(FriendsError::NotAuthenticated)) => {
                            screen.add_friend_state =
                                AddFriendState::Error("Sign in with GitHub first".to_string());
                        }
                        Ok(Err(FriendsError::NotFound)) => {
                            screen.add_friend_state =
                                AddFriendState::Error("User not found".to_string());
                        }
                        Ok(Err(e)) => {
                            screen.add_friend_state =
                                AddFriendState::Error(format!("Error: {:?}", e));
                        }
                        Err(_) => {
                            screen.add_friend_state =
                                AddFriendState::Error("Request failed".to_string());
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub fn do_accept_request(&mut self, username: &str, cx: &mut Context<Self>) {
        let target = username.to_string();
        cx.spawn(async move |this, cx| {
            let _ =
                std::thread::spawn(move || friends_engine::accept_friend_request(&target)).join();
            cx.update(|cx| {
                this.update(cx, |screen, cx| {
                    screen.refresh_friends(cx);
                });
            });
        })
        .detach();
    }

    pub fn do_decline_request(&mut self, username: &str, cx: &mut Context<Self>) {
        let target = username.to_string();
        cx.spawn(async move |this, cx| {
            let _ =
                std::thread::spawn(move || friends_engine::decline_friend_request(&target)).join();
            cx.update(|cx| {
                this.update(cx, |screen, cx| {
                    screen.refresh_friends(cx);
                });
            });
        })
        .detach();
    }

    fn filtered_friends(&self) -> Vec<&FriendEntry> {
        self.friends
            .iter()
            .filter(|f| match self.view {
                FriendTab::Online => f.online && f.relation_status == RelationStatus::Mutual,
                FriendTab::Pending => {
                    f.relation_status == RelationStatus::PendingInbound
                        || f.relation_status == RelationStatus::PendingOutbound
                }
                FriendTab::All => true,
            })
            .collect()
    }

    fn render_avatar(&self, friend: &FriendEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let avatar = self
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
}

fn fetch_avatar_image(url: &str) -> Result<Arc<RenderImage>, anyhow::Error> {
    let resp = reqwest::blocking::get(url)?;
    let bytes = resp.bytes()?;
    let img = image::load_from_memory(&bytes)?.into_rgba8();
    let frame = image::Frame::new(img);
    Ok(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}

impl Render for FriendsScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let foreground = theme.foreground;
        let muted_fg = theme.muted_foreground;
        let border = theme.border;
        let bg = theme.background;

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

        let filtered = self.filtered_friends();

        v_flex()
            .size_full()
            .bg(bg)
            .child(
                v_flex()
                    .w_full()
                    .px_8()
                    .pt_8()
                    .pb_6()
                    .gap_6()
                    .child(self.render_header(total, online_count, pending_count, cx))
                    .child(self.render_add_friend_bar(cx))
                    .child(self.render_tabs(pending_count, cx)),
            )
            .child(div().w_full().h(px(1.)).bg(border))
            .child(
                div()
                    .id("friends-list")
                    .flex_1()
                    .w_full()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .w_full()
                            .px_8()
                            .py_6()
                            .gap_3()
                            .children(if self.loading {
                                vec![self.render_loading_state(cx).into_any_element()]
                            } else if !self.authenticated {
                                vec![self.render_not_authenticated(cx).into_any_element()]
                            } else if filtered.is_empty() {
                                vec![self.render_empty_state(cx).into_any_element()]
                            } else {
                                filtered
                                    .into_iter()
                                    .map(|friend| {
                                        self.render_friend_row(friend, cx).into_any_element()
                                    })
                                    .collect()
                            }),
                    ),
            )
    }
}

impl FriendsScreen {
    fn render_header(
        &self,
        total: usize,
        online_count: usize,
        _pending_count: usize,
        cx: &mut Context<Self>,
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
                                    .text_color(theme.accent)
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
                Button::new("open-fab-marketplace")
                    .ghost()
                    .icon(Icon::new(IconName::ExternalLink).size(px(13.)))
                    .label("Discover")
                    .tooltip("Find friends on the FAB Marketplace"),
            )
    }

    fn render_add_friend_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let is_sending = matches!(self.add_friend_state, AddFriendState::Sending);

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
                                    .child(
                                        div().flex_1().h_full().child(ui::input::TextInput::new(
                                            &self.add_friend_input,
                                        )),
                                    ),
                            ),
                    )
                    .child(
                        Button::new("send-friend-request")
                            .primary()
                            .label("Add Friend")
                            .disabled(is_sending || self.add_friend_username.trim().is_empty())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.do_send_friend_request(cx);
                            })),
                    ),
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
                                    .text_sm()
                                    .text_color(theme.success)
                                    .child("Request Sent!"),
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
                            .child(div().text_sm().text_color(theme.danger).child(msg)),
                    )
                },
            )
    }

    fn render_tabs(&self, pending_count: usize, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_1()
            .child(self.render_tab_button("Online", FriendTab::Online, None, cx))
            .child(self.render_tab_button("Pending", FriendTab::Pending, Some(pending_count), cx))
            .child(self.render_tab_button("All", FriendTab::All, None, cx))
    }

    fn render_tab_button(
        &self,
        label: &'static str,
        tab: FriendTab,
        badge: Option<usize>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let is_active = self.view == tab;
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

    fn render_friend_row(&self, friend: &FriendEntry, cx: &mut Context<Self>) -> impl IntoElement {
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
                            .child(self.render_avatar(friend, cx))
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
                        RelationStatus::PendingInbound => self
                            .render_pending_inbound_actions(friend, cx)
                            .into_any_element(),
                        RelationStatus::PendingOutbound => self
                            .render_pending_outbound_state(friend, cx)
                            .into_any_element(),
                        RelationStatus::Mutual => {
                            self.render_mutual_actions(friend, cx).into_any_element()
                        }
                    }),
            )
            .child(div().w_full().h(px(1.)).bg(border_col.opacity(0.4)).mx_4())
    }

    fn render_pending_inbound_actions(
        &self,
        friend: &FriendEntry,
        cx: &mut Context<Self>,
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
                            this.do_accept_request(&uname, cx);
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
                            this.do_decline_request(&uname, cx);
                        })
                    }),
            )
    }

    fn render_pending_outbound_state(
        &self,
        _friend: &FriendEntry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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

    fn render_mutual_actions(
        &self,
        _friend: &FriendEntry,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex().gap_1().child(
            Button::new("friend-actions")
                .ghost()
                .icon(Icon::new(IconName::ArrowRight).size(px(15.)))
                .tooltip("Inspect project"),
        )
    }

    fn render_loading_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .w_full()
            .items_center()
            .justify_center()
            .py_16()
            .gap_4()
            .child(
                div()
                    .text_base()
                    .text_color(theme.muted_foreground)
                    .child("Loading friends..."),
            )
    }

    fn render_not_authenticated(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

    fn render_empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (icon, title, subtitle) = match self.view {
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
                "Add friends by searching for their GitHub username above.",
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
}
