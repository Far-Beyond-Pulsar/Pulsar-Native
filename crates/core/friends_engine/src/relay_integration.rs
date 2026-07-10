use sha2::{Digest, Sha256};

pub fn generate_room_key(username_a: &str, username_b: &str) -> String {
    let mut users = vec![username_a.to_lowercase(), username_b.to_lowercase()];
    users.sort();
    let combined = users.join("_");
    let mut hasher = Sha256::new();
    hasher.update(combined.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..16])
}

/// Check if a user is online by querying their relay server.
/// If the user has a home_server cached in our gist, queries that.
/// Otherwise falls back to our own relay.
pub fn is_user_online(username: &str) -> bool {
    // Try to find the user's home_server from our cached friend entries
    let home_server = crate::gist_storage::get_own_friend_entries()
        .ok()
        .and_then(|entries| {
            entries
                .iter()
                .find(|e| e.username == username)
                .and_then(|e| e.home_server.clone())
        })
        .or_else(|| {
            // Fall back to our own home server
            crate::gist_storage::get_own_username()
                .ok()
                .and_then(|u| crate::gist_storage::read_engine_friends_file_meta(&u).ok())
                .and_then(|hs| hs.into_iter().next())
        });

    let Some(hs) = home_server else {
        tracing::info!(
            "[relay_integration] is_user_online({}): no home server known",
            username
        );
        return false;
    };

    let url = format!(
        "{}/api/v1/users/{}/online",
        hs.trim_end_matches('/'),
        username
    );

    tracing::info!(
        "[relay_integration] is_user_online({}): checking {}",
        username,
        url
    );

    match reqwest::blocking::get(&url) {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(body) = resp.json::<serde_json::Value>() {
                let online = body
                    .get("online")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                tracing::info!(
                    "[relay_integration] is_user_online({}): {}",
                    username,
                    online
                );
                return online;
            }
            tracing::warn!(
                "[relay_integration] is_user_online({}): failed to parse response",
                username
            );
            false
        }
        Ok(resp) => {
            tracing::warn!(
                "[relay_integration] is_user_online({}): HTTP {}",
                username,
                resp.status()
            );
            false
        }
        Err(e) => {
            tracing::warn!(
                "[relay_integration] is_user_online({}): network error: {}",
                username,
                e
            );
            false
        }
    }
}

pub fn get_broadcast_info(_username: &str) -> Option<(String, String)> {
    None
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}
