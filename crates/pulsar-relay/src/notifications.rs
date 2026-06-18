//! Friend notification system for Pulsar Relay Server.
//!
//! Provides:
//! - GitHub token verification (POST /api/v1/auth/github)
//! - Notification storage and delivery (POST/GET /api/v1/notifications)
//! - WebSocket push for online users

use anyhow::{Context, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubAuthRequest {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubAuthResponse {
    pub github_login: String,
    pub github_id: u64,
    pub server_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub notification_type: NotificationType,
    pub from_username: String,
    pub from_home_server: Option<String>,
    pub message: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum NotificationType {
    FriendRequest,
    FriendRequestAccepted,
    FriendRequestDeclined,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushNotificationRequest {
    pub notification_type: NotificationType,
    pub from_username: String,
    pub from_home_server: Option<String>,
    pub target_username: String,
    pub target_home_server: String,
}

/// In-memory notification store, keyed by recipient GitHub username.
pub struct NotificationStore {
    notifications: DashMap<String, Vec<Notification>>,
    /// Cached GitHub profiles: github_username -> (github_id, verified_at)
    verified_users: DashMap<String, VerifiedUser>,
}

#[derive(Debug, Clone)]
struct VerifiedUser {
    github_id: u64,
    verified_at: u64,
}

impl NotificationStore {
    pub fn new() -> Self {
        Self {
            notifications: DashMap::new(),
            verified_users: DashMap::new(),
        }
    }

    /// Verify a GitHub token by calling the GitHub /user API.
    pub async fn verify_github_token(
        &self,
        token: &str,
    ) -> Result<(String, u64)> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("Pulsar-Relay/1.0")
            .build()
            .context("Failed to build HTTP client")?;

        let resp = client
            .get("https://api.github.com/user")
            .header("Accept", "application/json")
            .bearer_auth(token)
            .send()
            .await
            .context("Failed to reach GitHub API")?;

        if !resp.status().is_success() {
            anyhow::bail!("GitHub API returned HTTP {}", resp.status());
        }

        #[derive(Deserialize)]
        struct GitHubUser {
            login: String,
            id: u64,
        }

        let user: GitHubUser = resp
            .json()
            .await
            .context("Failed to parse GitHub response")?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        self.verified_users.insert(
            user.login.clone(),
            VerifiedUser {
                github_id: user.id,
                verified_at: now,
            },
        );

        Ok((user.login, user.id))
    }

    /// Check if a user was recently verified (within 1 hour).
    pub fn is_user_verified(&self, username: &str) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.verified_users
            .get(username)
            .map(|v| now - v.verified_at < 3600)
            .unwrap_or(false)
    }

    /// Store a notification for a recipient.
    pub fn push_notification(&self, notification: Notification) {
        let mut entries = self
            .notifications
            .entry(notification.from_username.clone())
            .or_insert_with(Vec::new);
        entries.push(notification);
    }

    /// Get all pending notifications for a user (and clear them).
    pub fn take_notifications(&self, username: &str) -> Vec<Notification> {
        self.notifications
            .remove(username)
            .map(|(_, v)| v)
            .unwrap_or_default()
    }

    /// Relay a notification from one home server to another.
    pub async fn relay_notification(
        &self,
        req: &PushNotificationRequest,
    ) -> Result<()> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("Pulsar-Relay/1.0")
            .build()
            .context("Failed to build HTTP client")?;

        let body = serde_json::json!({
            "notification_type": req.notification_type,
            "from_username": req.from_username,
            "from_home_server": req.from_home_server,
            "message": format!("{} sent you a friend request", req.from_username),
            "created_at": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
        });

        let target_url = format!(
            "{}/api/v1/notifications",
            req.target_home_server.trim_end_matches('/')
        );

        let resp = client
            .post(&target_url)
            .json(&body)
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => Ok(()),
            Ok(r) => anyhow::bail!(
                "Target server returned HTTP {}",
                r.status()
            ),
            Err(e) => anyhow::bail!("Failed to reach target server: {}", e),
        }
    }
}
