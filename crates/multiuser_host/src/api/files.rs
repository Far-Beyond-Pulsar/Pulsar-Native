//! File API for `pulsar-host`
//!
//! Exposes a project's workspace directory over HTTP so that remote editors
//! can read and write files as if they were working locally.
//!
//! ## Security
//!
//! * All user-supplied paths are validated against the project workspace root
//!   before any filesystem I/O.  Any path component that resolves to `..` – or
//!   an absolute path that escapes the workspace – is rejected with `400 Bad
//!   Request`.
//!
//! * Mutating endpoints (PUT, DELETE, POST rename/mkdir) are protected by the
//!   server-level Bearer-token middleware that wraps the `protected` router.
//!
//! ## Endpoints
//!
//! | Method | Path                                      | Description                      |
//! |--------|-------------------------------------------|----------------------------------|
//! | GET    | /files/list[?path=rel]                    | List one directory level         |
//! | GET    | /files[?path=rel]                         | Read a file (base64-encoded)     |
//! | GET    | /files/manifest[?path=rel]                | Full recursive file tree         |
//! | GET    | /files/exists?path=rel                    | Check whether a path exists      |
//! | GET    | /files/stat?path=rel                      | Basic metadata                   |
//! | PUT    | /files?path=rel[&create=true]             | Write / create a file            |
//! | DELETE | /files?path=rel                           | Delete a file or directory       |
//! | POST   | /files/mkdir?path=rel                     | Create a folder (recursive)      |
//! | POST   | /files/rename                             | Rename / move a path             |

use axum::{
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, warn};

use crate::state::AppState;

// ── Query / request / response types ─────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PathQuery {
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WriteQuery {
    path: Option<String>,
    /// If `true`, fail when the file already exists.
    create: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct WriteBody {
    /// Base64-encoded file contents.
    content: String,
}

#[derive(Debug, Deserialize)]
pub struct RenameBody {
    from: String,
    to: String,
}

#[derive(Debug, Serialize)]
struct DirEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<u64>,
}

#[derive(Debug, Serialize)]
struct ReadResponse {
    content: String, // base64
    size: u64,
}

#[derive(Debug, Serialize)]
struct ExistsResponse {
    exists: bool,
}

#[derive(Debug, Serialize)]
struct StatResponse {
    is_dir: bool,
    size: u64,
    modified: Option<u64>,
}

#[derive(Debug, Serialize)]
struct ManifestEntry {
    path: String,
    is_dir: bool,
    size: u64,
    modified: Option<u64>,
}

// ── Workspace root helper ─────────────────────────────────────────────────────

fn workspace_root(state: &AppState, project_id: &str) -> PathBuf {
    state
        .config
        .data_dir
        .join("projects")
        .join(project_id)
        .join("workspace")
}

// ── Path validation ───────────────────────────────────────────────────────────

/// Resolve `rel` under `root`, ensuring the result stays inside `root`.
///
/// Returns a `400` response on any traversal attempt.
fn safe_join(root: &std::path::Path, rel: &str) -> Result<PathBuf, Response> {
    // Reject obviously-dangerous inputs up-front.
    if rel.contains("..") {
        warn!("Path traversal attempt rejected: {:?}", rel);
        return Err((StatusCode::BAD_REQUEST, "Path traversal is not permitted").into_response());
    }

    // Normalise separators so Windows paths don't confuse things.
    let rel = rel
        .trim_start_matches('/')
        .trim_start_matches('\\')
        .replace('\\', "/");
    let candidate = root.join(&rel);

    // After joining, canonicalize both paths if the target exists; otherwise
    // compare lexicographically with both normalized to forward slashes to
    // prevent false negatives on Windows where PathBuf::join inserts '\'.
    let is_safe = if candidate.exists() {
        match (candidate.canonicalize(), root.canonicalize()) {
            (Ok(canon_candidate), Ok(canon_root)) => canon_candidate.starts_with(&canon_root),
            _ => false,
        }
    } else {
        // Normalize both strings to forward slashes.  On Windows PathBuf::join
        // produces backslashes while the root may have been configured with
        // forward slashes, so a naive starts_with would fail.
        let joined_str = candidate.to_string_lossy().replace('\\', "/");
        let root_str = root.to_string_lossy().replace('\\', "/");
        // Ensure a trailing '/' so "workspace_extra" can't match "workspace".
        let root_prefix = format!("{}/", root_str.trim_end_matches('/'));
        joined_str.starts_with(&root_prefix)
    };

    if !is_safe {
        warn!("Path escapes workspace root: {:?}", candidate);
        return Err((StatusCode::BAD_REQUEST, "Path escapes workspace root").into_response());
    }

    Ok(candidate)
}

