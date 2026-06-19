use crate::gist_storage;
use crate::mutual_detection;
use crate::notification_listener;
use crate::types::*;
use std::time::Duration;

pub fn get_friends_list() -> Result<Vec<FriendInfo>, FriendsError> {
    mutual_detection::compute_friends_list()
}

pub fn send_friend_request(target_username: &str) -> Result<(), FriendsError> {
    let username = gist_storage::get_own_username()?;
    tracing::info!("[FriendsService] send_friend_request: {} -> {}", username, target_username);
    let mut entries = gist_storage::get_own_friend_entries()?;
    tracing::info!("[FriendsService] send_friend_request: current entries: {:?}", entries);

    if entries.iter().any(|e| e.username == target_username) {
        tracing::info!("[FriendsService] send_friend_request: {} already in list, no-op", target_username);
        return Ok(());
    }

    entries.push(GistFriendEntry {
        username: target_username.to_string(),
        mutual: false,
        home_server: None,
    });
    tracing::info!("[FriendsService] send_friend_request: writing updated list");
    gist_storage::write_engine_friends(&entries)?;
    tracing::info!("[FriendsService] send_friend_request: write succeeded, {} -> {}", username, target_username);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let own_home_server = gist_storage::read_engine_friends_file_meta(&username)
        .ok()
        .and_then(|hs| hs.into_iter().next());

    // Push notification — if target is self, notify all our own sessions;
    // otherwise notify the target's sessions only.
    if target_username == username {
        // Self-friending: notify all our sessions
        if let Some(ref hs) = own_home_server {
            let body = serde_json::json!({
                "id": format!("{}-{}-{}", username, target_username, now),
                "notification_type": "FriendRequest",
                "from_username": &username,
                "to_username": &username,
                "from_home_server": own_home_server.clone(),
                "message": format!("You sent a friend request to yourself"),
                "created_at": now,
            });
            let url = format!("{}/api/v1/notifications", hs.trim_end_matches('/'));
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(5))
                .user_agent("Pulsar-Engine")
                .build()
                .map_err(|e| FriendsError::Network(e.to_string()))?;
            let _ = client.post(&url).json(&body).send();
        }
    } else if let Ok(target_home_servers) = gist_storage::read_engine_friends_file_meta(target_username) {
        for home_server in &target_home_servers {
            tracing::info!("[FriendsService] pushing notification to {}'s home server: {}", target_username, home_server);
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(10))
                .user_agent("Pulsar-Engine")
                .build()
                .map_err(|e| FriendsError::Network(e.to_string()))?;

            let body = serde_json::json!({
                "id": format!("{}-{}-{}", username, target_username, now),
                "notification_type": "FriendRequest",
                "from_username": &username,
                "to_username": target_username,
                "from_home_server": own_home_server.clone(),
                "message": format!("{} sent you a friend request", username),
                "created_at": now,
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
    let mut entries = gist_storage::get_own_friend_entries()?;

    if !entries.iter().any(|e| e.username == target_username) {
        entries.push(GistFriendEntry {
            username: target_username.to_string(),
            mutual: false,
            home_server: None,
        });
    }

    gist_storage::write_engine_friends(&entries)?;

    tracing::info!(
        "[FriendsService] {} accepted friend request from {}",
        username,
        target_username
    );
    Ok(())
}

pub fn decline_friend_request(target_username: &str) -> Result<(), FriendsError> {
    let mut entries = gist_storage::get_own_friend_entries()?;

    entries.retain(|e| e.username != target_username);

    gist_storage::write_engine_friends(&entries)?;

    tracing::info!(
        "[FriendsService] Declined friend request from {}",
        target_username
    );
    Ok(())
}

pub fn remove_friend(target_username: &str) -> Result<(), FriendsError> {
    let mut entries = gist_storage::get_own_friend_entries()?;
    entries.retain(|e| e.username != target_username);
    gist_storage::write_engine_friends(&entries)
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

pub fn fetch_friend_homes() -> Result<usize, FriendsError> {
    mutual_detection::fetch_friend_homes()
}

/// Start the background WebSocket notification listener.
/// Connects to the user's home server and receives notifications in real-time.
pub fn start_notification_listener() {
    notification_listener::start();
}

/// Stop the background WebSocket notification listener.
pub fn stop_notification_listener() {
    notification_listener::stop();
}

/// Take all pending notifications received via WebSocket.
pub fn take_notifications() -> Vec<serde_json::Value> {
    notification_listener::take_notifications()
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