//! HTTP admin server with Axum
//!
//! Provides REST API endpoints for session management, health checks,
//! metrics, and WebSocket signaling.

use anyhow::Result;
use axum::{
    extract::{Path, State, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{error, info};

use crate::auth::AuthService;
use pulsar_multiplayer_core::session::Role;
use crate::config::Config;
use crate::health::HealthChecker;
use crate::metrics::METRICS;
use crate::notifications::{GitHubAuthRequest, GitHubAuthResponse, Notification, NotificationStore, PushNotificationRequest};
use crate::rendezvous::RendezvousCoordinator;
use crate::session::SessionStore;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub auth: Arc<AuthService>,
    pub sessions: Arc<SessionStore>,
    pub health: Arc<HealthChecker>,
    pub rendezvous: Arc<RendezvousCoordinator>,
    pub notifications: Arc<NotificationStore>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub host_id: String,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub session_id: String,
    pub join_token: String,
    pub expires_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JoinSessionRequest {
    pub join_token: String,
    pub peer_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JoinSessionResponse {
    pub session_id: String,
    pub peer_id: String,
    pub role: Role,
    pub participant_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
}

impl IntoResponse for ErrorResponse {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(self)).into_response()
    }
}

pub async fn run_server(config: Arc<Config>, shutdown: mpsc::Receiver<()>) -> Result<()> {
    let bind_addr = config.http_bind;

    // Initialize services
    let auth = Arc::new(AuthService::new(&config)?);
    let sessions = Arc::new(SessionStore::new(config.clone()));
    let health = Arc::new(HealthChecker::new(config.clone()));
    let rendezvous = Arc::new(RendezvousCoordinator::new(auth.clone(), (*config).clone()));

    let notifications = Arc::new(NotificationStore::new());

    let state = AppState {
        config,
        auth,
        sessions,
        health,
        rendezvous,
        notifications,
    };

    let app = create_router(state);

    info!("🌐 HTTP server binding to {}", bind_addr);

    // Use stdlib TcpListener to match Python's behavior exactly
    let std_listener = std::net::TcpListener::bind(bind_addr)?;
    std_listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(std_listener)?;

    info!("🌐 HTTP server ready - accepting connections");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown))
        .await?;

    Ok(())
}

fn create_router(state: AppState) -> Router {
    Router::new()
        // Health and metrics
        .route("/health", get(health_check))
        .route("/health/liveness", get(liveness_check))
        .route("/health/readiness", get(readiness_check))
        .route("/metrics", get(metrics_handler))
        .route("/metrics/json", get(metrics_json_handler))
        // Session management
        .route("/v1/sessions", post(create_session))
        .route("/v1/sessions/{id}/join", post(join_session))
        .route("/v1/sessions/{id}/close", post(close_session))
        .route("/v1/sessions/{id}", get(get_session))
        // WebSocket signaling
        .route("/v1/signaling", get(websocket_handler))
        .route("/ws", get(websocket_handler))
        // GitHub auth & notifications
        .route("/api/v1/auth/github", post(github_auth_handler))
        .route("/api/v1/notifications", get(get_notifications).post(push_notification_handler))
        .route("/api/v1/notifications/relay", post(relay_notification_handler))
        .route("/api/v1/notifications/ws", get(notifications_websocket_handler))
        .route("/api/v1/users/{username}/online", get(user_online_handler))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    let status = state.health.check_health().await;
    Json(status)
}

async fn liveness_check(State(state): State<AppState>) -> impl IntoResponse {
    let status = state.health.liveness_check().await;
    (StatusCode::OK, Json(status))
}

async fn readiness_check(State(state): State<AppState>) -> impl IntoResponse {
    let status = state.health.check_health().await;
    match status.status.as_str() {
        "healthy" => (StatusCode::OK, Json(status)),
        "degraded" => (StatusCode::OK, Json(status)),
        _ => (StatusCode::SERVICE_UNAVAILABLE, Json(status)),
    }
}

async fn metrics_handler() -> impl IntoResponse {
    match METRICS.encode() {
        Ok(metrics) => (StatusCode::OK, metrics),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to encode metrics: {}", e),
        ),
    }
}

async fn metrics_json_handler() -> impl IntoResponse {
    match METRICS.as_json() {
        Ok(metrics) => (StatusCode::OK, Json(metrics)),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to encode metrics: {}", e)})),
        ),
    }
}

async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<CreateSessionResponse>, ErrorResponse> {
    info!(
        "📝 Received create_session request from host: {}",
        req.host_id
    );

    let metadata = req.metadata.unwrap_or_else(|| serde_json::json!({}));
    info!("📋 Session metadata: {:?}", metadata);

    info!("🔨 Creating session in session manager...");
    let session = state
        .sessions
        .create_session(req.host_id.clone(), metadata)
        .map_err(|e| {
            error!("❌ Session creation failed: {}", e);
            ErrorResponse {
                error: "session_creation_failed".to_string(),
                message: e.to_string(),
            }
        })?;

    info!("✅ Session created with ID: {}", session.id);

    // Generate join token
    info!("🔑 Generating join token for session: {}", session.id);
    let join_token = state
        .auth
        .create_join_token(session.id.clone(), Role::Host, Duration::from_secs(3600))
        .map_err(|e| {
            error!("❌ Token generation failed: {}", e);
            ErrorResponse {
                error: "token_generation_failed".to_string(),
                message: e.to_string(),
            }
        })?;

    info!("✅ Join token generated for session: {}", session.id);
    info!("📤 Sending response to client");

    Ok(Json(CreateSessionResponse {
        session_id: session.id,
        join_token,
        expires_at: session.expires_at,
    }))
}

