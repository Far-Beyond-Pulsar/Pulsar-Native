use axum::{
    middleware,
    routing::{delete, get, post, put},
    Router,
};

use crate::auth::require_auth;
use crate::state::AppState;

pub mod info;
pub mod projects;
pub mod sessions;
pub mod files;

/// Assemble and return the full `/api/v1` router.
pub fn router(state: AppState) -> Router {
    // File read/browse is public so remote editors can list and read without
    // needing an auth token (same policy as project listing).
    let public_files = Router::new()
        .route("/projects/:id/files", get(files::read_file))
        .route("/projects/:id/files/list", get(files::list_dir))
        .route("/projects/:id/files/manifest", get(files::manifest))
        .route("/projects/:id/files/exists", get(files::exists))
        .route("/projects/:id/files/stat", get(files::stat));

    // File mutation endpoints require a valid Bearer token when the server has
    // auth configured.
    let protected_files = Router::new()
        .route("/projects/:id/files", put(files::write_file))
        .route("/projects/:id/files", delete(files::delete_path))
        .route("/projects/:id/files/mkdir", post(files::mkdir))
        .route("/projects/:id/files/rename", post(files::rename))
        .layer(middleware::from_fn_with_state(state.clone(), require_auth));

    // Public endpoints (no auth middleware).
    let public = Router::new()
        .route("/info", get(info::get_info))
        .route("/projects", get(projects::list_projects))
        .route("/projects/:id", get(projects::get_project))
        .route("/projects/:id/session", get(sessions::join_session))
        .merge(public_files);

    // Write endpoints protected by the optional auth middleware.
    let protected = Router::new()
        .route("/projects", post(projects::create_project))
        .route("/projects/:id/prepare", post(projects::prepare_project))
        .route("/projects/:id/stop", post(projects::stop_project))
        .route("/projects/:id", delete(projects::delete_project))
        .merge(protected_files)
        .layer(middleware::from_fn_with_state(state.clone(), require_auth));

    Router::new()
        .nest("/api/v1", public.merge(protected))
        .with_state(state)
}
