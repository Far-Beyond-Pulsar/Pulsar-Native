use async_trait::async_trait;
use thiserror::Error;

use crate::session::Role;

#[async_trait]
pub trait SessionAuth: Send + Sync {
    async fn create_join_token(
        &self,
        session_id: &str,
        role: Role,
        ttl: std::time::Duration,
    ) -> Result<String, AuthError>;
    async fn verify_join_token(&self, token: &str) -> Result<(String, Role), AuthError>;
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("Invalid token: {0}")]
    Invalid(String),
    #[error("Expired token")]
    Expired,
    #[error("Internal error: {0}")]
    Internal(String),
}
