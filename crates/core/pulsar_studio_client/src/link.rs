//! The background tasks of one WebSocket connection: the writer, the pinger
//! and the reader, for both the relay protocol (`MultiuserClient::connect`)
//! and the Studio session protocol (`MultiuserClient::connect_to_workspace`).
//!
//! [`RelayTask`] and [`StudioTask`] run on the client's Tokio runtime; they
//! report the outcome of the handshake through `result_tx` and then serve the
//! socket until it closes or the shutdown signal fires.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use async_tungstenite::{
    tokio::{connect_async, ConnectStream},
    tungstenite::Message,
    WebSocketReceiver, WebSocketSender, WebSocketStream,
};
use futures::StreamExt;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch, RwLock};
use tracing::{debug, error, info, warn};

use crate::{ClientMessage, ConnectionStatus, PeerIdentity, ServerMessage, StudioWsMessage};

type WsSender = WebSocketSender<ConnectStream>;
type WsReceiver = WebSocketReceiver<ConnectStream>;

/// The client state every task of a connection updates.
#[derive(Clone)]
pub(crate) struct LinkShared {
    pub status: Arc<RwLock<ConnectionStatus>>,
    pub latency_ms: Arc<RwLock<Option<u32>>>,
    pub last_ping_at: Arc<RwLock<Option<Instant>>>,
}

/// Serialise `msg` and send it as a text frame. `false` means the socket
/// failed (the status is set to the error) and the writer should stop.
async fn send_json<T: Serialize>(
    write: &mut WsSender,
    msg: &T,
    status: &RwLock<ConnectionStatus>,
) -> bool {
    if let Ok(json) = serde_json::to_string(msg) {
        if let Err(e) = write.send(Message::Text(json.into())).await {
            error!("Failed to send message: {}", e);
            *status.write().await = ConnectionStatus::Error(e.to_string());
            return false;
        }
    }
    true
}

