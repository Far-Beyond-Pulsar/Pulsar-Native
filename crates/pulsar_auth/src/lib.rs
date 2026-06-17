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
                current_interval = (current_interval + 5).min(30);
            }
        }
    }

    anyhow::bail!("GitHub device flow timed out")
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

    Ok(AuthProfile {
        github_user_id: user.id,
        login: user.login,
        display_name: user.name,
        avatar_url: user.avatar_url,
    })
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