async fn join_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(req): Json<JoinSessionRequest>,
) -> Result<Json<JoinSessionResponse>, ErrorResponse> {
    info!(
        "👥 Received join_session request - Session: {}, Peer: {}",
        session_id, req.peer_id
    );

    // Verify join token
    info!("🔐 Verifying join token...");
    let (verified_session_id, role) =
        state.auth.verify_join_token(&req.join_token).map_err(|e| {
            error!("❌ Token verification failed: {}", e);
            ErrorResponse {
                error: "invalid_token".to_string(),
                message: e.to_string(),
            }
        })?;

    info!(
        "✅ Token verified - Session: {}, Role: {:?}",
        verified_session_id, role
    );

    if verified_session_id != session_id {
        error!(
            "❌ Session ID mismatch - Token: {}, Requested: {}",
            verified_session_id, session_id
        );
        return Err(ErrorResponse {
            error: "session_mismatch".to_string(),
            message: "Token session ID does not match".to_string(),
        });
    }

    // Join session
    info!("🚪 Joining session: {}", session_id);
    let session = state
        .sessions
        .join_session(&session_id, req.peer_id.clone(), role.clone())
        .map_err(|e| {
            error!("❌ Failed to join session: {}", e);
            ErrorResponse {
                error: "join_failed".to_string(),
                message: e.to_string(),
            }
        })?;

    info!(
        "✅ Peer {} joined session {} - Total participants: {}",
        req.peer_id,
        session_id,
        session.participants.len()
    );

    Ok(Json(JoinSessionResponse {
        session_id: session.id,
        peer_id: req.peer_id,
        role,
        participant_count: session.participants.len(),
    }))
}

async fn close_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, ErrorResponse> {
    state
        .sessions
        .close_session(&session_id, "user_requested")
        .map_err(|e| ErrorResponse {
            error: "close_failed".to_string(),
            message: e.to_string(),
        })?;

    Ok(StatusCode::NO_CONTENT)
}

async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    let session = state
        .sessions
        .get_session(&session_id)
        .ok_or_else(|| ErrorResponse {
            error: "not_found".to_string(),
            message: "Session not found".to_string(),
        })?;

    Ok(Json(serde_json::to_value(session).unwrap()))
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // Delegate to the rendezvous coordinator
    RendezvousCoordinator::handle_websocket(State(state.rendezvous.clone()), ws).await
}

async fn shutdown_signal(mut shutdown: mpsc::Receiver<()>) {
    shutdown.recv().await;
    info!("🛑 HTTP server shutdown signal received");
}

// ── GitHub Auth & Notification Handlers ─────────────────────

async fn github_auth_handler(
    State(state): State<AppState>,
    Json(req): Json<GitHubAuthRequest>,
) -> Result<Json<GitHubAuthResponse>, ErrorResponse> {
    info!("🔐 GitHub auth request received");

    let (github_login, github_id) = state
        .notifications
        .verify_github_token(&req.token)
        .await
        .map_err(|e| ErrorResponse {
            error: "auth_failed".to_string(),
            message: e.to_string(),
        })?;

    // Issue a short-lived server JWT for subsequent API calls
    let server_token = state
        .auth
        .create_token(
            github_login.clone(),
            "auth".to_string(),
            pulsar_multiplayer_core::session::Role::Editor,
            std::time::Duration::from_secs(3600),
        )
        .map_err(|e| ErrorResponse {
            error: "token_creation_failed".to_string(),
            message: e.to_string(),
        })?;

    info!("✅ GitHub auth successful for {}", github_login);

    Ok(Json(GitHubAuthResponse {
        github_login,
        github_id,
        server_token,
    }))
}

async fn get_notifications(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<Notification>>, ErrorResponse> {
    let username = params.get("username").ok_or_else(|| ErrorResponse {
        error: "missing_parameter".to_string(),
        message: "username query parameter is required".to_string(),
    })?;

    if !state.notifications.is_user_verified(username) {
        return Err(ErrorResponse {
            error: "unverified_user".to_string(),
            message: "User must authenticate via /api/v1/auth/github first".to_string(),
        });
    }

    let notes = state.notifications.take_notifications(username);
    info!("📬 Returning {} notifications for {}", notes.len(), username);
    Ok(Json(notes))
}

async fn push_notification_handler(
    State(state): State<AppState>,
    Json(notification): Json<Notification>,
) -> Result<impl IntoResponse, ErrorResponse> {
    info!(
        "📨 push_notification_handler: id={} type={:?} from={} to={} msg={}",
        notification.id, notification.notification_type,
        notification.from_username, notification.to_username,
        notification.message
    );
    state.notifications.push_notification(notification);
    info!("📨 push_notification_handler: done");
    Ok(StatusCode::ACCEPTED)
}

async fn relay_notification_handler(
    State(state): State<AppState>,
    Json(req): Json<PushNotificationRequest>,
) -> Result<impl IntoResponse, ErrorResponse> {
    info!(
        "🔄 Relaying notification from {} to {} at {}",
        req.from_username, req.target_username, req.target_home_server
    );

    state
        .notifications
        .relay_notification(&req)
        .await
        .map_err(|e| ErrorResponse {
            error: "relay_failed".to_string(),
            message: e.to_string(),
        })?;

    info!("✅ Notification relayed successfully");
    Ok(StatusCode::ACCEPTED)
}

async fn notifications_websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| {
        state.notifications.clone().handle_websocket(socket)
    })
}

async fn user_online_handler(
    Path(username): Path<String>,
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let online = state.notifications.is_user_online(&username);
    info!("👤 online check: {} -> {}", username, online);
    Json(serde_json::json!({ "username": username, "online": online }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_response() {
        let err = ErrorResponse {
            error: "test_error".to_string(),
            message: "Test message".to_string(),
        };

        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("test_error"));
    }
}
