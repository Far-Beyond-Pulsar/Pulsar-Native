use crate::types::FriendsError;
use serde::{Deserialize, Serialize};
use std::sync::RwLock;

const GIST_FILENAME: &str = "engine_friends.json";
const GIST_DESCRIPTION: &str = "Pulsar Engine - Friends List";

static CACHED_OWN_GIST: RwLock<Option<(String, String)>> = RwLock::new(None);

// ── Internal gist format ────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Default)]
struct GistContent {
    #[serde(default)]
    friends: Vec<serde_json::Value>,
    #[serde(default)]
    home_servers: Vec<String>,
    #[serde(default)]
    updated_at: String,
}

impl GistContent {
    fn from_str(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }

    fn to_string_pretty(&self) -> Result<String, FriendsError> {
        serde_json::to_string_pretty(self).map_err(|e| FriendsError::Api(e.to_string()))
    }
}

// ── HTTP helpers ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct GistFile {
    content: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct GistRequest {
    description: String,
    public: bool,
    files: std::collections::HashMap<String, GistFile>,
}

#[derive(Debug, Deserialize)]
struct GistResponse {
    id: Option<String>,
    html_url: Option<String>,
    files: Option<std::collections::HashMap<String, GistFileResponse>>,
}

#[derive(Debug, Deserialize)]
struct GistFileResponse {
    filename: Option<String>,
    content: Option<String>,
    raw_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UserResponse {
    login: String,
}

#[derive(Deserialize)]
struct GistOwner {
    login: String,
}

#[derive(Deserialize)]
struct PublicGistEntry {
    owner: Option<GistOwner>,
    files: Option<std::collections::HashMap<String, serde_json::Value>>,
}

// ── Auth helpers ────────────────────────────────────────────────────────────

fn github_token() -> Result<String, FriendsError> {
    pulsar_auth::load_access_token()
        .map_err(|_| FriendsError::NotAuthenticated)?
        .ok_or(FriendsError::NotAuthenticated)
}

pub fn github_username() -> Result<String, FriendsError> {
    if let Some(ec) = engine_state::EngineContext::global() {
        if let Some(profile) = ec.auth_profile() {
            return Ok(profile.login.clone());
        }
    }
    if let Some(profile) = pulsar_auth::load_cached_profile() {
        return Ok(profile.login);
    }
    let token = github_token()?;
    fetch_username(&token)
}

fn fetch_username(token: &str) -> Result<String, FriendsError> {
    let client = blocking_client()?;
    let resp = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Pulsar-Engine")
        .send()
        .map_err(|e| FriendsError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(FriendsError::Api(format!("HTTP {}", resp.status())));
    }
    let user: UserResponse = resp.json().map_err(|e| FriendsError::Api(e.to_string()))?;
    Ok(user.login)
}

fn blocking_client() -> Result<reqwest::blocking::Client, FriendsError> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| FriendsError::Network(e.to_string()))
}

// ── Gist cache ──────────────────────────────────────────────────────────────

fn cached_own_gist_id(username: &str) -> Option<String> {
    let lock = CACHED_OWN_GIST.read().ok()?;
    lock.as_ref().and_then(|(u, id)| {
        if u == username {
            Some(id.clone())
        } else {
            None
        }
    })
}

fn set_cached_own_gist_id(username: &str, id: &str) {
    if let Ok(mut lock) = CACHED_OWN_GIST.write() {
        *lock = Some((username.to_string(), id.to_string()));
    }
}