/// Return an error response when the project does not exist.
fn require_project(state: &AppState, project_id: &str) -> Option<Response> {
    if state.projects.get(project_id).is_none() {
        Some((StatusCode::NOT_FOUND, "Project not found").into_response())
    } else {
        None
    }
}

// ── List directory ────────────────────────────────────────────────────────────

pub async fn list_dir(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Query(q): Query<PathQuery>,
) -> Response {
    if let Some(e) = require_project(&state, &project_id) {
        return e;
    }
    let root = workspace_root(&state, &project_id);
    let target = match q.path.as_deref() {
        Some(p) if !p.is_empty() => match safe_join(&root, p) {
            Ok(p) => p,
            Err(e) => return e,
        },
        _ => root.clone(),
    };

    // Ensure the workspace directory exists (auto-create on first access, same
    // as the manifest endpoint does).
    if !root.exists() {
        if let Err(e) = std::fs::create_dir_all(&root) {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }

    // Ensure workspace exists.
    if !target.is_dir() {
        return (StatusCode::NOT_FOUND, "Directory not found").into_response();
    }

    let entries = match std::fs::read_dir(&target) {
        Ok(rd) => rd,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };

    let mut out: Vec<DirEntry> = Vec::new();
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            out.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: meta.is_dir(),
                size: meta.len(),
                modified,
            });
        }
    }

    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Json(out).into_response()
}

// ── Read file ─────────────────────────────────────────────────────────────────

