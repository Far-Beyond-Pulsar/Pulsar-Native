//! GitHub device-flow authentication and local credential/profile caching.
//!
//! Owns: the GitHub OAuth device flow (`start_device_flow`, `poll_device_flow`,
//! `wait_for_device_flow_token`), fetching the signed-in user's profile
//! (`fetch_profile`), and persisting the access token (OS keyring with a
//! plain-file fallback) and the cached `AuthProfile` in the Pulsar data dir.
//!
//! Entry points: `restore_session_from_storage` at startup, the device-flow
//! functions to sign in, `sign_out` to clear all stored state, and the
//! `github_client_*_from_env` helpers for configuration.
//!
//! Must not depend on: any UI crate, `engine_state`, or the friends/relay
//! crates. Higher layers (`friends_engine`, `ui_*`) depend on this crate, never
//! the reverse. Pure response parsing lives in private helpers so it can be
//! tested without network access.

use anyhow::{Context, Result};
use directories::ProjectDirs;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const KEYRING_SERVICE: &str = "pulsar_engine";
const KEYRING_ACCOUNT: &str = "github_access_token";
const ENV_GITHUB_CLIENT_ID: &str = "PULSAR_GITHUB_CLIENT_ID";
const ENV_GITHUB_CLIENT_SECRET: &str = "PULSAR_GITHUB_CLIENT_SECRET";
const GITHUB_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const GITHUB_ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER_URL: &str = "https://api.github.com/user";
const PROFILE_CACHE_FILE: &str = "auth_profile.json";
const TOKEN_CACHE_FILE: &str = "auth_token.txt";
static ENV_LOADED: std::sync::Once = std::sync::Once::new();

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthProfile {
    pub github_user_id: u64,
    pub login: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub expires_in: u64,
    #[serde(default)]
    pub interval: u64,
}

#[derive(Debug, Clone)]
pub enum DevicePollState {
    Pending,
    SlowDown,
    Authorized(String),
}

#[derive(Debug, Deserialize)]
struct AccessTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubUserResponse {
    id: u64,
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
}