/// Queue a `Ping` every `interval` until the connection is shut down or
/// closed, stamping the send time so the reader can measure the round trip.
fn spawn_ping_task(
    shared: &LinkShared,
    mut shutdown_rx: watch::Receiver<bool>,
    ping_tx: mpsc::UnboundedSender<ClientMessage>,
    interval: Duration,
) {
    let status = shared.status.clone();
    let last_ping_at = shared.last_ping_at.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_ok() && *shutdown_rx.borrow() {
                        break;
                    }
                }
                _ = tokio::time::sleep(interval) => {
                    if matches!(*status.read().await, ConnectionStatus::Disconnected) {
                        break;
                    }

                    *last_ping_at.write().await = Some(Instant::now());
                    if ping_tx.send(ClientMessage::Ping).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

/// Record the round trip of a `Pong` against the last ping sent.
async fn record_pong(shared: &LinkShared) {
    if let Some(sent_at) = shared.last_ping_at.write().await.take() {
        let rtt = sent_at.elapsed().as_millis();
        *shared.latency_ms.write().await = Some(rtt as u32);
    }
}

async fn mark_closed(shared: &LinkShared) {
    *shared.status.write().await = ConnectionStatus::Disconnected;
    *shared.latency_ms.write().await = None;
}

async fn mark_failed(shared: &LinkShared, error: String) {
    *shared.status.write().await = ConnectionStatus::Error(error);
    *shared.latency_ms.write().await = None;
}

// ── Relay protocol ───────────────────────────────────────────────────────────

/// One connection to the relay server.
pub(crate) struct RelayTask {
    pub ws_url: String,
    pub shared: LinkShared,
    pub session_id: String,
    pub peer_id: String,
    pub join_token: String,
    pub identity: Option<PeerIdentity>,
    pub message_tx: mpsc::UnboundedSender<ClientMessage>,
    pub message_rx: mpsc::UnboundedReceiver<ClientMessage>,
    pub event_tx: mpsc::UnboundedSender<ServerMessage>,
    pub shutdown_rx: watch::Receiver<bool>,
    pub result_tx: oneshot::Sender<Result<()>>,
}

impl RelayTask {
    /// Connect (in the Tokio runtime context), then serve the socket.
    pub(crate) async fn run(self) {
        match connect_async(&self.ws_url).await {
            Ok((ws_stream, _)) => self.serve(ws_stream).await,
            Err(e) => {
                error!("Failed to connect to WebSocket: {}", e);
                mark_failed(&self.shared, e.to_string()).await;
                let _ = self
                    .result_tx
                    .send(Err(anyhow::anyhow!("Connection failed: {}", e)));
            }
        }
    }

    async fn serve(self, ws_stream: WebSocketStream<ConnectStream>) {
        let Self {
            ws_url: _,
            shared,
            session_id,
            peer_id,
            join_token,
            identity,
            message_tx,
            message_rx,
            event_tx,
            shutdown_rx,
            result_tx,
        } = self;
        info!("WebSocket connected successfully");

        let (mut write, read) = ws_stream.split();

        // Send initial Join message
        let join_msg = ClientMessage::Join {
            session_id,
            peer_id,
            join_token,
            display_name: identity.as_ref().and_then(|i| i.display_name.clone()),
            avatar_url: identity.as_ref().and_then(|i| i.avatar_url.clone()),
            github_login: identity.as_ref().and_then(|i| i.github_login.clone()),
        };

        if let Ok(join_json) = serde_json::to_string(&join_msg) {
            if let Err(e) = write.send(Message::Text(join_json.into())).await {
                error!("Failed to send join message: {}", e);
                let _ = result_tx.send(Err(anyhow::anyhow!("Failed to send join: {}", e)));
                return;
            }
        }

        // Signal successful connection
        let _ = result_tx.send(Ok(()));

        tokio::spawn(relay_writer(
            write,
            shared.status.clone(),
            shutdown_rx.clone(),
            message_rx,
        ));
        spawn_ping_task(
            &shared,
            shutdown_rx.clone(),
            message_tx.clone(),
            Duration::from_secs(3),
        );
        relay_reader(read, &shared, &event_tx, shutdown_rx.clone()).await;
    }
}

/// Send the client's queued messages until shutdown or the queue closes.
async fn relay_writer(
    mut write: WsSender,
    status: Arc<RwLock<ConnectionStatus>>,
    mut shutdown_rx: watch::Receiver<bool>,
    mut message_rx: mpsc::UnboundedReceiver<ClientMessage>,
) {
    loop {
        tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_ok() && *shutdown_rx.borrow() {
                    let _ = write.send(Message::Close(None)).await;
                    break;
                }
            }
            maybe_msg = message_rx.recv() => {
                match maybe_msg {
                    Some(msg) => {
                        if !send_json(&mut write, &msg, &status).await {
                            break;
                        }
                    }
                    None => {
                        let _ = write.send(Message::Close(None)).await;
                        break;
                    }
                }
            }
        }
    }
}

/// Handle incoming messages until shutdown, close, error or a dropped event
/// receiver.
async fn relay_reader(
    mut read: WsReceiver,
    shared: &LinkShared,
    event_tx: &mpsc::UnboundedSender<ServerMessage>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_ok() && *shutdown_rx.borrow() {
                    break;
                }
            }
            result = read.next() => {
                match result {
                    Some(Ok(Message::Text(text))) => {
                        if !on_relay_text(&text, shared, event_tx).await {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        info!("WebSocket closed by server");
                        mark_closed(shared).await;
                        break;
                    }
                    Some(Ok(Message::Ping(_))) => {
                        // Pings are handled automatically by tungstenite
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        error!("WebSocket error: {}", e);
                        mark_failed(shared, e.to_string()).await;
                        break;
                    }
                    None => break,
                }
            }
        }
    }
}

/// Forward one relay message to the event receiver. `false` when the loop
/// should end.
async fn on_relay_text(
    text: &str,
    shared: &LinkShared,
    event_tx: &mpsc::UnboundedSender<ServerMessage>,
) -> bool {
    match serde_json::from_str::<ServerMessage>(text) {
        Ok(msg) => {
            debug!("Received message: {:?}", msg);

            if matches!(&msg, ServerMessage::Pong) {
                record_pong(shared).await;
            }

            // Update status on successful join
            if matches!(msg, ServerMessage::Joined { .. }) {
                *shared.status.write().await = ConnectionStatus::Connected;
            }

            if event_tx.send(msg).is_err() {
                warn!("Event receiver dropped");
                return false;
            }
        }
        Err(e) => {
            error!("Failed to parse server message: {}", e);
        }
    }
    true
}

