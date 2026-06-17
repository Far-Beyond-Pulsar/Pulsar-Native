use crate::gist_storage;
use crate::types::{FriendInfo, FriendsError, RelationStatus};
use std::collections::HashSet;

pub fn compute_friends_list() -> Result<Vec<FriendInfo>, FriendsError> {
    let username = gist_storage::get_own_username()?;
    tracing::info!("[mutual_detection] compute_friends_list: own username = {}", username);

    let own_friends = gist_storage::get_own_friends()?;
    tracing::info!("[mutual_detection] compute_friends_list: own friends list ({} entries): {:?}", own_friends.len(), own_friends);
    let own_set: HashSet<&str> = own_friends.iter().map(|s| s.as_str()).collect();

    let mut result = Vec::new();

    for friend_username in &own_friends {
        if friend_username == &username {
            tracing::info!("[mutual_detection] compute_friends_list: skipping self entry {}", friend_username);
            continue;
        }

        tracing::info!("[mutual_detection] compute_friends_list: reading {}'s friends list", friend_username);
        let their_friends = match gist_storage::read_user_friends_list(friend_username) {
            Ok(f) => f,
            Err(e) => {
                tracing::info!("[mutual_detection] compute_friends_list: failed to read {}'s list: {:?} -> PendingOutbound", friend_username, e);
                result.push(FriendInfo {
                    username: friend_username.clone(),
                    pfp: format!("https://github.com/{}.png", friend_username),
                    relation_status: RelationStatus::PendingOutbound,
                    current_project: None,
                    last_seen: None,
                });
                continue;
            }
        };

        tracing::info!("[mutual_detection] compute_friends_list: {}'s friends: {:?}", friend_username, their_friends);
        let their_set: HashSet<&str> = their_friends.iter().map(|s| s.as_str()).collect();

        if their_set.contains(username.as_str()) {
            tracing::info!("[mutual_detection] compute_friends_list: {} <-> {} is Mutual", username, friend_username);
            result.push(FriendInfo {
                username: friend_username.clone(),
                pfp: format!("https://github.com/{}.png", friend_username),
                relation_status: RelationStatus::Mutual,
                current_project: None,
                last_seen: None,
            });
        } else {
            tracing::info!("[mutual_detection] compute_friends_list: {} -> {} is PendingOutbound", username, friend_username);
            result.push(FriendInfo {
                username: friend_username.clone(),
                pfp: format!("https://github.com/{}.png", friend_username),
                relation_status: RelationStatus::PendingOutbound,
                current_project: None,
                last_seen: None,
            });
        }
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
        });
    }

    tracing::info!("[mutual_detection] compute_friends_list: returning {} entries", result.len());
    Ok(result)
}

pub fn check_mutual(username_a: &str, username_b: &str) -> Result<bool, FriendsError> {
    let friends_a = gist_storage::read_user_friends_list(username_a)?;
    let friends_b = gist_storage::read_user_friends_list(username_b)?;

    let set_a: HashSet<&str> = friends_a.iter().map(|s| s.as_str()).collect();
    let set_b: HashSet<&str> = friends_b.iter().map(|s| s.as_str()).collect();

    Ok(set_a.contains(username_b) && set_b.contains(username_a))
}
