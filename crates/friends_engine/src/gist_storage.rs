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
    pulsar_auth::load_access_token()
        .map_err(|_| FriendsError::NotAuthenticated)?
        .ok_or(FriendsError::NotAuthenticated)
}

fn github_username() -> Result<String, FriendsError> {
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

    if !resp.status().is_success() {
        return Err(FriendsError::Api(format!("HTTP {}", resp.status())));
    }

    let user: UserResponse = resp
        .json()
        .map_err(|e| FriendsError::Api(e.to_string()))?;
    Ok(user.login)
}

fn find_pulsar_gist(token: &str, username: &str) -> Result<Option<String>, FriendsError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| FriendsError::Network(e.to_string()))?;

    let url = format!("https://api.github.com/users/{}/gists", username);
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Pulsar-Engine")
        .send()
        .map_err(|e| FriendsError::Network(e.to_string()))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(FriendsError::Api(format!("HTTP {}: {}", status, body)));
    }

    let gists: Vec<serde_json::Value> = resp
        .json()
        .map_err(|e| FriendsError::Api(e.to_string()))?;

    for gist in &gists {
        if let Some(files) = gist.get("files").and_then(|f| f.as_object()) {
            if files.contains_key(GIST_FILENAME) {
                return Ok(gist.get("id").and_then(|id| id.as_str().map(String::from)));
            }
        }
    }
    Ok(None)
}

pub fn read_engine_friends(username: &str) -> Result<Vec<String>, FriendsError> {
    let token = github_token()?;
    let gist_id = find_pulsar_gist(&token, username)?;

    let gist_id = match gist_id {
        Some(id) => id,
        None => return Ok(Vec::new()),
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| FriendsError::Network(e.to_string()))?;

    let url = format!("https://api.github.com/gists/{}", gist_id);
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Pulsar-Engine")
        .send()
        .map_err(|e| FriendsError::Network(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(FriendsError::Api(format!("HTTP {}", resp.status())));
    }

    let gist: GistResponse = resp
        .json()
        .map_err(|e| FriendsError::Api(e.to_string()))?;

    if let Some(files) = gist.files {
        if let Some(file) = files.get(GIST_FILENAME) {
            if let Some(content) = &file.content {
                let parsed: crate::types::EngineFriendsFile =
                    serde_json::from_str(content).unwrap_or(crate::types::EngineFriendsFile {
                        friends: Vec::new(),
                    });
                return Ok(parsed.friends);
            }
        }
    }
    Ok(Vec::new())
}

pub fn write_engine_friends(friends: &[String]) -> Result<(), FriendsError> {
    let token = github_token()?;
    let username = github_username()?;
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
        let body = serde_json::json!({ "files": files });
        let resp = client
            .patch(&url)
            .json(&body)
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "Pulsar-Engine")
            .send()
            .map_err(|e| FriendsError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(FriendsError::Api(format!("HTTP {}", resp.status())));
        }
    } else {
        let body = GistRequest {
            description: GIST_DESCRIPTION.to_string(),
            public: false,
            files,
        };
        let resp = client
            .post("https://api.github.com/gists")
            .json(&body)
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "Pulsar-Engine")
            .send()
            .map_err(|e| FriendsError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(FriendsError::Api(format!("HTTP {}", resp.status())));
        }
    }

    Ok(())
}

pub fn read_user_friends_list(username: &str) -> Result<Vec<String>, FriendsError> {
    read_engine_friends(username)
}

pub fn get_own_username() -> Result<String, FriendsError> {
    github_username()
}

pub fn get_own_friends() -> Result<Vec<String>, FriendsError> {
    let username = github_username()?;
    read_engine_friends(&username)
}
