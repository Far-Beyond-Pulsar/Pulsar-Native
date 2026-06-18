use crate::gist_storage;
use crate::mutual_detection;
use crate::types::*;
use std::time::Duration;

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

    // Push notification to target's home server(s)
    if let Ok(target_home_servers) = gist_storage::read_engine_friends_file_meta(target_username) {
        let own_home_servers = gist_storage::read_engine_friends_file_meta(&username).unwrap_or_default();
        let own_home_server = own_home_servers.first().cloned();

        for home_server in &target_home_servers {
            tracing::info!("[FriendsService] pushing notification to {}'s home server: {}", target_username, home_server);
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(10))
                .user_agent("Pulsar-Engine")
                .build()
                .map_err(|e| FriendsError::Network(e.to_string()))?;

            let body = serde_json::json!({
                "notification_type": "FriendRequest",
                "from_username": &username,
                "from_home_server": own_home_server.clone(),
                "message": format!("{} sent you a friend request", username),
                "created_at": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            });

            let url = format!("{}/api/v1/notifications", home_server.trim_end_matches('/'));
            match client.post(&url).json(&body).send() {
                Ok(r) if r.status().is_success() => {
                    tracing::info!("[FriendsService] notification pushed to {}", home_server);
                }
                Ok(r) => {
                    tracing::warn!("[FriendsService] notification push to {} returned HTTP {}", home_server, r.status());
                }
                Err(e) => {
                    tracing::warn!("[FriendsService] failed to push notification to {}: {}", home_server, e);
                }
            }
        }
    }

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

pub fn set_home_servers(home_servers: &[String]) -> Result<(), FriendsError> {
    gist_storage::set_home_servers(home_servers)
}

pub fn check_user_has_gist(username: &str) -> Result<bool, FriendsError> {
    gist_storage::check_user_has_gist(username)
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
