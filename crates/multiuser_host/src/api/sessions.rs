use std::collections::HashMap;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures::{SinkExt as _, StreamExt as _};
use tracing::{debug, info, warn};

use crate::auth::extract_token;
use crate::sessions::WsMessage;
use crate::state::AppState;

/// `GET /api/v1/projects/:id/session` — upgrade to WebSocket.
///
/// Query parameters:
/// - `user`  — username to display to collaborators (required)
/// - `token` — bearer auth token if the server requires it (can also be sent
///             as the `Authorization` header before upgrade)
pub async fn join_session(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    // Extract username early so it can be used in log messages.
    let username = params
        .get("user")
        .cloned()
        .unwrap_or_else(|| "anonymous".to_string());

    debug!("WS /projects/{project_id}/session — connection attempt from '{username}'");

    // Auth check — token can arrive as a query param or Authorization header.
    if state.config.auth_required() {
        let token = params
            .get("token")
            .map(|t| t.as_str())
            .or_else(|| extract_token(&headers));

        match token {
            Some(t) if state.config.verify_token(t) => {
                debug!("WS /projects/{project_id}/session — auth OK for '{username}'");
            }
            Some(_) => {
                warn!("WS /projects/{project_id}/session — invalid token from '{username}'");
                return StatusCode::UNAUTHORIZED.into_response();
            }
            None => {
                warn!("WS /projects/{project_id}/session — no token provided by '{username}'");
                return StatusCode::UNAUTHORIZED.into_response();
            }
        }
    }

    // Project must exist.
    if state.projects.get(&project_id).is_none() {
        warn!("WS /projects/{project_id}/session — project not found (user '{username}')");
        return StatusCode::NOT_FOUND.into_response();
    }

    info!("WS /projects/{project_id}/session — upgrading connection for '{username}'");
    ws.on_upgrade(move |socket| handle_socket(socket, state, project_id, username))
}

async fn handle_socket(socket: WebSocket, state: AppState, project_id: String, username: String) {
    let (mut sender, mut receiver) = socket.split();

    // Register the user and get the broadcast channel + current user list.
    let (tx, user_list) = state.sessions.user_joined(&project_id, &username);
    let mut rx = tx.subscribe();

    info!(
        "WS '{username}' joined project '{project_id}' — {} user(s) now present",
        user_list.len()
    );

    // Send the initial user list to the newly joined member.
    let list_msg = WsMessage::UserList { users: user_list };
    if let Ok(json) = serde_json::to_string(&list_msg) {
        let _ = sender.send(Message::Text(json.into())).await;
    }

    // Ensure project is Running while users are present.
    let _ = state.projects.begin_prepare(&project_id);

    // ── Task: forward broadcast messages to this client ────────────────────
    let send_task = tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if sender.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    // ── Main loop: receive messages from this client ───────────────────────
    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(text) => match serde_json::from_str::<WsMessage>(&text) {
                Ok(WsMessage::Ping) => {
                    debug!("WS ping from '{username}' in project '{project_id}'");
                    let _ = tx.send(WsMessage::Pong);
                }
                Ok(WsMessage::StatePatch { patch }) => {
                    debug!("WS state-patch from '{username}' in project '{project_id}'");
                    let _ = tx.send(WsMessage::StatePatch { patch });
                }
                Ok(WsMessage::Chat {
                    text: chat_text, ..
                }) => {
                    info!("WS chat from '{username}' in project '{project_id}': {chat_text:?}");
                    let _ = tx.send(WsMessage::Chat {
                        user: username.clone(),
                        text: chat_text,
                    });
                }
                Err(e) => {
                    warn!("WS unparseable message from '{username}' in project '{project_id}': {e}")
                }
                _ => {}
            },
            Message::Close(_) => {
                debug!("WS close frame from '{username}' in project '{project_id}'");
                break;
            }
            _ => {}
        }
    }

    // ── Cleanup ────────────────────────────────────────────────────────────
    send_task.abort();
    state.sessions.user_left(&project_id, &username);

    let users_remaining = state.sessions.user_count(&project_id);
    info!(
        "WS '{username}' left project '{project_id}' — {} user(s) remaining",
        users_remaining
    );

    if users_remaining == 0 {
        state.sessions.cleanup_if_empty(&project_id);
        state.projects.mark_idle(&project_id);
        info!("No users left in project '{project_id}' — marked idle");
    }
}
