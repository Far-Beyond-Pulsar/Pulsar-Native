use crate::types::FriendsError;
use serde::{Deserialize, Serialize};

const GIST_FILENAME: &str = "engine_friends.json";
const GIST_DESCRIPTION: &str = "Pulsar Engine - Friends List";

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

fn github_token() -> Result<String, FriendsError> {
    let result = pulsar_auth::load_access_token()
        .map_err(|_| FriendsError::NotAuthenticated)?
        .ok_or(FriendsError::NotAuthenticated);
    if result.is_err() {
        tracing::info!("[gist_storage] github_token: no token found (not authenticated)");
    }
    result
}

fn github_username() -> Result<String, FriendsError> {
    if let Some(ec) = engine_state::EngineContext::global() {
        if let Some(profile) = ec.auth_profile() {
            tracing::info!("[gist_storage] github_username: resolved from EngineContext as {}", profile.login);
            return Ok(profile.login.clone());
        }
    }
    if let Some(profile) = pulsar_auth::load_cached_profile() {
        tracing::info!("[gist_storage] github_username: resolved from cached profile as {}", profile.login);
        return Ok(profile.login);
    }
    tracing::info!("[gist_storage] github_username: no cached profile, fetching from API");
    let token = github_token()?;
    fetch_username(&token)
}

fn fetch_username(token: &str) -> Result<String, FriendsError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| FriendsError::Network(e.to_string()))?;

    let resp = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Pulsar-Engine")
        .send()
        .map_err(|e| FriendsError::Network(e.to_string()))?;

    let status = resp.status();
    tracing::info!("[gist_storage] fetch_username: GET /user -> HTTP {}", status);

    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        tracing::info!("[gist_storage] fetch_username: error body: {}", body);
        return Err(FriendsError::Api(format!("HTTP {}: {}", status, body)));
    }

    let user: UserResponse = resp
        .json()
        .map_err(|e| FriendsError::Api(e.to_string()))?;
    tracing::info!("[gist_storage] fetch_username: resolved as {}", user.login);
    Ok(user.login)
}

fn find_pulsar_gist(token: &str, username: &str) -> Result<Option<String>, FriendsError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| FriendsError::Network(e.to_string()))?;

    let url = format!("https://api.github.com/users/{}/gists", username);
    tracing::info!("[gist_storage] find_pulsar_gist: fetching gist list for {}", username);

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Pulsar-Engine")
        .send()
        .map_err(|e| FriendsError::Network(e.to_string()))?;

    let status = resp.status();
    tracing::info!("[gist_storage] find_pulsar_gist: {} -> HTTP {}", url, status);

    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        tracing::info!("[gist_storage] find_pulsar_gist: error body: {}", body);
        return Err(FriendsError::Api(format!("HTTP {}: {}", status, body)));
    }

    let gists: Vec<serde_json::Value> = resp
        .json()
        .map_err(|e| FriendsError::Api(e.to_string()))?;

    tracing::info!("[gist_storage] find_pulsar_gist: {} returned {} gists for {}", url, gists.len(), username);

    for gist in &gists {
        if let Some(files) = gist.get("files").and_then(|f| f.as_object()) {
            let filenames: Vec<&str> = files.keys().map(|k| k.as_str()).collect();
            tracing::info!("[gist_storage] find_pulsar_gist: gist {} has files: {:?}", gist.get("id").and_then(|v| v.as_str()).unwrap_or("?"), filenames);
            if files.contains_key(GIST_FILENAME) {
                let id = gist.get("id").and_then(|id| id.as_str().map(String::from));
                tracing::info!("[gist_storage] find_pulsar_gist: found pulsar gist id={:?} for {}", id, username);
                return Ok(id);
            }
        }
    }
    tracing::info!("[gist_storage] find_pulsar_gist: no pulsar gist found for {}", username);
    Ok(None)
}

