//! Friend notification system for Pulsar Relay Server.
//!
//! Provides:
//! - GitHub token verification (POST /api/v1/auth/github)
//! - Notification storage and delivery (POST/GET /api/v1/notifications)
//! - WebSocket push for online users

use anyhow::{Context, Result};
use axum::extract::ws::{Message, WebSocket};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

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
    pub to_username: String,
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
    verified_users: DashMap<String, VerifiedUser>,
    /// WebSocket connections per recipient username.
    connected_users: DashMap<String, Vec<mpsc::UnboundedSender<Notification>>>,
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
            connected_users: DashMap::new(),
        }
    }

    pub async fn verify_github_token(&self, token: &str) -> Result<(String, u64)> {
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

    /// Register a WebSocket connection for a user.
    pub fn register_connection(&self, username: &str, tx: mpsc::UnboundedSender<Notification>) {
        self.connected_users
            .entry(username.to_string())
            .or_insert_with(Vec::new)
            .push(tx);
    }

    /// Unregister a WebSocket connection for a user.
    pub fn unregister_connection(&self, username: &str, tx: &mpsc::UnboundedSender<Notification>) {
        let is_empty = {
            if let Some(mut senders) = self.connected_users.get_mut(username) {
                senders.retain(|t| !t.same_channel(tx));
                senders.is_empty()
            } else {
                false
            }
        };
        if is_empty {
            self.connected_users.remove(username);
        }
    }

    /// Store a notification and push to any open WebSocket for the recipient.
    pub fn push_notification(&self, notification: Notification) {
        // Store in-memory for HTTP poll fallback
        self.notifications
            .entry(notification.to_username.clone())
            .or_insert_with(Vec::new)
            .push(notification.clone());

        // Push to open WebSocket connections for recipient
        if let Some(entries) = self.connected_users.get(&notification.to_username) {
            for tx in entries.value().iter() {
                let _ = tx.send(notification.clone());
            }
        }
    }

    pub fn take_notifications(&self, username: &str) -> Vec<Notification> {
        self.notifications
            .remove(username)
            .map(|(_, v)| v)
            .unwrap_or_default()
    }

    pub async fn relay_notification(&self, req: &PushNotificationRequest) -> Result<()> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("Pulsar-Relay/1.0")
            .build()
            .context("Failed to build HTTP client")?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let body = serde_json::json!({
            "id": format!("relay-{}-{}-{}", req.from_username, req.target_username, now),
            "notification_type": req.notification_type,
            "from_username": req.from_username,
            "to_username": req.target_username,
            "from_home_server": req.from_home_server,
            "message": format!("{} sent you a friend request", req.from_username),
            "created_at": now,
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
            Ok(r) => anyhow::bail!("Target server returned HTTP {}", r.status()),
            Err(e) => anyhow::bail!("Failed to reach target server: {}", e),
        }
    }

    /// Handle an incoming WebSocket connection for notifications.
    /// Expects a GitHub token as the first text message for authentication.
    pub async fn handle_websocket(self: Arc<Self>, mut ws: WebSocket) {
        // First message must be the GitHub token
        let token = match ws.recv().await {
            Some(Ok(Message::Text(t))) => t,
            _ => return,
        };

        let username = match self.verify_github_token(&token).await {
            Ok((uname, _)) => uname,
            Err(e) => {
                tracing::warn!("[NotificationWS] auth failed: {}", e);
                let _ = ws.send(Message::Text(
                    serde_json::json!({"error": "auth_failed"}).to_string().into(),
                )).await;
                return;
            }
        };

        tracing::info!("[NotificationWS] {} connected", username);

        let (tx, mut rx) = mpsc::unbounded_channel::<Notification>();
        self.register_connection(&username, tx.clone());

        // Send any pending in-memory notifications immediately
        let pending = self.take_notifications(&username);
        for note in &pending {
            if let Ok(json) = serde_json::to_string(note) {
                let _ = ws.send(Message::Text(json.into())).await;
            }
        }

        // Forward new notifications from the channel to the WebSocket
        let (mut ws_sender, mut ws_receiver) = ws.split();

        let forward_task = tokio::spawn(async move {
            while let Some(note) = rx.recv().await {
                if let Ok(json) = serde_json::to_string(&note) {
                    if ws_sender.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
            }
        });

        // Drain any inbound messages until connection closes
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
                _ => {}
            }
        }

        forward_task.abort();
        self.unregister_connection(&username, &tx);
        tracing::info!("[NotificationWS] {} disconnected", username);
    }
}
