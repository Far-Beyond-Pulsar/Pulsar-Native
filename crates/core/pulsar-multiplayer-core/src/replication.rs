use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::protocol::{SessionMessage, StateUpdate};
use crate::transport::{SessionChannel, SessionError};

#[async_trait]
pub trait Replicator: Send + Sync {
    fn replication_id(&self) -> &str;
    fn serialize_state(&self) -> Result<Value, String>;
    fn deserialize_state(&mut self, state: Value) -> Result<(), String>;
    fn channel(&self) -> Option<Arc<dyn SessionChannel>>;
    fn set_channel(&mut self, channel: Arc<dyn SessionChannel>);

    fn on_session_start(&mut self, channel: Arc<dyn SessionChannel>) {
        self.set_channel(channel);
    }

    fn on_session_end(&mut self);

    async fn sync_state(&self) -> Result<(), SessionError> {
        let channel = self
            .channel()
            .ok_or_else(|| SessionError::Protocol("No session channel".into()))?;
        let msg = SessionMessage::StateUpdate(StateUpdate {
            element_id: self.replication_id().to_string(),
            state: self
                .serialize_state()
                .map_err(|e| SessionError::Protocol(e))?,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| SessionError::Protocol(e.to_string()))?
                .as_secs(),
        });
        channel.send(msg).await
    }

    async fn apply_remote_update(&mut self, state: Value) -> Result<(), String> {
        self.deserialize_state(state)
    }
}