fn find_pulsar_gist(token: &str, username: &str) -> Result<Option<String>, FriendsError> {
    if let Ok(own) = github_username() {
        if own == username {
            if let Some(id) = cached_own_gist_id(username) {
                return Ok(Some(id));
            }
        }
    }

    let client = blocking_client()?;
    let resp = client
        .get(format!("https://api.github.com/users/{}/gists", username))
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Pulsar-Engine")
        .send()
        .map_err(|e| FriendsError::Network(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(FriendsError::Api(format!("HTTP {}", resp.status())));
    }

    let gists: Vec<serde_json::Value> =
        resp.json().map_err(|e| FriendsError::Api(e.to_string()))?;

    for gist in &gists {
        if let Some(files) = gist.get("files").and_then(|f| f.as_object()) {
            if files.contains_key(GIST_FILENAME) {
                let id = gist.get("id").and_then(|id| id.as_str().map(String::from));
                if let Some(ref id_str) = id {
                    if let Ok(own) = github_username() {
                        if own == username {
                            set_cached_own_gist_id(username, id_str);
                        }
                    }
                }
                return Ok(id);
            }
        }
    }
    Ok(None)
}

// ── Read gist content ───────────────────────────────────────────────────────

fn fetch_gist_content(token: &str, gist_id: &str) -> Result<GistContent, FriendsError> {
    let client = blocking_client()?;
    let resp = client
        .get(format!("https://api.github.com/gists/{}", gist_id))
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Pulsar-Engine")
        .send()
        .map_err(|e| FriendsError::Network(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(FriendsError::Api(format!("HTTP {}", resp.status())));
    }

    let gist: GistResponse = resp.json().map_err(|e| FriendsError::Api(e.to_string()))?;

    let content = gist
        .files
        .and_then(|mut f| f.remove(GIST_FILENAME))
        .and_then(|f| f.content)
        .unwrap_or_default();

    Ok(GistContent::from_str(&content))
}

// ── Write gist content ──────────────────────────────────────────────────────

fn write_gist_content(
    token: &str,
    username: &str,
    gist_id: Option<String>,
    content: &GistContent,
) -> Result<(), FriendsError> {
    let mut data = content.to_string_pretty()?;
    // stamp updated_at
    let mut val: serde_json::Value = serde_json::from_str(&data).unwrap_or_default();
    val["updated_at"] = serde_json::Value::String(chrono::Utc::now().to_rfc3339());
    data = serde_json::to_string_pretty(&val).map_err(|e| FriendsError::Api(e.to_string()))?;

    let client = blocking_client()?;
    let mut files = std::collections::HashMap::new();
    files.insert(GIST_FILENAME.to_string(), GistFile { content: data });

    if let Some(id) = gist_id {
        let resp = client
            .patch(format!("https://api.github.com/gists/{}", id))
            .json(&serde_json::json!({ "files": files }))
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "Pulsar-Engine")
            .send()
            .map_err(|e| FriendsError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(FriendsError::Api(format!("HTTP {}", resp.status())));
        }
    } else {
        let resp = client
            .post("https://api.github.com/gists")
            .json(&GistRequest {
                description: GIST_DESCRIPTION.to_string(),
                public: true,
                files,
            })
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "Pulsar-Engine")
            .send()
            .map_err(|e| FriendsError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(FriendsError::Api(format!("HTTP {}", resp.status())));
        }
        if let Ok(created) = resp.json::<GistResponse>() {
            if let Some(ref new_id) = created.id {
                set_cached_own_gist_id(username, new_id);
            }
        }
    }
    Ok(())
}

// ── URL normalization ───────────────────────────────────────────────────────

/// Normalize a relay URL: add https:// if no scheme, reject localhost.
/// Returns None if the URL should not be stored in the gist.
pub fn normalize_relay_url(raw: &str) -> Option<String> {
    let s = raw.trim().trim_end_matches('/');
    if s.is_empty() {
        return None;
    }
    if s.starts_with("localhost") || s.starts_with("127.0.0.1") || s.starts_with("0.0.0.0") {
        return None;
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        Some(s.to_string())
    } else {
        Some(format!("https://{}", s))
    }
}

// ── Public API ──────────────────────────────────────────────────────────────

fn parse_friend_entries(values: &[serde_json::Value]) -> Vec<crate::types::GistFriendEntry> {
    values
        .iter()
        .filter_map(|v| {
            let username = v
                .get("username")
                .and_then(|u| u.as_str())
                .map(String::from)?;
            let mutual = v.get("mutual").and_then(|m| m.as_bool()).unwrap_or(false);
            let home_server = v
                .get("home_server")
                .and_then(|h| h.as_str())
                .map(String::from);
            Some(crate::types::GistFriendEntry {
                username,
                mutual,
                home_server,
            })
        })
        .collect()
}

