use crate::components::popover::FriendsPopover;
use crate::screen::FriendsScreen;
use crate::utils::avatar::fetch_avatar_image;
use crate::utils::types::{AddFriendState, FriendEntry, FriendTab};
use friends_engine::{FriendInfo, FriendsError, RelationStatus};
use gpui::{prelude::*, *};
use std::sync::Arc;

pub fn on_refresh_friends(this: &mut FriendsScreen, cx: &mut Context<FriendsScreen>) {
    this.loading = true;
    cx.notify();

    if !friends_engine::is_authenticated() {
        this.loading = false;
        this.friends.clear();
        this.friends_list
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
                            .map(|info| friend_info_to_entry(info, own.as_deref()))
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
                    ensure_avatar_loaded(screen, url, cx);
                }
                cx.notify();
            });
        });
    })
    .detach();
}

pub fn on_send_friend_request(this: &mut FriendsScreen, cx: &mut Context<FriendsScreen>) {
    let username = this.add_friend_username.trim().to_string();
    if username.is_empty() {
        return;
    }

    let is_self = friends_engine::get_own_username().ok().as_deref() == Some(&username);

    if is_self {
        this.add_friend_state = AddFriendState::Sending;
        cx.notify();
        on_add_friend(this, &username, cx);
        return;
    }

    this.add_friend_state = AddFriendState::CheckingGist;
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
                    on_add_friend(screen, &target_for_complete, cx);
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

pub fn on_accept_request(
    this: &mut FriendsScreen,
    username: &str,
    cx: &mut Context<FriendsScreen>,
) {
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
                on_refresh_friends(screen, cx);
            });
        });
    })
    .detach();
}

pub fn on_decline_request(
    this: &mut FriendsScreen,
    username: &str,
    cx: &mut Context<FriendsScreen>,
) {
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
                on_refresh_friends(screen, cx);
            });
        });
    })
    .detach();
}

pub fn on_fetch_friend_homes(this: &mut FriendsScreen, cx: &mut Context<FriendsScreen>) {
    this.fetching_homes = true;
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
                        on_refresh_friends(screen, cx);
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

pub fn on_add_friend(this: &mut FriendsScreen, username: &str, cx: &mut Context<FriendsScreen>) {
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
                            on_refresh_friends(screen, cx);
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

pub fn on_invite_to_session(
    this: &mut FriendsPopover,
    username: &str,
    cx: &mut Context<FriendsPopover>,
) {
    let clicked = username.to_string();
    std::thread::spawn(move || {
        tracing::info!("[FriendsPopover] checking online status for {}", clicked);
        let online = friends_engine::relay_integration::is_user_online(&clicked);
        tracing::info!(
            "[FriendsPopover] {} is {}",
            clicked,
            if online { "ONLINE" } else { "OFFLINE" }
        );
        match friends_engine::friends_service::send_session_invite(&clicked) {
            Ok(()) => {
                tracing::info!("[FriendsPopover] session invite sent to {}", clicked)
            }
            Err(e) => tracing::error!(
                "[FriendsPopover] failed to send invite to {}: {:?}",
                clicked,
                e
            ),
        }
    });
    cx.emit(DismissEvent);
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

fn ensure_avatar_loaded(this: &mut FriendsScreen, url: &str, cx: &mut Context<FriendsScreen>) {
    if this.avatar_cache.contains_key(url) {
        return;
    }
    this.avatar_cache.insert(url.to_string(), None);

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
