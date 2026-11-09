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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_key_is_order_and_case_insensitive() {
        let a = generate_room_key("Alice", "bob");
        assert_eq!(a, generate_room_key("BOB", "alice"));
        assert_ne!(a, generate_room_key("alice", "carol"));
    }

    #[test]
    fn room_key_is_32_lowercase_hex_chars() {
        let key = generate_room_key("a", "b");
        assert_eq!(key.len(), 32);
        assert!(key.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    }

    #[test]
    fn room_key_matches_known_sha256_prefix() {
        // First 16 bytes of sha256("a_b"), the sorted lowercase join.
        assert_eq!(
            generate_room_key("b", "A"),
            "648fa9b31bc7ff7eb914e7a7180f07e0"
        );
    }

    #[test]
    fn hex_encode_pads_each_byte() {
        assert_eq!(hex::encode(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
        assert_eq!(hex::encode(&[]), "");
    }

    #[test]
    fn broadcast_info_is_currently_unavailable() {
        assert_eq!(get_broadcast_info("anyone"), None);
    }
}