/// Read home servers from the gist. Never writes. Safe to call on startup.
pub(crate) fn read_engine_friends_file_meta(username: &str) -> Result<Vec<String>, FriendsError> {
    let token = github_token()?;
    let gist_id = match find_pulsar_gist(&token, username)? {
        Some(id) => id,
        None => return Ok(Vec::new()),
    };
    let content = fetch_gist_content(&token, &gist_id)?;
    Ok(content.home_servers)
}

pub fn read_engine_friends(username: &str) -> Result<Vec<String>, FriendsError> {
    let entries = read_engine_friend_entries(username)?;
    Ok(entries.into_iter().map(|e| e.username).collect())
}

pub fn read_engine_friend_entries(
    username: &str,
) -> Result<Vec<crate::types::GistFriendEntry>, FriendsError> {
    tracing::info!(
        "[gist_storage] read_engine_friend_entries: reading for {}",
        username
    );
    let token = github_token()?;
    let gist_id = match find_pulsar_gist(&token, username)? {
        Some(id) => id,
        None => return Ok(Vec::new()),
    };
    let content = fetch_gist_content(&token, &gist_id)?;
    let entries = parse_friend_entries(&content.friends);
    tracing::info!(
        "[gist_storage] read_engine_friend_entries: found {} entries",
        entries.len()
    );
    Ok(entries)
}

/// Write friends list, preserving home_servers already in the gist.
pub fn write_engine_friends(friends: &[crate::types::GistFriendEntry]) -> Result<(), FriendsError> {
    tracing::info!(
        "[gist_storage] write_engine_friends: writing {} entries",
        friends.len()
    );
    let token = github_token()?;
    let username = github_username()?;
    let gist_id = find_pulsar_gist(&token, &username)?;

    // Read existing content to preserve home_servers
    let mut content = match &gist_id {
        Some(id) => fetch_gist_content(&token, id).unwrap_or_default(),
        None => GistContent::default(),
    };

    content.friends = friends
        .iter()
        .map(|e| {
            serde_json::json!({
                "username": e.username,
                "mutual": e.mutual,
                "home_server": e.home_server
            })
        })
        .collect();

    write_gist_content(&token, &username, gist_id, &content)?;
    tracing::info!("[gist_storage] write_engine_friends: done");
    Ok(())
}

/// Add a relay home server to the gist. Only writes if the URL is new and public.
/// Never touches the friends list. Never called on startup — only on explicit user action.
pub fn set_home_servers(home_servers: &[String]) -> Result<(), FriendsError> {
    let token = github_token()?;
    let username = github_username()?;
    let gist_id = find_pulsar_gist(&token, &username)?;

    let normalized: Vec<String> = home_servers
        .iter()
        .filter_map(|hs| normalize_relay_url(hs))
        .collect();

    if normalized.is_empty() {
        tracing::info!("[gist_storage] set_home_servers: no public URLs to write, skipping");
        return Ok(());
    }

    // Read existing content to preserve friends
    let mut content = match &gist_id {
        Some(id) => fetch_gist_content(&token, id).unwrap_or_default(),
        None => GistContent::default(),
    };

    // Replace home_servers with the new list (user explicitly configured these)
    content.home_servers = normalized;

    write_gist_content(&token, &username, gist_id, &content)?;
    tracing::info!("[gist_storage] set_home_servers: done");
    Ok(())
}

pub fn check_user_has_gist(username: &str) -> Result<bool, FriendsError> {
    let token = github_token()?;
    Ok(find_pulsar_gist(&token, username)?.is_some())
}

pub fn read_user_friends_list(username: &str) -> Result<Vec<String>, FriendsError> {
    read_engine_friends(username)
}

