use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::warn;

use crate::projects::ProjectStatus;
use crate::state::AppState;

// ── Shared serialisation helper ───────────────────────────────────────────────

fn project_to_json(p: &crate::projects::ProjectRecord, user_count: usize) -> Value {
    let (status_str, error_msg) = match &p.status {
        ProjectStatus::Idle      => ("idle", None),
        ProjectStatus::Preparing => ("preparing", None),
        ProjectStatus::Running   => ("running", None),
        ProjectStatus::Error(m)  => ("error", Some(m.as_str())),
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
    let project = state.projects.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    let user_count = state.sessions.user_count(&id);
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
    if body.name.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    if state.projects.count() >= state.config.max_projects {
        return Err(StatusCode::INSUFFICIENT_STORAGE);
    }

    match state.projects.create(body.name, body.description, body.owner) {
        Ok(record) => Ok((
            StatusCode::CREATED,
            Json(project_to_json(&record, 0)),
        )),
        Err(e) => {
            warn!("Failed to create project: {e}");
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
    if state.projects.get(&id).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    match state.projects.begin_prepare(&id) {
        Ok(false) => {
            // Already preparing / running — idempotent success.
        }
        Ok(true) => {
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
            });
        }
        Err(e) => {
            warn!("prepare_project error: {e}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    let project = state.projects.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    let user_count = state.sessions.user_count(&id);
    Ok(Json(project_to_json(&project, user_count)))
}

// ── Delete project ────────────────────────────────────────────────────────────

/// `DELETE /api/v1/projects/:id`
pub async fn delete_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> StatusCode {
    match state.projects.delete(&id) {
        Ok(true)  => StatusCode::NO_CONTENT,
        Ok(false) => StatusCode::NOT_FOUND,
        Err(e) => {
            warn!("delete_project error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}
