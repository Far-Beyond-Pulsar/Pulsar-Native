//! Per-peer state and session tracking.

use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::Response,
};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Peer session information
#[derive(Debug, Clone)]
struct PeerSession {
    peer_id: String,
    session_id: String,
    tx: mpsc::Sender<ServerMessage>,
    joined_at: SystemTime,
}

impl PeerSession {
    fn new(peer_id: String, session_id: String, tx: mpsc::Sender<ServerMessage>) -> Self {
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
struct RendezvousSession {
    session_id: String,
    host_id: String,
    peers: DashMap<String, PeerSession>,
    created_at: SystemTime,
}

impl RendezvousSession {
    fn new(session_id: String, host_id: String) -> Self {
        Self {
            session_id,
            host_id,
            peers: DashMap::new(),
            created_at: SystemTime::now(),
        }
    }

    fn add_peer(&self, peer: PeerSession) {
        self.peers.insert(peer.peer_id.clone(), peer);
    }

    fn remove_peer(&self, peer_id: &str) -> Option<PeerSession> {
        self.peers.remove(peer_id).map(|(_, p)| p)
    }

    fn get_peer(&self, peer_id: &str) -> Option<PeerSession> {
        self.peers.get(peer_id).map(|p| p.clone())
    }

    fn list_peers(&self) -> Vec<PeerSession> {
        self.peers.iter().map(|p| p.value().clone()).collect()
    }

    fn peer_count(&self) -> usize {
        self.peers.len()
    }
}

