use axum::{
    middleware,
    routing::{delete, get, post},
    Router,
};

use crate::auth::require_auth;
use crate::state::AppState;

pub mod info;
pub mod projects;
pub mod sessions;

/// Assemble and return the full `/api/v1` router.
pub fn router(state: AppState) -> Router {
    // Public endpoints (no auth middleware).
    let public = Router::new()
        .route("/info", get(info::get_info))
        .route("/projects", get(projects::list_projects))
        .route("/projects/:id", get(projects::get_project))
        .route("/projects/:id/session", get(sessions::join_session));

    // Write endpoints protected by the optional auth middleware.
    let protected = Router::new()
        .route("/projects", post(projects::create_project))
        .route("/projects/:id/prepare", post(projects::prepare_project))
        .route("/projects/:id", delete(projects::delete_project))
        .layer(middleware::from_fn_with_state(state.clone(), require_auth));

    Router::new()
        .nest("/api/v1", public.merge(protected))
        .with_state(state)
}
