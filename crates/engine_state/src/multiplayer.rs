use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use engine_fs::providers::p2p::P2pFsProvider;
use engine_fs::providers::FsProvider;
use pulsar_multiplayer_core::protocol::SessionMessage;
use pulsar_multiplayer_core::replication::Replicator;
use pulsar_multiplayer_core::session::{ParticipantInfo, Role, SessionInfo, SessionMode};
use pulsar_multiplayer_core::transport::{SessionChannel, SessionError};

pub struct GameSession {
    session_info: SessionInfo,
    channel: Arc<dyn SessionChannel>,
    fs_provider: Arc<dyn FsProvider>,
    replicators: Vec<Arc<RwLock<dyn Replicator>>>,
}

impl GameSession {
    pub async fn new_hosted(
        server_url: String,
        project_id: String,
        channel: Arc<dyn SessionChannel>,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let session_info = SessionInfo {
            id: project_id.clone(),
            host_id: "local".to_string(),
            participants: vec![],
            created_at: now,
            mode: SessionMode::Hosted {
                server_url,
                project_id,
            },
            metadata: HashMap::new(),
        };

        let fs_provider = Arc::new(P2pFsProvider::new(channel.clone(), session_info.id.clone()));

        Self {
            session_info,
            channel,
            fs_provider,
            replicators: vec![],
        }
    }

    pub async fn new_p2p(
        session_id: String,
        relay_url: Option<String>,
        channel: Arc<dyn SessionChannel>,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let session_info = SessionInfo {
            id: session_id,
            host_id: "local".to_string(),
            participants: vec![],
            created_at: now,
            mode: SessionMode::P2P { relay_url },
            metadata: HashMap::new(),
        };

        let fs_provider = Arc::new(P2pFsProvider::new(channel.clone(), session_info.id.clone()));

        Self {
            session_info,
            channel,
            fs_provider,
            replicators: vec![],
        }
    }

    pub fn session_info(&self) -> &SessionInfo {
        &self.session_info
    }

    pub fn channel(&self) -> &Arc<dyn SessionChannel> {
        &self.channel
    }

    pub fn fs_provider(&self) -> &Arc<dyn FsProvider> {
        &self.fs_provider
    }

    pub fn register_replicator(&mut self, replicator: Arc<RwLock<dyn Replicator>>) {
        if replicator.read().channel().is_none() {
            replicator.write().set_channel(self.channel.clone());
        }
        self.replicators.push(replicator);
    }

    pub fn registered_replicators(&self) -> &[Arc<RwLock<dyn Replicator>>] {
        &self.replicators
    }

    pub async fn broadcast(&self, msg: SessionMessage) -> Result<(), SessionError> {
        self.channel.send(msg).await
    }

    pub async fn sync_all(&self) {
        for replicator in &self.replicators {
            let r = replicator.read();
            if r.channel().is_some() {
                let _ = r.sync_state().await;
            }
        }
    }

    pub fn add_participant(&mut self, participant: ParticipantInfo) {
        self.session_info.participants.push(participant);
    }

    pub fn remove_participant(&mut self, peer_id: &str) {
        self.session_info.participants.retain(|p| p.peer_id != peer_id);
    }
}