// ── Studio session protocol ──────────────────────────────────────────────────

/// One connection to a `pulsar-studio` workspace session.
pub(crate) struct StudioTask {
    pub ws_url: String,
    pub shared: LinkShared,
    pub session_id: String,
    pub peer_id: String,
    pub username: String,
    pub message_tx: mpsc::UnboundedSender<ClientMessage>,
    pub message_rx: mpsc::UnboundedReceiver<ClientMessage>,
    pub event_tx: mpsc::UnboundedSender<ServerMessage>,
    pub shutdown_rx: watch::Receiver<bool>,
    pub result_tx: oneshot::Sender<Result<()>>,
}

impl StudioTask {
    /// Connect (in the Tokio runtime context), then serve the socket.
    pub(crate) async fn run(self) {
        match connect_async(&self.ws_url).await {
            Ok((ws_stream, _)) => self.serve(ws_stream).await,
            Err(e) => {
                error!("Failed to connect to Studio WebSocket: {}", e);
                mark_failed(&self.shared, e.to_string()).await;
                let _ = self
                    .result_tx
                    .send(Err(anyhow::anyhow!("Connection failed: {}", e)));
            }
        }
    }

    async fn serve(self, ws_stream: WebSocketStream<ConnectStream>) {
        let Self {
            ws_url: _,
            shared,
            session_id,
            peer_id,
            username,
            message_tx,
            message_rx,
            event_tx,
            shutdown_rx,
            result_tx,
        } = self;
        info!("Studio WebSocket connected successfully");

        let (write, read) = ws_stream.split();

        // ── Send initial presence ──
        // Studio protocol doesn't require an explicit Join message;
        // just connecting with query params establishes identity.

        // Signal successful connection
        let _ = result_tx.send(Ok(()));

        // The writer maps ClientMessage → StudioWsMessage; `raw_tx` lets the
        // reader queue protocol replies (a Pong) directly.
        let (raw_tx, raw_rx) = mpsc::unbounded_channel::<StudioWsMessage>();
        tokio::spawn(
            StudioWriter {
                write,
                status: shared.status.clone(),
                shutdown_rx: shutdown_rx.clone(),
                message_rx,
                raw_rx,
                username,
            }
            .run(),
        );
        spawn_ping_task(
            &shared,
            shutdown_rx.clone(),
            message_tx.clone(),
            Duration::from_secs(15),
        );

        // ── Handle incoming messages (StudioWsMessage → ServerMessage) ──
        StudioReader {
            shared,
            event_tx,
            raw_tx,
            session_id,
            peer_id,
            first_message: true,
        }
        .run(read, shutdown_rx.clone())
        .await;
    }
}

/// Sends client messages as Studio protocol messages.
struct StudioWriter {
    write: WsSender,
    status: Arc<RwLock<ConnectionStatus>>,
    shutdown_rx: watch::Receiver<bool>,
    message_rx: mpsc::UnboundedReceiver<ClientMessage>,
    raw_rx: mpsc::UnboundedReceiver<StudioWsMessage>,
    username: String,
}

impl StudioWriter {
    async fn run(mut self) {
        loop {
            tokio::select! {
                changed = self.shutdown_rx.changed() => {
                    if changed.is_ok() && *self.shutdown_rx.borrow() {
                        let _ = self.write.send(Message::Close(None)).await;
                        break;
                    }
                }
                maybe_raw = self.raw_rx.recv() => {
                    match maybe_raw {
                        Some(ws_msg) => {
                            if !send_json(&mut self.write, &ws_msg, &self.status).await {
                                break;
                            }
                        }
                        None => {
                            let _ = self.write.send(Message::Close(None)).await;
                            break;
                        }
                    }
                }
                maybe_msg = self.message_rx.recv() => {
                    match maybe_msg {
                        Some(msg) => {
                            let ws_msg = match msg {
                                ClientMessage::ChatMessage { message, .. } => {
                                    StudioWsMessage::Chat {
                                        user: self.username.clone(),
                                        text: message,
                                    }
                                }
                                ClientMessage::Ping => StudioWsMessage::Ping,
                                _ => continue,
                            };
                            if !send_json(&mut self.write, &ws_msg, &self.status).await {
                                break;
                            }
                        }
                        None => {
                            let _ = self.write.send(Message::Close(None)).await;
                            break;
                        }
                    }
                }
            }
        }
    }
}

