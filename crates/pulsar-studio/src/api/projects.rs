use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

use crate::projects::ProjectStatus;
use crate::state::AppState;

// ── Shared serialisation helper ───────────────────────────────────────────────

fn project_to_json(p: &crate::projects::ProjectRecord, user_count: usize) -> Value {
    let (status_str, error_msg) = match &p.status {
        ProjectStatus::Idle => ("idle", None),
        ProjectStatus::Preparing => ("preparing", None),
        ProjectStatus::Running => ("running", None),
        ProjectStatus::Error(m) => ("error", Some(m.as_str())),
    };

    let mut v = json!({
        "id":            p.id,
        "name":          p.name,
        "description":   p.description,
        "owner":         p.owner,
        "status":        status_str,
        "user_count":    user_count,
        "last_modified": p.last_modified.to_rfc3339(),
        "size_bytes":    p.size_bytes,
    });

    if let Some(msg) = error_msg {
        v["error_msg"] = json!(msg);
    }

    v
}

// ── List projects ─────────────────────────────────────────────────────────────

/// `GET /api/v1/projects`
pub async fn list_projects(State(state): State<AppState>) -> Json<Value> {
    let projects = state.projects.list();
    debug!("GET /projects — {} project(s) in store", projects.len());
    let items: Vec<Value> = projects
        .iter()
        .map(|p| project_to_json(p, state.sessions.user_count(&p.id)))
        .collect();
    Json(json!(items))
}

// ── Get single project ────────────────────────────────────────────────────────

/// `GET /api/v1/projects/:id`
pub async fn get_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    debug!("GET /projects/{id}");
    let project = state.projects.get(&id).ok_or_else(|| {
        debug!("GET /projects/{id} — not found");
        StatusCode::NOT_FOUND
    })?;
    let user_count = state.sessions.user_count(&id);
    debug!(
        "GET /projects/{id} — '{}' [{}] {} user(s)",
        project.name,
        project.status.as_str(),
        user_count
    );
    Ok(Json(project_to_json(&project, user_count)))
}

// ── Create project ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateProjectBody {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub owner: String,
}

/// `POST /api/v1/projects`
pub async fn create_project(
    State(state): State<AppState>,
    Json(body): Json<CreateProjectBody>,
) -> Result<(StatusCode, Json<Value>), StatusCode> {
    info!(
        "POST /projects — name={:?} owner={:?}",
        body.name, body.owner
    );

    if body.name.trim().is_empty() {
        warn!("POST /projects — rejected empty name");
        return Err(StatusCode::BAD_REQUEST);
    }

    let current = state.projects.count();
    if current >= state.config.max_projects {
        warn!(
            "POST /projects — rejected: at project limit {}/{}",
            current, state.config.max_projects
        );
        return Err(StatusCode::INSUFFICIENT_STORAGE);
    }

    match state
        .projects
        .create(body.name.clone(), body.description, body.owner)
    {
        Ok(record) => {
            info!("POST /projects — created '{}' ({})", record.name, record.id);
            Ok((StatusCode::CREATED, Json(project_to_json(&record, 0))))
        }
        Err(e) => {
            error!("POST /projects — failed to create '{}': {e}", body.name);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// ── Prepare project ───────────────────────────────────────────────────────────

/// `POST /api/v1/projects/:id/prepare`
///
/// Tells the server to warm up the project so it is ready for immediate
/// editing.  Spawns a background task that transitions the project through
/// Preparing → Running.  If the project is already Running the call is
/// idempotent (returns 200).
pub async fn prepare_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    info!("POST /projects/{id}/prepare");

    if state.projects.get(&id).is_none() {
        warn!("POST /projects/{id}/prepare — project not found");
        return Err(StatusCode::NOT_FOUND);
    }

    match state.projects.begin_prepare(&id) {
        Ok(false) => {
            // Already preparing / running — idempotent success.
            debug!("POST /projects/{id}/prepare — already active, skipping");
        }
        Ok(true) => {
            info!("POST /projects/{id}/prepare — starting preparation background task");
            // Spawn a background task to complete the "prepare" work.
            // In a real implementation this would load project state into
            // memory.  For now we transition immediately to Running.
            let pm = state.projects.clone();
            let project_id = id.clone();
            tokio::spawn(async move {
                // Simulate a brief async preparation step.
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                pm.mark_running(&project_id);
                pm.update_size(&project_id);
                info!("POST /projects/{project_id}/prepare — project is now running");
            });
        }
        Err(e) => {
            error!("POST /projects/{id}/prepare — begin_prepare failed: {e}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    let project = state.projects.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    let user_count = state.sessions.user_count(&id);
    debug!(
        "POST /projects/{id}/prepare — returning status={}",
        project.status.as_str()
    );
    Ok(Json(project_to_json(&project, user_count)))
}

// ── Stop project ──────────────────────────────────────────────────────────────

/// `POST /api/v1/projects/:id/stop`
///
/// Forces the project back to `Idle`, disconnecting any active preparation.
/// Returns the updated project record.
pub async fn stop_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    info!("POST /projects/{id}/stop");

    let project = state.projects.get(&id).ok_or_else(|| {
        warn!("POST /projects/{id}/stop — project not found");
        StatusCode::NOT_FOUND
    })?;

    let prev_status = project.status.as_str();
    if state.projects.stop(&id) {
        info!("POST /projects/{id}/stop — {prev_status} → idle");
    } else {
        debug!("POST /projects/{id}/stop — already idle, no-op");
    }

    let updated = state.projects.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    let user_count = state.sessions.user_count(&id);
    Ok(Json(project_to_json(&updated, user_count)))
}

// ── Delete project ────────────────────────────────────────────────────────────

/// `DELETE /api/v1/projects/:id`
pub async fn delete_project(State(state): State<AppState>, Path(id): Path<String>) -> StatusCode {
    info!("DELETE /projects/{id}");
    match state.projects.delete(&id) {
        Ok(true) => {
            info!("DELETE /projects/{id} — deleted");
            StatusCode::NO_CONTENT
        }
        Ok(false) => {
            warn!("DELETE /projects/{id} — not found");
            StatusCode::NOT_FOUND
        }
        Err(e) => {
            error!("DELETE /projects/{id} — failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}
