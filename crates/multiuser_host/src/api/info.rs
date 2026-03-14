use axum::{extract::State, http::StatusCode, Json};
use serde_json::{json, Value};

use crate::state::AppState;

/// `GET /api/v1/info`
///
/// Returns public server metadata and live statistics.  No authentication
/// required so that launcher clients can poll reachability without a token.
pub async fn get_info(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let active_users    = state.sessions.total_user_count();
    let active_projects = state.sessions.active_project_count();

    let body = json!({
        "server_name":      state.config.server_name,
        "version":          env!("CARGO_PKG_VERSION"),
        "active_users":     active_users,
        "active_projects":  active_projects,
        "uptime_seconds":   state.uptime_seconds(),
        "max_projects":     state.config.max_projects,
    });

    (StatusCode::OK, Json(body))
}
