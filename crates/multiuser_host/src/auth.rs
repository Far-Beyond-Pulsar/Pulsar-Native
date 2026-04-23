use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use tracing::{debug, warn};

use crate::state::AppState;

/// Extract the raw Bearer token from an `Authorization` header, if present.
pub fn extract_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
}

/// Axum middleware that enforces Bearer token authentication when the server
/// was started with an `--auth-token`.  Unauthenticated requests to protected
/// endpoints receive `401 Unauthorized`.
///
/// Use [`require_auth`] as an Axum layer:
/// ```
/// Router::new()
///     .route(...)
///     .layer(axum::middleware::from_fn_with_state(state, require_auth))
/// ```
pub async fn require_auth(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if !state.config.auth_required() {
        return Ok(next.run(request).await);
    }

    let path = request.uri().path().to_owned();
    let method = request.method().clone();

    let token = extract_token(request.headers()).ok_or_else(|| {
        warn!("{method} {path} — auth rejected: no Bearer token");
        StatusCode::UNAUTHORIZED
    })?;

    if state.config.verify_token(token) {
        debug!("{method} {path} — auth OK");
        Ok(next.run(request).await)
    } else {
        warn!("{method} {path} — auth rejected: invalid token");
        Err(StatusCode::UNAUTHORIZED)
    }
}
