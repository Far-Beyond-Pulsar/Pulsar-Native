use crate::types::{AddFriendState, FriendEntry, FriendTab};
use friends_engine::{FriendInfo, FriendsError, RelationStatus};
use gpui::{prelude::*, *};
use std::collections::HashMap;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    dropdown::{SearchableList, SearchableListEvent},
    h_flex,
    input::{InputEvent, InputState},
    popover::Popover,
    skeleton::Skeleton,
    v_flex, ActiveTheme as _, Disableable, Icon, IconName,
};

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
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DismissEvent> for FriendsScreen {}

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
                                this.do_send_friend_request(cx);
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
        };

        if friends_engine::is_authenticated() {
            screen.loading = true;
        }

        screen
    }

    pub fn do_fetch_friend_homes(&mut self, cx: &mut Context<Self>) {
        self.fetching_homes = true;
        cx.notify();

        let (tx, rx) = smol::channel::bounded::<Result<usize, friends_engine::FriendsError>>(1);

        std::thread::spawn(move || {
            let result = friends_engine::fetch_friend_homes();
            let _ = smol::block_on(tx.send(result));
        });

        cx.spawn(async move |this, cx| {
            let result = rx
                .recv()
                .await
                .unwrap_or(Err(friends_engine::FriendsError::Network(
                    "Channel closed".to_string(),
                )));
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    screen.fetching_homes = false;
                    match result {
                        Ok(count) => {
                            tracing::info!(
                                "[FriendsScreen] Fetched home servers for {} friends",
                                count
                            );
                            screen.refresh_friends(cx);
                        }
                        Err(e) => {
                            tracing::error!(
                                "[FriendsScreen] Failed to fetch friend homes: {:?}",
                                e
                            );
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub fn refresh_friends(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();

        if !friends_engine::is_authenticated() {
            self.loading = false;
            self.friends.clear();
            self.friends_list
                .update(cx, |list, cx| list.set_items(Vec::new(), cx));
            cx.notify();
            return;
        }

        let (tx, rx) = smol::channel::bounded::<Result<Vec<FriendInfo>, FriendsError>>(1);

        std::thread::spawn(move || {
            let result = friends_engine::get_friends_list();
            let _ = smol::block_on(tx.send(result));
        });

        cx.spawn(async move |this, cx| {
            let result = rx
                .recv()
                .await
                .unwrap_or(Err(FriendsError::Network("Channel closed".to_string())));
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    screen.loading = false;
                    match result {
                        Ok(list) => {
                            let own = friends_engine::get_own_username().ok();
                            screen.friends = list
                                .into_iter()
                                .map(|info| Self::friend_info_to_entry(info, own.as_deref()))
                                .collect();
                            let friends_usernames: Vec<String> = screen
                                .friends
                                .iter()
                                .filter(|f| f.relation_status == RelationStatus::Mutual)
                                .map(|f| f.username.clone())
                                .collect();
                            screen
                                .friends_list
                                .update(cx, |list, cx| list.set_items(friends_usernames, cx));
                        }
                        Err(e) => {
                            tracing::error!("[FriendsScreen] Failed to load friends: {:?}", e);
                            screen.friends = Vec::new();
                            screen
                                .friends_list
                                .update(cx, |list, cx| list.set_items(Vec::new(), cx));
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
                    let _ = this.update(cx, |screen, cx| {
                        screen.avatar_cache.insert(url, maybe_image);
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    fn friend_info_to_entry(info: FriendInfo, own_username: Option<&str>) -> FriendEntry {
        let is_self = own_username == Some(&info.username);
        FriendEntry {
            username: info.username.clone(),
            pfp_url: info.pfp.clone(),
            relation_status: info.relation_status,
            current_project: info.current_project,
            current_project_version: None,
            online: false,
            last_seen: info.last_seen,
            is_self,
        }
    }

    pub fn do_send_friend_request(&mut self, cx: &mut Context<Self>) {
        let username = self.add_friend_username.trim().to_string();
        if username.is_empty() {
            return;
        }

        let is_self = friends_engine::get_own_username().ok().as_deref() == Some(&username);

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
            let has_gist = rx
                .recv()
                .await
                .unwrap_or(Err(FriendsError::Network("Channel closed".to_string())));
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| match has_gist {
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
                        screen.add_friend_state = AddFriendState::Error(format!("{:?}", e));
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    fn do_complete_invite(&mut self, username: &str, cx: &mut Context<Self>) {
        let target = username.to_string();
        let is_self = friends_engine::get_own_username().ok().as_deref() == Some(&target);

        let (tx, rx) = smol::channel::bounded::<Result<(), FriendsError>>(1);
        let target_for_thread = target.clone();
        std::thread::spawn(move || {
            let result = friends_engine::send_friend_request(&target_for_thread);
            let _ = smol::block_on(tx.send(result));
        });

        let target_for_self = target.clone();
        cx.spawn(async move |this, cx| {
            let result = rx
                .recv()
                .await
                .unwrap_or(Err(FriendsError::Network("Channel closed".to_string())));
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    match result {
                        Ok(()) => {
                            if is_self {
                                screen.add_friend_state = AddFriendState::SelfFriended;
                                let self_entry = FriendEntry {
                                    username: "yourself".to_string(),
                                    pfp_url: format!("https://github.com/{}.png", &target_for_self),
                                    relation_status: RelationStatus::Mutual,
                                    current_project: Some("self-love".to_string()),
                                    current_project_version: None,
                                    online: true,
                                    last_seen: None,
                                    is_self: true,
                                };
                                if !screen.friends.iter().any(|f| f.is_self) {
                                    screen.friends.insert(0, self_entry);
                                }
                            } else {
                                screen.add_friend_state = AddFriendState::Success;
                            }
                            screen.add_friend_username.clear();
                            if !is_self {
                                screen.refresh_friends(cx);
                            }
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

    pub fn do_accept_request(&mut self, username: &str, cx: &mut Context<Self>) {
        let target = username.to_string();
        let (tx, rx) = smol::channel::bounded::<Result<(), FriendsError>>(1);
        std::thread::spawn(move || {
            let result = friends_engine::accept_friend_request(&target);
            let _ = smol::block_on(tx.send(result));
        });
        cx.spawn(async move |this, cx| {
            let _ = rx.recv().await;
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    screen.refresh_friends(cx);
                });
            });
        })
        .detach();
    }

    pub fn do_decline_request(&mut self, username: &str, cx: &mut Context<Self>) {
        let target = username.to_string();
        let (tx, rx) = smol::channel::bounded::<Result<(), FriendsError>>(1);
        std::thread::spawn(move || {
            let result = friends_engine::decline_friend_request(&target);
            let _ = smol::block_on(tx.send(result));
        });
        cx.spawn(async move |this, cx| {
            let _ = rx.recv().await;
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
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
                    .when(self.show_invite, |this| {
                        this.child(self.render_add_friend_bar(cx))
                    })
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
                            } else if !friends_engine::is_authenticated() {
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
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("refresh-friends")
                            .ghost()
                            .icon(Icon::new(IconName::Refresh).size(px(13.)))
                            .tooltip("Refresh friends list")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh_friends(cx);
                            })),
                    )
                    .child(
                        Button::new("fetch-friend-homes")
                            .ghost()
                            .icon(Icon::new(IconName::Globe).size(px(13.)))
                            .label(if self.fetching_homes {
                                "Fetching..."
                            } else {
                                "Fetch homes"
                            })
                            .disabled(self.fetching_homes)
                            .tooltip("Fetch home server URLs for non-mutual friends")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.do_fetch_friend_homes(cx);
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
                                let list = self.friends_list.clone();
                                move |_window, _cx| list.clone()
                            }),
                    ),
            )
    }

    fn render_add_friend_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let is_busy = matches!(
            self.add_friend_state,
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
                                            self.add_friend_input.as_ref().unwrap(),
                                        ),
                                    )),
                            ),
                    )
                    .child(
                        Button::new("send-friend-request")
                            .primary()
                            .label(
                                if matches!(self.add_friend_state, AddFriendState::CheckingGist) {
                                    "Checking"
                                } else {
                                    "Add Friend"
                                },
                            )
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
                                    .text_sm()
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

    fn render_self_row(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();

        let self_entry = self.friends.iter().find(|f| f.is_self);
        let avatar =
            self_entry.and_then(|f| self.avatar_cache.get(&f.pfp_url).and_then(|o| o.clone()));

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

    fn render_friend_row(&self, friend: &FriendEntry, cx: &mut Context<Self>) -> AnyElement {
        if friend.is_self {
            return self.render_self_row(cx);
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
            .into_any_element()
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
}
