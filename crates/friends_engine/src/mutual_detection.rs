use crate::gist_storage;
use crate::types::{FriendInfo, FriendsError, GistFriendEntry, RelationStatus};
use std::collections::HashSet;

pub fn compute_friends_list() -> Result<Vec<FriendInfo>, FriendsError> {
    let username = gist_storage::get_own_username()?;
    tracing::info!("[mutual_detection] compute_friends_list: own username = {}", username);

    let own_entries = gist_storage::get_own_friend_entries()?;
    tracing::info!("[mutual_detection] compute_friends_list: own entries ({} items): {:?}", own_entries.len(), own_entries);
    let own_set: HashSet<&str> = own_entries.iter().map(|e| e.username.as_str()).collect();

    let mut result = Vec::new();
    let mut updated_entries: Vec<GistFriendEntry> = Vec::new();

    for entry in &own_entries {
        if entry.username == username {
            tracing::info!("[mutual_detection] compute_friends_list: self entry {} found, adding self-friend entry", entry.username);
            result.push(FriendInfo {
                username: username.clone(),
                pfp: format!("https://github.com/{}.png", username),
                relation_status: RelationStatus::Mutual,
                current_project: None,
                last_seen: None,
                home_server: None,
            });
            updated_entries.push(entry.clone());
            continue;
        }

        if entry.mutual {
            tracing::info!("[mutual_detection] compute_friends_list: {} already cached as mutual, skipping re-check", entry.username);
            result.push(FriendInfo {
                username: entry.username.clone(),
                pfp: format!("https://github.com/{}.png", entry.username),
                relation_status: RelationStatus::Mutual,
                current_project: None,
                last_seen: None,
                home_server: entry.home_server.clone(),
            });
            updated_entries.push(entry.clone());
            continue;
        }

        tracing::info!("[mutual_detection] compute_friends_list: reading {}'s friends list", entry.username);
        // Read the friend's home server from their gist's top-level home_servers array
        let friend_home_server = gist_storage::read_engine_friends_file_meta(&entry.username)
            .ok()
            .and_then(|hs| hs.into_iter().next());

        match gist_storage::read_engine_friend_entries(&entry.username) {
            Ok(their_entries) => {
                let their_usernames: Vec<String> = their_entries.iter().map(|e| e.username.clone()).collect();
                let their_set: HashSet<&str> = their_usernames.iter().map(|s| s.as_str()).collect();
                let is_mutual = their_set.contains(username.as_str());
                let home_server = friend_home_server.clone();

                tracing::info!("[mutual_detection] compute_friends_list: {} <-> {} is_mutual={}, home_server={:?}", username, entry.username, is_mutual, home_server);

                if is_mutual {
                    result.push(FriendInfo {
                        username: entry.username.clone(),
                        pfp: format!("https://github.com/{}.png", entry.username),
                        relation_status: RelationStatus::Mutual,
                        current_project: None,
                        last_seen: None,
                        home_server: home_server.clone(),
                    });
                    updated_entries.push(GistFriendEntry {
                        username: entry.username.clone(),
                        mutual: true,
                        home_server,
                    });
                } else {
                    result.push(FriendInfo {
                        username: entry.username.clone(),
                        pfp: format!("https://github.com/{}.png", entry.username),
                        relation_status: RelationStatus::PendingOutbound,
                        current_project: None,
                        last_seen: None,
                        home_server: home_server.clone(),
                    });
                    updated_entries.push(GistFriendEntry {
                        username: entry.username.clone(),
                        mutual: false,
                        home_server,
                    });
                }
            }
            Err(e) => {
                tracing::info!("[mutual_detection] compute_friends_list: failed to read {}'s list: {:?} -> PendingOutbound", entry.username, e);
                result.push(FriendInfo {
                    username: entry.username.clone(),
                    pfp: format!("https://github.com/{}.png", entry.username),
                    relation_status: RelationStatus::PendingOutbound,
                    current_project: None,
                    last_seen: None,
                    home_server: friend_home_server.clone(),
                });
                // Still update home_server if we read it from meta
                if friend_home_server.is_some() && friend_home_server != entry.home_server {
                    updated_entries.push(GistFriendEntry {
                        username: entry.username.clone(),
                        mutual: entry.mutual,
                        home_server: friend_home_server.clone(),
                    });
                } else {
                    updated_entries.push(entry.clone());
                }
            }
        }
    }

    // Write back updated mutual/home_server cache
    if updated_entries != own_entries {
        tracing::info!("[mutual_detection] compute_friends_list: writing updated mutual cache to gist");
        let _ = gist_storage::write_engine_friends(&updated_entries);
    }

    let inbound = gist_storage::search_inbound_requests(&username);
    tracing::info!("[mutual_detection] compute_friends_list: {} inbound requests found: {:?}", inbound.len(), inbound);
    for inbound_username in &inbound {
        if own_set.contains(inbound_username.as_str()) || inbound_username == &username {
            tracing::info!("[mutual_detection] compute_friends_list: skipping inbound {} (already in own list or self)", inbound_username);
            continue;
        }
        result.push(FriendInfo {
            username: inbound_username.clone(),
            pfp: format!("https://github.com/{}.png", inbound_username),
            relation_status: RelationStatus::PendingInbound,
            current_project: None,
            last_seen: None,
            home_server: None,
        });
    }

    tracing::info!("[mutual_detection] compute_friends_list: returning {} entries", result.len());
    Ok(result)
}

pub fn check_mutual(username_a: &str, username_b: &str) -> Result<bool, FriendsError> {
    let friends_a = gist_storage::read_engine_friends(username_a)?;
    let friends_b = gist_storage::read_engine_friends(username_b)?;

    let set_a: HashSet<&str> = friends_a.iter().map(|s| s.as_str()).collect();
    let set_b: HashSet<&str> = friends_b.iter().map(|s| s.as_str()).collect();

    Ok(set_a.contains(username_b) && set_b.contains(username_a))
}

/// Fetch home servers for all non-mutual friends by re-reading their gists.
/// Returns the number of friends whose home_server was updated.
pub fn fetch_friend_homes() -> Result<usize, FriendsError> {
    let mut entries = gist_storage::get_own_friend_entries()?;
    let mut updated = 0;

    for entry in &mut entries {
        // Only fetch for non-mutual friends; mutual ones already have their home_server
        if entry.username == gist_storage::get_own_username().ok().as_deref().unwrap_or("") {
            continue;
        }
        if entry.home_server.is_some() {
            continue;
        }
        if let Ok(hs) = gist_storage::read_engine_friends_file_meta(&entry.username) {
            if let Some(server) = hs.into_iter().next() {
                tracing::info!("[mutual_detection] fetch_friend_homes: {} home_server = {}", entry.username, server);
                entry.home_server = Some(server);
                updated += 1;
            }
        }
    }

    if updated > 0 {
        gist_storage::write_engine_friends(&entries)?;
    }

    Ok(updated)
}