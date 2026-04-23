//! Per-peer state and session registry.

use dashmap::DashMap;
use std::time::SystemTime;
use tokio::sync::mpsc;

use super::sync_protocol::ServerMessage;

/// Peer session information
#[derive(Debug, Clone)]
pub(super) struct PeerSession {
    pub(super) peer_id: String,
    pub(super) session_id: String,
    pub(super) tx: mpsc::Sender<ServerMessage>,
    pub(super) joined_at: SystemTime,
}

impl PeerSession {
    pub(super) fn new(
        peer_id: String,
        session_id: String,
        tx: mpsc::Sender<ServerMessage>,
    ) -> Self {
        Self {
            peer_id,
            session_id,
            tx,
            joined_at: SystemTime::now(),
        }
    }
}

/// Rendezvous session
#[derive(Debug)]
pub(super) struct RendezvousSession {
    pub(super) session_id: String,
    pub(super) host_id: String,
    peers: DashMap<String, PeerSession>,
    pub(super) created_at: SystemTime,
}

impl RendezvousSession {
    pub(super) fn new(session_id: String, host_id: String) -> Self {
        Self {
            session_id,
            host_id,
            peers: DashMap::new(),
            created_at: SystemTime::now(),
        }
    }
    pub(super) fn add_peer(&self, peer: PeerSession) {
        self.peers.insert(peer.peer_id.clone(), peer);
    }
    pub(super) fn remove_peer(&self, peer_id: &str) -> Option<PeerSession> {
        self.peers.remove(peer_id).map(|(_, p)| p)
    }
    pub(super) fn get_peer(&self, peer_id: &str) -> Option<PeerSession> {
        self.peers.get(peer_id).map(|p| p.clone())
    }
    pub(super) fn list_peers(&self) -> Vec<PeerSession> {
        self.peers.iter().map(|p| p.value().clone()).collect()
    }
    pub(super) fn peer_count(&self) -> usize {
        self.peers.len()
    }
}