pub async fn read_file(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Query(q): Query<PathQuery>,
) -> Response {
    if let Some(e) = require_project(&state, &project_id) {
        return e;
    }
    let root = workspace_root(&state, &project_id);
    let path = match q.path.as_deref() {
        Some(p) if !p.is_empty() => match safe_join(&root, p) {
            Ok(p) => p,
            Err(e) => return e,
        },
        _ => return (StatusCode::BAD_REQUEST, "path query parameter required").into_response(),
    };

    if !path.is_file() {
        return (StatusCode::NOT_FOUND, "File not found").into_response();
    }

    match std::fs::read(&path) {
        Ok(bytes) => {
            use base64::Engine as _;
            let size = bytes.len() as u64;
            let content = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Json(ReadResponse { content, size }).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Write / create file ───────────────────────────────────────────────────────

pub async fn write_file(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Query(q): Query<WriteQuery>,
    Json(body): Json<WriteBody>,
) -> Response {
    if let Some(e) = require_project(&state, &project_id) {
        return e;
    }
    let root = workspace_root(&state, &project_id);
    let path = match q.path.as_deref() {
        Some(p) if !p.is_empty() => match safe_join(&root, p) {
            Ok(p) => p,
            Err(e) => return e,
        },
        _ => return (StatusCode::BAD_REQUEST, "path query parameter required").into_response(),
    };

    if q.create.unwrap_or(false) && path.exists() {
        return (StatusCode::CONFLICT, "File already exists").into_response();
    }

    use base64::Engine as _;
    let bytes = match base64::engine::general_purpose::STANDARD.decode(body.content.trim()) {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid base64 content").into_response(),
    };

    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }

    match std::fs::write(&path, &bytes) {
        Ok(()) => {
            // Broadcast file-change event to WebSocket subscribers.
            let kind = if q.create.unwrap_or(false) {
                "created"
            } else {
                "modified"
            };
            let rel = path
                .strip_prefix(&root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            state
                .sessions
                .broadcast_file_change(&project_id, rel, kind.to_string());

            StatusCode::OK.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Delete ────────────────────────────────────────────────────────────────────

pub async fn delete_path(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Query(q): Query<PathQuery>,
) -> Response {
    if let Some(e) = require_project(&state, &project_id) {
        return e;
    }
    let root = workspace_root(&state, &project_id);
    let path = match q.path.as_deref() {
        Some(p) if !p.is_empty() => match safe_join(&root, p) {
            Ok(p) => p,
            Err(e) => return e,
        },
        _ => return (StatusCode::BAD_REQUEST, "path query parameter required").into_response(),
    };

    let result = if path.is_dir() {
        std::fs::remove_dir_all(&path)
    } else {
        std::fs::remove_file(&path)
    };

    match result {
        Ok(()) => {
            let rel = path
                .strip_prefix(&root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            state
                .sessions
                .broadcast_file_change(&project_id, rel, "deleted".into());
            StatusCode::OK.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Create directory ──────────────────────────────────────────────────────────

pub async fn mkdir(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Query(q): Query<PathQuery>,
) -> Response {
    if let Some(e) = require_project(&state, &project_id) {
        return e;
    }
    let root = workspace_root(&state, &project_id);
    let path = match q.path.as_deref() {
        Some(p) if !p.is_empty() => match safe_join(&root, p) {
            Ok(p) => p,
            Err(e) => return e,
        },
        _ => return (StatusCode::BAD_REQUEST, "path query parameter required").into_response(),
    };

    match std::fs::create_dir_all(&path) {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Rename / move ─────────────────────────────────────────────────────────────

pub async fn rename(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Json(body): Json<RenameBody>,
) -> Response {
    if let Some(e) = require_project(&state, &project_id) {
        return e;
    }
    let root = workspace_root(&state, &project_id);

    let from = match safe_join(&root, &body.from) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let to = match safe_join(&root, &body.to) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if let Some(parent) = to.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }

    match std::fs::rename(&from, &to) {
        Ok(()) => {
            let from_rel = from
                .strip_prefix(&root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let to_rel = to
                .strip_prefix(&root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            state
                .sessions
                .broadcast_file_change(&project_id, from_rel, "deleted".into());
            state
                .sessions
                .broadcast_file_change(&project_id, to_rel, "created".into());
            StatusCode::OK.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Exists / stat ─────────────────────────────────────────────────────────────

pub async fn exists(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Query(q): Query<PathQuery>,
) -> Response {
    if let Some(e) = require_project(&state, &project_id) {
        return e;
    }
    let root = workspace_root(&state, &project_id);
    let path = match q.path.as_deref() {
        Some(p) if !p.is_empty() => match safe_join(&root, p) {
            Ok(p) => p,
            Err(e) => return e,
        },
        _ => root.clone(),
    };
    Json(ExistsResponse {
        exists: path.exists(),
    })
    .into_response()
}

pub async fn stat(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Query(q): Query<PathQuery>,
) -> Response {
    if let Some(e) = require_project(&state, &project_id) {
        return e;
    }
    let root = workspace_root(&state, &project_id);
    let path = match q.path.as_deref() {
        Some(p) if !p.is_empty() => match safe_join(&root, p) {
            Ok(p) => p,
            Err(e) => return e,
        },
        _ => root.clone(),
    };

    match std::fs::metadata(&path) {
        Ok(m) => {
            let modified = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            Json(StatResponse {
                is_dir: m.is_dir(),
                size: m.len(),
                modified,
            })
            .into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "Path not found").into_response(),
    }
}

// ── Manifest (full project tree) ──────────────────────────────────────────────

pub async fn manifest(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Query(q): Query<PathQuery>,
) -> Response {
    if let Some(e) = require_project(&state, &project_id) {
        return e;
    }
    let root = workspace_root(&state, &project_id);

    // Ensure workspace directory exists (auto-create on first access).
    if !root.exists() {
        if let Err(e) = std::fs::create_dir_all(&root) {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }

    let base = match q.path.as_deref() {
        Some(p) if !p.is_empty() => match safe_join(&root, p) {
            Ok(p) => p,
            Err(e) => return e,
        },
        _ => root.clone(),
    };

    let mut entries = Vec::new();
    collect_manifest(&base, &root, &mut entries);
    Json(entries).into_response()
}

fn collect_manifest(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<ManifestEntry>) {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };

    for entry in read_dir.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let path = entry.path();

        // Skip hidden and build-artefact directories.
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') || name_str == "target" {
            continue;
        }

        let rel = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();

        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        out.push(ManifestEntry {
            path: rel,
            is_dir: meta.is_dir(),
            size: meta.len(),
            modified,
        });

        if meta.is_dir() {
            collect_manifest(&path, root, out);
        }
    }
}