pub fn read_engine_friends(username: &str) -> Result<Vec<String>, FriendsError> {
    tracing::info!("[gist_storage] read_engine_friends: reading friends for {}", username);
    let token = github_token()?;
    let gist_id = find_pulsar_gist(&token, username)?;

    let gist_id = match gist_id {
        Some(id) => id,
        None => {
            tracing::info!("[gist_storage] read_engine_friends: no gist found for {}, returning empty", username);
            return Ok(Vec::new());
        }
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| FriendsError::Network(e.to_string()))?;

    let url = format!("https://api.github.com/gists/{}", gist_id);
    tracing::info!("[gist_storage] read_engine_friends: fetching gist content from {}", url);

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Pulsar-Engine")
        .send()
        .map_err(|e| FriendsError::Network(e.to_string()))?;

    let status = resp.status();
    tracing::info!("[gist_storage] read_engine_friends: GET {} -> HTTP {}", url, status);

    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        tracing::info!("[gist_storage] read_engine_friends: error body: {}", body);
        return Err(FriendsError::Api(format!("HTTP {}: {}", status, body)));
    }

    let gist: GistResponse = resp
        .json()
        .map_err(|e| FriendsError::Api(e.to_string()))?;

    tracing::info!("[gist_storage] read_engine_friends: gist files present: {}", gist.files.is_some());

    if let Some(files) = gist.files {
        tracing::info!("[gist_storage] read_engine_friends: file keys in response: {:?}", files.keys().collect::<Vec<_>>());
        if let Some(file) = files.get(GIST_FILENAME) {
            tracing::info!("[gist_storage] read_engine_friends: file found, content present: {}, raw_url: {:?}", file.content.is_some(), file.raw_url);
            if let Some(content) = &file.content {
                tracing::info!("[gist_storage] read_engine_friends: raw content: {}", content);
                let parsed: crate::types::EngineFriendsFile =
                    serde_json::from_str(content).unwrap_or_else(|e| {
                        tracing::info!("[gist_storage] read_engine_friends: parse error: {}", e);
                        crate::types::EngineFriendsFile { friends: Vec::new() }
                    });
                tracing::info!("[gist_storage] read_engine_friends: parsed {} friends for {}: {:?}", parsed.friends.len(), username, parsed.friends);
                return Ok(parsed.friends);
            } else {
                tracing::info!("[gist_storage] read_engine_friends: content field is null (truncated?), raw_url={:?}", file.raw_url);
            }
        } else {
            tracing::info!("[gist_storage] read_engine_friends: {} key not found in gist files", GIST_FILENAME);
        }
    }
    tracing::info!("[gist_storage] read_engine_friends: fell through, returning empty for {}", username);
    Ok(Vec::new())
}