pub fn search_inbound_requests(username: &str) -> Vec<String> {
    let token = match github_token() {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let client = match blocking_client() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let resp = match client
        .get("https://api.github.com/gists/public?per_page=100")
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Pulsar-Engine")
        .send()
    {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    if !resp.status().is_success() {
        return Vec::new();
    }
    let gists: Vec<PublicGistEntry> = match resp.json() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };

    let mut result = Vec::new();
    for gist in gists.iter().filter(|g| {
        g.files
            .as_ref()
            .map(|f| f.contains_key(GIST_FILENAME))
            .unwrap_or(false)
    }) {
        let owner_login = match gist.owner.as_ref().map(|o| o.login.as_str()) {
            Some(l) => l.to_string(),
            None => continue,
        };
        if owner_login.eq_ignore_ascii_case(username) {
            continue;
        }
        if let Ok(their_friends) = read_engine_friends(&owner_login) {
            if their_friends
                .iter()
                .any(|f| f.eq_ignore_ascii_case(username))
            {
                result.push(owner_login);
            }
        }
    }
    result
}

pub fn get_own_username() -> Result<String, FriendsError> {
    github_username()
}

pub fn get_own_friends() -> Result<Vec<String>, FriendsError> {
    let username = github_username()?;
    read_engine_friends(&username)
}

pub fn get_own_friend_entries() -> Result<Vec<crate::types::GistFriendEntry>, FriendsError> {
    let username = github_username()?;
    read_engine_friend_entries(&username)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_adds_https_and_trims_whitespace_and_trailing_slashes() {
        assert_eq!(
            normalize_relay_url("  relay.example.com// "),
            Some("https://relay.example.com".to_string())
        );
    }

    #[test]
    fn normalize_keeps_existing_scheme() {
        assert_eq!(
            normalize_relay_url("http://relay.example.com/"),
            Some("http://relay.example.com".to_string())
        );
        assert_eq!(
            normalize_relay_url("https://relay.example.com:8443"),
            Some("https://relay.example.com:8443".to_string())
        );
    }

    #[test]
    fn normalize_rejects_empty_and_local_addresses() {
        for raw in ["", "   ", "/", "localhost:3000", "127.0.0.1", "0.0.0.0:80"] {
            assert_eq!(normalize_relay_url(raw), None, "input {raw:?}");
        }
    }

    #[test]
    fn parse_friend_entries_reads_fields_and_defaults() {
        let values = vec![
            json!({"username": "alice", "mutual": true, "home_server": "https://a.test"}),
            json!({"username": "bob"}),
        ];
        let entries = parse_friend_entries(&values);
        assert_eq!(
            entries,
            vec![
                crate::types::GistFriendEntry {
                    username: "alice".into(),
                    mutual: true,
                    home_server: Some("https://a.test".into()),
                },
                crate::types::GistFriendEntry {
                    username: "bob".into(),
                    mutual: false,
                    home_server: None,
                },
            ]
        );
    }

    #[test]
    fn parse_friend_entries_skips_entries_without_string_username() {
        let values = vec![
            json!({"mutual": true}),
            json!({"username": 5}),
            json!("bare-string"),
            json!({"username": "carol", "home_server": null}),
        ];
        let entries = parse_friend_entries(&values);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].username, "carol");
        assert_eq!(entries[0].home_server, None);
    }

    #[test]
    fn gist_content_tolerates_garbage_and_missing_fields() {
        let broken = GistContent::from_str("not json");
        assert!(broken.friends.is_empty() && broken.home_servers.is_empty());

        let partial = GistContent::from_str(r#"{"home_servers":["https://r.test"]}"#);
        assert!(partial.friends.is_empty());
        assert_eq!(partial.home_servers, vec!["https://r.test".to_string()]);
        assert_eq!(partial.updated_at, "");
    }

    #[test]
    fn gist_content_round_trips_through_pretty_json() {
        let content = GistContent {
            friends: vec![json!({"username": "dave", "mutual": false, "home_server": null})],
            home_servers: vec!["https://r.test".into()],
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        let text = content.to_string_pretty().unwrap();
        let back = GistContent::from_str(&text);
        assert_eq!(back.friends, content.friends);
        assert_eq!(back.home_servers, content.home_servers);
        assert_eq!(back.updated_at, content.updated_at);
    }
}
