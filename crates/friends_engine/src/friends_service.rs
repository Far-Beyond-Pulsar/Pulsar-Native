use crate::gist_storage;
use crate::mutual_detection;
use crate::types::*;

pub fn get_friends_list() -> Result<Vec<FriendInfo>, FriendsError> {
    mutual_detection::compute_friends_list()
}

pub fn send_friend_request(target_username: &str) -> Result<(), FriendsError> {
    let username = gist_storage::get_own_username()?;
    tracing::info!("[FriendsService] send_friend_request: {} -> {}", username, target_username);
    let mut friends = gist_storage::get_own_friends()?;
    tracing::info!("[FriendsService] send_friend_request: current friends list: {:?}", friends);

    if friends.contains(&target_username.to_string()) {
        tracing::info!("[FriendsService] send_friend_request: {} already in list, no-op", target_username);
        return Ok(());
    }

    friends.push(target_username.to_string());
    tracing::info!("[FriendsService] send_friend_request: writing updated list: {:?}", friends);
    gist_storage::write_engine_friends(&friends)?;
    tracing::info!("[FriendsService] send_friend_request: write succeeded, {} -> {}", username, target_username);
    Ok(())
}

pub fn accept_friend_request(target_username: &str) -> Result<(), FriendsError> {
    let username = gist_storage::get_own_username()?;
    let mut friends = gist_storage::get_own_friends()?;

    if !friends.contains(&target_username.to_string()) {
        friends.push(target_username.to_string());
    }

    gist_storage::write_engine_friends(&friends)?;

    tracing::info!(
        "[FriendsService] {} accepted friend request from {}",
        username,
        target_username
    );
    Ok(())
}

pub fn decline_friend_request(target_username: &str) -> Result<(), FriendsError> {
    let mut friends = gist_storage::get_own_friends()?;

    friends.retain(|f| f != target_username);

    gist_storage::write_engine_friends(&friends)?;

    tracing::info!(
        "[FriendsService] Declined friend request from {}",
        target_username
    );
    Ok(())
}

pub fn remove_friend(target_username: &str) -> Result<(), FriendsError> {
    let mut friends = gist_storage::get_own_friends()?;
    friends.retain(|f| f != target_username);
    gist_storage::write_engine_friends(&friends)
}

pub fn get_own_username() -> Result<String, FriendsError> {
    gist_storage::get_own_username()
}

pub fn is_authenticated() -> bool {
    if let Some(ec) = engine_state::EngineContext::global() {
        if ec.auth_profile().is_some() {
            return true;
        }
    }
    if let Some(_profile) = pulsar_auth::load_cached_profile() {
        return true;
    }
    pulsar_auth::load_access_token().ok().flatten().is_some()
}