pub fn github_client_id_from_env() -> Option<String> {
    load_dotenv_once();
    std::env::var(ENV_GITHUB_CLIENT_ID)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn github_client_secret_from_env() -> Option<String> {
    load_dotenv_once();
    std::env::var(ENV_GITHUB_CLIENT_SECRET)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn start_device_flow(client_id: &str) -> Result<DeviceCodeResponse> {
    let client = github_http_client()?;
    let response = client
        .post(GITHUB_DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .form(&[("client_id", client_id), ("scope", "read:user,gist")])
        .send()
        .context("Failed to request GitHub device code")?;

    if !response.status().is_success() {
        anyhow::bail!(
            "GitHub device-code request failed: HTTP {}",
            response.status()
        );
    }

    response
        .json::<DeviceCodeResponse>()
        .context("Failed to parse GitHub device-code response")
}

pub fn poll_device_flow(client_id: &str, device_code: &str) -> Result<DevicePollState> {
    let client = github_http_client()?;
    let response = client
        .post(GITHUB_ACCESS_TOKEN_URL)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", client_id),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .context("Failed to poll GitHub device flow")?;

    if !response.status().is_success() {
        anyhow::bail!("GitHub token polling failed: HTTP {}", response.status());
    }

    let parsed = response
        .json::<AccessTokenResponse>()
        .context("Failed to parse GitHub token polling response")?;

    interpret_poll_response(parsed)
}

fn interpret_poll_response(parsed: AccessTokenResponse) -> Result<DevicePollState> {
    if let Some(token) = parsed.access_token {
        return Ok(DevicePollState::Authorized(token));
    }

    match parsed.error.as_deref() {
        Some("authorization_pending") => Ok(DevicePollState::Pending),
        Some("slow_down") => Ok(DevicePollState::SlowDown),
        Some("expired_token") => anyhow::bail!("GitHub device code expired"),
        Some("access_denied") => anyhow::bail!("GitHub device flow denied"),
        Some(other) => anyhow::bail!("GitHub auth error: {other}"),
        None => anyhow::bail!("Missing access token in GitHub response"),
    }
}

pub fn wait_for_device_flow_token(client_id: &str, flow: &DeviceCodeResponse) -> Result<String> {
    let interval_secs = flow.interval.max(5);
    let timeout = Duration::from_secs(flow.expires_in.max(interval_secs));
    let started = Instant::now();
    let mut current_interval = interval_secs;

    while started.elapsed() < timeout {
        std::thread::sleep(Duration::from_secs(current_interval));
        match poll_device_flow(client_id, &flow.device_code)? {
            DevicePollState::Authorized(token) => return Ok(token),
            DevicePollState::Pending => {}
            DevicePollState::SlowDown => {
                current_interval = slowed_interval(current_interval);
            }
        }
    }

    anyhow::bail!("GitHub device flow timed out")
}

fn slowed_interval(current_secs: u64) -> u64 {
    (current_secs + 5).min(30)
}

pub fn fetch_profile(access_token: &str) -> Result<AuthProfile> {
    let client = github_http_client()?;
    let response = client
        .get(GITHUB_USER_URL)
        .header("Accept", "application/json")
        .bearer_auth(access_token)
        .send()
        .context("Failed to fetch GitHub user profile")?;

    if !response.status().is_success() {
        anyhow::bail!("GitHub /user failed: HTTP {}", response.status());
    }

    let user = response
        .json::<GitHubUserResponse>()
        .context("Failed to parse GitHub profile payload")?;

    Ok(profile_from_user(user))
}

fn profile_from_user(user: GitHubUserResponse) -> AuthProfile {
    AuthProfile {
        github_user_id: user.id,
        login: user.login,
        display_name: user.name,
        avatar_url: user.avatar_url,
    }
}

pub fn store_access_token(access_token: &str) -> Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .context("Failed to initialize keyring entry")?;
    entry
        .set_password(access_token)
        .context("Failed to store GitHub token in keyring")?;
    let _ = save_token_to_cache(access_token);
    Ok(())
}

pub fn load_access_token() -> Result<Option<String>> {
    match (|| -> Result<Option<String>> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
            .context("Failed to initialize keyring entry")?;
        match entry.get_password() {
            Ok(token) => return Ok(Some(token)),
            Err(keyring::Error::NoEntry) => {}
            Err(e) => anyhow::bail!("Failed to read token from keyring: {e}"),
        }
        Ok(None)
    })() {
        Ok(Some(token)) => Ok(Some(token)),
        Ok(None) => load_token_from_cache().or(Ok(None)),
        Err(_) => load_token_from_cache().or(Ok(None)),
    }
}

pub fn clear_access_token() -> Result<()> {
    let _ = clear_token_cache();
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .context("Failed to initialize keyring entry")?;
    match entry.delete_credential() {
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow::anyhow!("Failed to delete keyring token: {e}")),
    }
}

pub fn save_cached_profile(profile: &AuthProfile) -> Result<()> {
    let path = profile_cache_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create auth cache directory: {}",
                parent.display()
            )
        })?;
    }
    let json = serde_json::to_string_pretty(profile).context("Failed to serialize auth profile")?;
    std::fs::write(&path, json)
        .with_context(|| format!("Failed to write auth profile cache: {}", path.display()))
}

pub fn load_cached_profile() -> Option<AuthProfile> {
    let path = profile_cache_path().ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<AuthProfile>(&text).ok()
}

pub fn clear_cached_profile() -> Result<()> {
    let path = profile_cache_path()?;
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("Failed to remove profile cache: {}", path.display()))?;
    }
    Ok(())
}

pub fn sign_out() -> Result<()> {
    clear_access_token()?;
    clear_cached_profile()?;
    Ok(())
}

pub fn restore_session_from_storage() -> Option<AuthProfile> {
    let token = load_access_token().ok().flatten()?;
    match fetch_profile(&token) {
        Ok(profile) => {
            let _ = save_cached_profile(&profile);
            Some(profile)
        }
        Err(_) => load_cached_profile(),
    }
}

fn github_http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("Pulsar-Native/1.0")
        .build()
        .context("Failed to create GitHub HTTP client")
}

fn profile_cache_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .context("Could not resolve Pulsar project directories")?;
    Ok(dirs.data_dir().join(PROFILE_CACHE_FILE))
}

fn token_cache_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .context("Could not resolve Pulsar project directories")?;
    Ok(dirs.data_dir().join(TOKEN_CACHE_FILE))
}

