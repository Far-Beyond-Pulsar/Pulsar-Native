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

    /// Check if a user has an active WebSocket connection.
    pub fn is_user_online(&self, username: &str) -> bool {
        let online = self.connected_users.get(username)
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        tracing::info!("[NotificationStore] is_user_online: {} -> {}", username, online);
        online
    }

    /// Register a WebSocket connection for a user.
    pub fn register_connection(&self, username: &str, tx: mpsc::UnboundedSender<Notification>) {
        let count = {
            let mut entries = self.connected_users
                .entry(username.to_string())
                .or_insert_with(Vec::new);
            entries.push(tx);
            entries.len()
        };
        tracing::info!("[NotificationStore] register_connection: {} ({} active sockets)", username, count);
    }

    /// Unregister a WebSocket connection for a user.
    pub fn unregister_connection(&self, username: &str, tx: &mpsc::UnboundedSender<Notification>) {
        let (removed, remaining) = {
            if let Some(mut senders) = self.connected_users.get_mut(username) {
                let before = senders.len();
                senders.retain(|t| !t.same_channel(tx));
                (before - senders.len(), senders.len())
            } else {
                (0, 0)
            }
        };
        // Clean up empty entries
        if remaining == 0 {
            self.connected_users.remove(username);
        }
        tracing::info!("[NotificationStore] unregister_connection: {} (removed {} socket, {} remaining)", username, removed, remaining);
    }

    /// Store a notification and push to any open WebSocket for the recipient.
    pub fn push_notification(&self, notification: Notification) {
        tracing::info!(
            "[NotificationStore] push_notification: id={} type={:?} from={} to={} msg={}",
            notification.id, notification.notification_type,
            notification.from_username, notification.to_username,
            notification.message
        );

        // Store in-memory for HTTP poll fallback
        let queue_len = {
            let mut entries = self.notifications
                .entry(notification.to_username.clone())
                .or_insert_with(Vec::new);
            entries.push(notification.clone());
            entries.len()
        };
        tracing::info!("[NotificationStore] push_notification: stored for {} ({} pending)", notification.to_username, queue_len);

        // Push to open WebSocket connections for recipient
        let ws_count = self.connected_users.get(&notification.to_username)
            .map(|entries| {
                let count = entries.value().len();
                for tx in entries.value().iter() {
                    match tx.send(notification.clone()) {
                        Ok(()) => tracing::info!("[NotificationStore] push_notification: sent to WS for {}", notification.to_username),
                        Err(e) => tracing::warn!("[NotificationStore] push_notification: WS send failed for {}: {}", notification.to_username, e),
                    }
                }
                count
            })
            .unwrap_or(0);

        if ws_count == 0 {
            tracing::info!("[NotificationStore] push_notification: no active WS connection for {}, stored for HTTP poll", notification.to_username);
        }
    }

    pub fn take_notifications(&self, username: &str) -> Vec<Notification> {
        let notes = self.notifications
            .remove(username)
            .map(|(_, v)| v)
            .unwrap_or_default();
        tracing::info!("[NotificationStore] take_notifications: {} -> {} notifications", username, notes.len());
        notes
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
            Some(Ok(Message::Text(t))) => {
                tracing::info!("[NotificationWS] received auth token ({} chars)", t.len());
                t
            }
            Some(Ok(other)) => {
                tracing::warn!("[NotificationWS] expected text token, got {:?}", other);
                return;
            }
            Some(Err(e)) => {
                tracing::warn!("[NotificationWS] error reading auth token: {}", e);
                return;
            }
            None => {
                tracing::warn!("[NotificationWS] connection closed before auth");
                return;
            }
        };

        let username = match self.verify_github_token(&token).await {
            Ok((uname, _)) => {
                tracing::info!("[NotificationWS] auth success: {}", uname);
                uname
            }
            Err(e) => {
                tracing::warn!("[NotificationWS] auth failed: {}", e);
                let _ = ws.send(Message::Text(
                    serde_json::json!({"error": "auth_failed"}).to_string().into(),
                )).await;
                return;
            }
        };

        let (tx, mut rx) = mpsc::unbounded_channel::<Notification>();
        self.register_connection(&username, tx.clone());

        // Send any pending in-memory notifications immediately
        let pending = self.take_notifications(&username);
        tracing::info!("[NotificationWS] {}: {} pending notifications to replay", username, pending.len());
        for note in &pending {
            tracing::info!("[NotificationWS] {}: replaying notification id={}", username, note.id);
            if let Ok(json) = serde_json::to_string(note) {
                if let Err(e) = ws.send(Message::Text(json.into())).await {
                    tracing::warn!("[NotificationWS] {}: failed to replay notification: {}", username, e);
                    return;
                }
            }
        }
        tracing::info!("[NotificationWS] {}: pending replay complete", username);

        // Forward new notifications from the channel to the WebSocket
        let (mut ws_sender, mut ws_receiver) = ws.split();

        let fwd_username = username.clone();
        let forward_task = tokio::spawn(async move {
            tracing::info!("[NotificationWS] {}: forward task started", fwd_username);
            while let Some(note) = rx.recv().await {
                tracing::info!("[NotificationWS] {}: forwarding notification id={} type={:?}", fwd_username, note.id, note.notification_type);
                if let Ok(json) = serde_json::to_string(&note) {
                    if let Err(e) = ws_sender.send(Message::Text(json.into())).await {
                        tracing::warn!("[NotificationWS] {}: forward task send error: {}", fwd_username, e);
                        break;
                    }
                    tracing::info!("[NotificationWS] {}: forwarded notification id={}", fwd_username, note.id);
                }
            }
            tracing::info!("[NotificationWS] {}: forward task ended", fwd_username);
        });

        // Drain any inbound messages until connection closes
        tracing::info!("[NotificationWS] {}: listening for inbound messages", username);
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Close(_)) => {
                    tracing::info!("[NotificationWS] {}: received close frame", username);
                    break;
                }
                Err(e) => {
                    tracing::warn!("[NotificationWS] {}: websocket error: {}", username, e);
                    break;
                }
                Ok(Message::Ping(_)) => {
                    tracing::info!("[NotificationWS] {}: received ping", username);
                }
                Ok(Message::Pong(_)) => {
                    tracing::info!("[NotificationWS] {}: received pong", username);
                }
                Ok(Message::Text(t)) => {
                    tracing::info!("[NotificationWS] {}: received unexpected text: {}", username, t);
                }
                Ok(Message::Binary(d)) => {
                    tracing::info!("[NotificationWS] {}: received unexpected binary ({} bytes)", username, d.len());
                }
            }
        }

        forward_task.abort();
        self.unregister_connection(&username, &tx);
        tracing::info!("[NotificationWS] {}: disconnected", username);
    }
}