/// Turns Studio protocol messages into the client's `ServerMessage` events.
struct StudioReader {
    shared: LinkShared,
    event_tx: mpsc::UnboundedSender<ServerMessage>,
    raw_tx: mpsc::UnboundedSender<StudioWsMessage>,
    session_id: String,
    peer_id: String,
    /// The first user list is the join confirmation.
    first_message: bool,
}

impl StudioReader {
    async fn run(mut self, mut read: WsReceiver, mut shutdown_rx: watch::Receiver<bool>) {
        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_ok() && *shutdown_rx.borrow() {
                        break;
                    }
                }
                result = read.next() => {
                    match result {
                        Some(Ok(Message::Text(text))) => {
                            if !self.on_text(&text).await {
                                break;
                            }
                        }
                        Some(Ok(Message::Close(_))) => {
                            info!("Studio WebSocket closed by server");
                            mark_closed(&self.shared).await;
                            break;
                        }
                        Some(Ok(Message::Ping(_))) => {}
                        Some(Ok(_)) => {}
                        Some(Err(e)) => {
                            error!("Studio WebSocket error: {}", e);
                            mark_failed(&self.shared, e.to_string()).await;
                            break;
                        }
                        None => break,
                    }
                }
            }
        }
    }

    /// `false` when the loop should end.
    async fn on_text(&mut self, text: &str) -> bool {
        match serde_json::from_str::<StudioWsMessage>(text) {
            Ok(studio_msg) => {
                debug!("Received Studio message: {:?}", studio_msg);
                self.on_message(studio_msg).await
            }
            Err(e) => {
                error!("Failed to parse Studio message: {}", e);
                true
            }
        }
    }

    /// `false` when the loop should end.
    async fn on_message(&mut self, studio_msg: StudioWsMessage) -> bool {
        match studio_msg {
            StudioWsMessage::Pong => record_pong(&self.shared).await,
            StudioWsMessage::UserList { users } => {
                if self.first_message {
                    self.first_message = false;
                    *self.shared.status.write().await = ConnectionStatus::Connected;
                    let _ = self.event_tx.send(ServerMessage::Joined {
                        session_id: self.session_id.clone(),
                        peer_id: self.peer_id.clone(),
                        participants: users,
                        join_token: None,
                        participant_profiles: None,
                    });
                }
            }
            StudioWsMessage::UserJoined { user } => {
                let _ = self.event_tx.send(ServerMessage::PeerJoined {
                    session_id: self.session_id.clone(),
                    peer_id: user,
                    profile: None,
                });
            }
            StudioWsMessage::UserLeft { user } => {
                let _ = self.event_tx.send(ServerMessage::PeerLeft {
                    session_id: self.session_id.clone(),
                    peer_id: user,
                });
            }
            StudioWsMessage::Chat { user, text } => {
                let _ = self.event_tx.send(ServerMessage::ChatMessage {
                    session_id: self.session_id.clone(),
                    peer_id: user,
                    message: text,
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                });
            }
            StudioWsMessage::StatePatch { patch } => {
                let data = patch.to_string();
                let _ = self.event_tx.send(ServerMessage::ReplicationUpdate {
                    session_id: self.session_id.clone(),
                    from_peer_id: "studio".to_string(),
                    data,
                });
            }
            StudioWsMessage::Error { message } => {
                error!("Studio session error: {}", message);
                *self.shared.status.write().await = ConnectionStatus::Error(message.clone());
                let _ = self.event_tx.send(ServerMessage::Error { message });
                return false;
            }
            StudioWsMessage::Ping => {
                // Respond with Pong
                let _ = self.raw_tx.send(StudioWsMessage::Pong);
            }
            StudioWsMessage::FileChanged { .. } => {
                // File changes are handled via HTTP file API; ignore for now
            }
        }
        true
    }
}
