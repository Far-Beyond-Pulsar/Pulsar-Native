use async_trait::async_trait;
use thiserror::Error;

use crate::protocol::SessionMessage;

#[async_trait]
pub trait SessionChannel: Send + Sync {
    async fn send(&self, msg: SessionMessage) -> Result<(), SessionError>;
    async fn recv(&self) -> Result<SessionMessage, SessionError>;
    fn is_connected(&self) -> bool;
    async fn close(&self) -> Result<(), SessionError>;
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("Connection closed")]
    ConnectionClosed,
    #[error("Timeout")]
    Timeout,
    #[error("Protocol error: {0}")]
    Protocol(String),
    #[error("IO error: {0}")]
    Io(String),
}
