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

pub fn is_user_online(username: &str) -> bool {
    let _ = username;
    false
}

pub fn get_broadcast_info(username: &str) -> Option<(String, String)> {
    let _ = username;
    None
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}