fn save_token_to_cache(token: &str) -> Result<()> {
    let path = token_cache_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, token)?;
    Ok(())
}

fn load_token_from_cache() -> Result<Option<String>> {
    let path = token_cache_path()?;
    if path.exists() {
        Ok(Some(std::fs::read_to_string(path)?))
    } else {
        Ok(None)
    }
}

fn clear_token_cache() -> Result<()> {
    let path = token_cache_path()?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

fn load_dotenv_once() {
    ENV_LOADED.call_once(|| {
        let _ = dotenvy::dotenv();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(token: Option<&str>, error: Option<&str>) -> AccessTokenResponse {
        AccessTokenResponse {
            access_token: token.map(str::to_string),
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn poll_token_wins_over_error() {
        let state = interpret_poll_response(response(Some("tok"), Some("slow_down"))).unwrap();
        assert!(matches!(state, DevicePollState::Authorized(t) if t == "tok"));
    }

    #[test]
    fn poll_pending_and_slow_down_are_not_errors() {
        let pending = interpret_poll_response(response(None, Some("authorization_pending")));
        assert!(matches!(pending.unwrap(), DevicePollState::Pending));
        let slow = interpret_poll_response(response(None, Some("slow_down")));
        assert!(matches!(slow.unwrap(), DevicePollState::SlowDown));
    }

    #[test]
    fn poll_terminal_errors_carry_distinct_messages() {
        let msg = |e: Option<&str>| {
            interpret_poll_response(response(None, e))
                .unwrap_err()
                .to_string()
        };
        assert_eq!(msg(Some("expired_token")), "GitHub device code expired");
        assert_eq!(msg(Some("access_denied")), "GitHub device flow denied");
        assert_eq!(msg(Some("weird")), "GitHub auth error: weird");
        assert_eq!(msg(None), "Missing access token in GitHub response");
    }

    #[test]
    fn slow_down_backoff_adds_five_seconds_and_caps_at_thirty() {
        assert_eq!(slowed_interval(5), 10);
        assert_eq!(slowed_interval(26), 30);
        assert_eq!(slowed_interval(30), 30);
        assert_eq!(slowed_interval(100), 30);
    }

    #[test]
    fn device_code_response_defaults_missing_timing_fields() {
        let parsed: DeviceCodeResponse = serde_json::from_str(
            r#"{"device_code":"dc","user_code":"ABCD-1234","verification_uri":"https://example.test/device"}"#,
        )
        .unwrap();
        assert_eq!(parsed.device_code, "dc");
        assert_eq!(parsed.user_code, "ABCD-1234");
        assert_eq!(parsed.expires_in, 0);
        assert_eq!(parsed.interval, 0);
    }

    #[test]
    fn access_token_response_parses_error_payload() {
        let parsed: AccessTokenResponse =
            serde_json::from_str(r#"{"error":"authorization_pending","error_description":"x"}"#)
                .unwrap();
        assert_eq!(parsed.access_token, None);
        assert_eq!(parsed.error.as_deref(), Some("authorization_pending"));
    }

    #[test]
    fn github_user_payload_maps_to_profile_ignoring_extra_fields() {
        let user: GitHubUserResponse = serde_json::from_str(
            r#"{"id":42,"login":"octo","name":null,"avatar_url":"https://a.test/x.png","bio":"ignored"}"#,
        )
        .unwrap();
        assert_eq!(
            profile_from_user(user),
            AuthProfile {
                github_user_id: 42,
                login: "octo".into(),
                display_name: None,
                avatar_url: Some("https://a.test/x.png".into()),
            }
        );
    }

    #[test]
    fn auth_profile_json_round_trips() {
        let profile = AuthProfile {
            github_user_id: 7,
            login: "someone".into(),
            display_name: Some("Some One".into()),
            avatar_url: None,
        };
        let json = serde_json::to_string_pretty(&profile).unwrap();
        assert_eq!(serde_json::from_str::<AuthProfile>(&json).unwrap(), profile);
    }

    #[test]
    fn cache_paths_are_distinct_files_in_one_directory() {
        let profile = profile_cache_path().unwrap();
        let token = token_cache_path().unwrap();
        assert_eq!(profile.parent(), token.parent());
        assert_ne!(profile.file_name(), token.file_name());
    }
}
