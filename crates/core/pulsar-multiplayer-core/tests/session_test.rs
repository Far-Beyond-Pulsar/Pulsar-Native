use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use pulsar_multiplayer_core::protocol::*;
use pulsar_multiplayer_core::session::*;
use pulsar_multiplayer_core::transport::{SessionChannel, SessionError};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct TestChannel {
    tx: mpsc::UnboundedSender<SessionMessage>,
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<SessionMessage>>,
}

impl TestChannel {
    fn new() -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            tx,
            rx: tokio::sync::Mutex::new(rx),
        })
    }
}

#[async_trait]
impl SessionChannel for TestChannel {
    async fn send(&self, msg: SessionMessage) -> Result<(), SessionError> {
        self.tx
            .send(msg)
            .map_err(|_| SessionError::ConnectionClosed)
    }

    async fn recv(&self) -> Result<SessionMessage, SessionError> {
        self.rx
            .lock()
            .await
            .recv()
            .await
            .ok_or(SessionError::ConnectionClosed)
    }

    fn is_connected(&self) -> bool {
        true
    }

    async fn close(&self) -> Result<(), SessionError> {
        Ok(())
    }
}

fn make_session(host_id: &str, mode: SessionMode) -> SessionInfo {
    SessionInfo {
        id: "sess-integration".into(),
        host_id: host_id.into(),
        participants: vec![],
        created_at: 100,
        mode,
        metadata: HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// Full session lifecycle: join → message → leave
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_session_lifecycle() {
    let server = TestChannel::new();
    let session = make_session(
        "host-1",
        SessionMode::Hosted {
            server_url: "wss://pulsar.example.com".into(),
            project_id: "proj-42".into(),
        },
    );

    // Client joins
    let join_msg = SessionMessage::Join(JoinRequest {
        session_id: session.id.clone(),
        display_name: Some("Alice".into()),
        token: None,
    });
    server.send(join_msg).await.unwrap();

    let received_join = server.recv().await.unwrap();
    assert_eq!(received_join.kind(), "join");
    assert!(received_join.is_lifecycle());
    if let SessionMessage::Join(join) = received_join {
        assert_eq!(join.session_id, "sess-integration");
        assert_eq!(join.display_name, Some("Alice".into()));
    }

    // Server sends Joined response
    let joined_msg = SessionMessage::Joined(JoinedResponse {
        session: session.clone(),
        your_peer_id: "peer-alice".into(),
    });
    server.send(joined_msg).await.unwrap();

    let received_joined = server.recv().await.unwrap();
    assert_eq!(received_joined.kind(), "joined");
    if let SessionMessage::Joined(joined) = received_joined {
        assert_eq!(joined.your_peer_id, "peer-alice");
        assert_eq!(joined.session.id, "sess-integration");
        assert_eq!(joined.session.host_id, "host-1");
    }

    // Server broadcasts PeerJoined to other participants
    let peer_msg = SessionMessage::PeerJoined(PeerJoined {
        peer: ParticipantInfo {
            peer_id: "peer-alice".into(),
            role: Role::Editor,
            display_name: Some("Alice".into()),
            avatar_url: None,
            joined_at: 200,
            last_seen: 200,
        },
    });
    let second = TestChannel::new();
    second.send(peer_msg).await.unwrap();

    // Alice sends a chat message
    let chat_msg = SessionMessage::Chat(ChatMessage {
        sender_id: "peer-alice".into(),
        sender_name: Some("Alice".into()),
        text: "Hello from Alice!".into(),
        timestamp: 300,
    });
    server.send(chat_msg).await.unwrap();

    let received_chat = server.recv().await.unwrap();
    assert_eq!(received_chat.kind(), "chat");
    assert!(received_chat.is_presence());

    // Alice leaves
    let leave_msg = SessionMessage::Leave(LeaveRequest {
        reason: Some("done".into()),
    });
    server.send(leave_msg).await.unwrap();

    let received_leave = server.recv().await.unwrap();
    assert_eq!(received_leave.kind(), "leave");
    assert!(received_leave.is_lifecycle());

    // Server broadcasts PeerLeft
    let peer_left = SessionMessage::PeerLeft(PeerLeft {
        peer_id: "peer-alice".into(),
        reason: Some("disconnected".into()),
    });
    second.send(peer_left).await.unwrap();
}

// ---------------------------------------------------------------------------
// Multiple participants
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_multiple_participants() {
    let host = TestChannel::new();
    let alice = TestChannel::new();
    let bob = TestChannel::new();

    let participants = vec![
        ParticipantInfo {
            peer_id: "alice".into(),
            role: Role::Editor,
            display_name: Some("Alice".into()),
            avatar_url: None,
            joined_at: 100,
            last_seen: 100,
        },
        ParticipantInfo {
            peer_id: "bob".into(),
            role: Role::Observer,
            display_name: Some("Bob".into()),
            avatar_url: None,
            joined_at: 200,
            last_seen: 200,
        },
    ];

    let session = SessionInfo {
        id: "sess-multi".into(),
        host_id: "host-1".into(),
        participants: participants.clone(),
        created_at: 0,
        mode: SessionMode::Hosted {
            server_url: "wss://example.com".into(),
            project_id: "proj-1".into(),
        },
        metadata: HashMap::new(),
    };

    let joined = SessionMessage::Joined(JoinedResponse {
        session,
        your_peer_id: "alice".into(),
    });
    host.send(joined).await.unwrap();
    alice.send(SessionMessage::Ping).await.unwrap();
    bob.send(SessionMessage::Pong).await.unwrap();

    assert_eq!(participants.len(), 2);
    assert!(participants
        .iter()
        .any(|p| p.peer_id == "alice" && p.role.can_write()));
    assert!(participants
        .iter()
        .any(|p| p.peer_id == "bob" && !p.role.can_write()));

    let chat = SessionMessage::Chat(ChatMessage {
        sender_id: "bob".into(),
        sender_name: Some("Bob".into()),
        text: "Hi Alice!".into(),
        timestamp: 300,
    });
    bob.send(chat).await.unwrap();
}

// ---------------------------------------------------------------------------
// State replication flow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_state_replication_flow() {
    let host = TestChannel::new();

    let state_msg = SessionMessage::StateUpdate(StateUpdate {
        element_id: "transform-1".into(),
        state: json!({
            "position": {"x": 10.0, "y": 20.0, "z": 0.0},
            "rotation": 0.0,
            "scale": 1.0
        }),
        timestamp: 500,
    });
    host.send(state_msg).await.unwrap();

    let received_state = host.recv().await.unwrap();
    assert_eq!(received_state.kind(), "state_update");
    assert!(received_state.is_replication());
    if let SessionMessage::StateUpdate(update) = received_state {
        assert_eq!(update.element_id, "transform-1");
        assert_eq!(update.state["position"]["x"], 10.0);
        assert_eq!(update.timestamp, 500);
    }

    // Client requests lock
    let lock_req = SessionMessage::RequestLock(RequestLock {
        element_id: "transform-1".into(),
        peer_id: Some("client-1".into()),
    });
    host.send(lock_req).await.unwrap();

    let received_lock_req = host.recv().await.unwrap();
    assert_eq!(received_lock_req.kind(), "request_lock");
    if let SessionMessage::RequestLock(lr) = received_lock_req {
        assert_eq!(lr.element_id, "transform-1");
        assert_eq!(lr.peer_id, Some("client-1".into()));
    }

    // Host grants lock
    let lock_grant = SessionMessage::LockGranted(LockGranted {
        element_id: "transform-1".into(),
        peer_id: "client-1".into(),
    });
    host.send(lock_grant).await.unwrap();

    let received_grant = host.recv().await.unwrap();
    assert_eq!(received_grant.kind(), "lock_granted");
    if let SessionMessage::LockGranted(lg) = received_grant {
        assert_eq!(lg.peer_id, "client-1");
    }

    // Client releases lock
    let release = SessionMessage::ReleaseLock(ReleaseLock {
        element_id: "transform-1".into(),
    });
    host.send(release).await.unwrap();

    let received_release = host.recv().await.unwrap();
    assert_eq!(received_release.kind(), "release_lock");

    // Permission flow
    let perm_req = SessionMessage::RequestPermission(RequestPermission {
        element_id: "transform-1".into(),
        permission: "write".into(),
    });
    host.send(perm_req).await.unwrap();
    let _ = host.recv().await.unwrap();

    let perm_denied = SessionMessage::PermissionDenied(PermissionDenied {
        element_id: "transform-1".into(),
        permission: "admin".into(),
        reason: "insufficient role".into(),
    });
    host.send(perm_denied).await.unwrap();
    let received_denied = host.recv().await.unwrap();
    assert_eq!(received_denied.kind(), "permission_denied");
}

// ---------------------------------------------------------------------------
// P2P session mode
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_p2p_session_mode() {
    let session = make_session(
        "peer-host",
        SessionMode::P2P {
            relay_url: Some("wss://relay.pulsar.dev".into()),
        },
    );

    let json = serde_json::to_value(&session).unwrap();
    assert_eq!(json["mode"]["P2P"]["relay_url"], "wss://relay.pulsar.dev");

    // P2P signaling
    let chan = TestChannel::new();

    let conn_req = SessionMessage::P2pConnectionRequest(P2pConnectionRequest {
        session_id: "sess-p2p".into(),
        sdp: "v=0\no=...".into(),
        candidate: None,
    });
    chan.send(conn_req).await.unwrap();

    let received = chan.recv().await.unwrap();
    assert_eq!(received.kind(), "p2p_connection_request");
    if let SessionMessage::P2pConnectionRequest(req) = received {
        assert_eq!(req.session_id, "sess-p2p");
        assert_eq!(req.sdp, "v=0\no=...");
        assert!(req.candidate.is_none());
    }
}