pub fn write_engine_friends(friends: &[String]) -> Result<(), FriendsError> {
    tracing::info!("[gist_storage] write_engine_friends: writing {} friends: {:?}", friends.len(), friends);
    let token = github_token()?;
    let username = github_username()?;
    tracing::info!("[gist_storage] write_engine_friends: own username resolved as {}", username);
    let gist_id = find_pulsar_gist(&token, &username)?;

    let content = serde_json::to_string_pretty(&serde_json::json!({
        "friends": friends,
        "updated_at": chrono::Utc::now().to_rfc3339()
    }))
    .map_err(|e| FriendsError::Api(e.to_string()))?;

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| FriendsError::Network(e.to_string()))?;

    let mut files = std::collections::HashMap::new();
    files.insert(
        GIST_FILENAME.to_string(),
        GistFile {
            content: content.clone(),
        },
    );

    if let Some(id) = gist_id {
        let url = format!("https://api.github.com/gists/{}", id);

        // Read the live gist content before patching so we don't lose entries that
        // weren't returned by the (potentially cached) gist-list endpoint.
        tracing::info!("[gist_storage] write_engine_friends: reading live content from {} before patch", url);
        let live_resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "Pulsar-Engine")
            .send()
            .map_err(|e| FriendsError::Network(e.to_string()))?;
        let live_status = live_resp.status();
        tracing::info!("[gist_storage] write_engine_friends: GET {} -> HTTP {}", url, live_status);

        let mut merged: Vec<String> = friends.to_vec();
        if live_status.is_success() {
            if let Ok(live_gist) = live_resp.json::<GistResponse>() {
                if let Some(live_files) = live_gist.files {
                    if let Some(live_file) = live_files.get(GIST_FILENAME) {
                        if let Some(live_content) = &live_file.content {
                            if let Ok(existing) = serde_json::from_str::<crate::types::EngineFriendsFile>(live_content) {
                                tracing::info!("[gist_storage] write_engine_friends: live gist has {} entries: {:?}", existing.friends.len(), existing.friends);
                                // Union: keep everything in the incoming list plus any existing
                                // entries not already present (preserves entries missed by stale cache read).
                                for entry in existing.friends {
                                    if !merged.contains(&entry) {
                                        tracing::info!("[gist_storage] write_engine_friends: merging in missing entry {}", entry);
                                        merged.push(entry);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        tracing::info!("[gist_storage] write_engine_friends: merged list ({} entries): {:?}", merged.len(), merged);
        let merged_content = serde_json::to_string_pretty(&serde_json::json!({
            "friends": merged,
            "updated_at": chrono::Utc::now().to_rfc3339()
        }))
        .map_err(|e| FriendsError::Api(e.to_string()))?;
        let mut merged_files = std::collections::HashMap::new();
        merged_files.insert(GIST_FILENAME.to_string(), GistFile { content: merged_content });

        tracing::info!("[gist_storage] write_engine_friends: PATCHing {}", url);
        let patch_body = serde_json::json!({ "files": merged_files });
        let resp = client
            .patch(&url)
            .json(&patch_body)
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "Pulsar-Engine")
            .send()
            .map_err(|e| FriendsError::Network(e.to_string()))?;

        let status = resp.status();
        tracing::info!("[gist_storage] write_engine_friends: PATCH {} -> HTTP {}", url, status);
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            tracing::info!("[gist_storage] write_engine_friends: PATCH error body: {}", body);
            return Err(FriendsError::Api(format!("HTTP {}: {}", status, body)));
        }
    } else {
        tracing::info!("[gist_storage] write_engine_friends: no existing gist, POSTing new gist");
        let body = GistRequest {
            description: GIST_DESCRIPTION.to_string(),
            public: true,
            files,
        };
        let resp = client
            .post("https://api.github.com/gists")
            .json(&body)
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "Pulsar-Engine")
            .send()
            .map_err(|e| FriendsError::Network(e.to_string()))?;

        let status = resp.status();
        tracing::info!("[gist_storage] write_engine_friends: POST /gists -> HTTP {}", status);
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            tracing::info!("[gist_storage] write_engine_friends: POST error body: {}", body);
            return Err(FriendsError::Api(format!("HTTP {}: {}", status, body)));
        }
    }

    tracing::info!("[gist_storage] write_engine_friends: write complete for {}", username);
    Ok(())
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

pub fn read_user_friends_list(username: &str) -> Result<Vec<String>, FriendsError> {
    read_engine_friends(username)
}

/// Scans recent public gists for ones that name `username` in their friends list.
/// GitHub has no gist-specific full-text search, so we fetch recent public gists,
/// filter for ones containing our sentinel filename, then read each one's content.
pub fn search_inbound_requests(username: &str) -> Vec<String> {
    tracing::info!("[gist_storage] search_inbound_requests: scanning public gists for inbound requests to {}", username);
    let token = match github_token() {
        Ok(t) => t,
        Err(_) => {
            tracing::info!("[gist_storage] search_inbound_requests: no token, skipping");
            return Vec::new();
        }
    };

    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
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
        Err(e) => {
            tracing::info!("[gist_storage] search_inbound_requests: request error: {}", e);
            return Vec::new();
        }
    };

    let status = resp.status();
    tracing::info!("[gist_storage] search_inbound_requests: GET /gists/public -> HTTP {}", status);
    if !status.is_success() {
        return Vec::new();
    }

    let gists: Vec<PublicGistEntry> = match resp.json() {
        Ok(g) => g,
        Err(e) => {
            tracing::info!("[gist_storage] search_inbound_requests: parse error: {}", e);
            return Vec::new();
        }
    };

    let pulsar_gists: Vec<_> = gists.iter()
        .filter(|g| g.files.as_ref().map(|f| f.contains_key(GIST_FILENAME)).unwrap_or(false))
        .collect();
    tracing::info!("[gist_storage] search_inbound_requests: {} total public gists, {} have {}", gists.len(), pulsar_gists.len(), GIST_FILENAME);

    let mut result = Vec::new();
    for gist in &pulsar_gists {
        let owner_login = match gist.owner.as_ref().map(|o| o.login.as_str()) {
            Some(l) => l.to_string(),
            None => continue,
        };
        if owner_login.eq_ignore_ascii_case(username) {
            continue;
        }
        tracing::info!("[gist_storage] search_inbound_requests: checking {}'s friends list", owner_login);
        match read_engine_friends(&owner_login) {
            Ok(their_friends) => {
                tracing::info!("[gist_storage] search_inbound_requests: {} has friends: {:?}", owner_login, their_friends);
                if their_friends.iter().any(|f| f.eq_ignore_ascii_case(username)) {
                    tracing::info!("[gist_storage] search_inbound_requests: {} has us in their list -> inbound request", owner_login);
                    result.push(owner_login);
                }
            }
            Err(e) => {
                tracing::info!("[gist_storage] search_inbound_requests: failed to read {}'s friends: {:?}", owner_login, e);
            }
        }
    }
    tracing::info!("[gist_storage] search_inbound_requests: found {} inbound requests: {:?}", result.len(), result);
    result
}

pub fn get_own_username() -> Result<String, FriendsError> {
    github_username()
}

pub fn get_own_friends() -> Result<Vec<String>, FriendsError> {
    let username = github_username()?;
    tracing::info!("[gist_storage] get_own_friends: fetching friends for own user {}", username);
    read_engine_friends(&username)
}
