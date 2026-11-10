//! Per-message handlers shared by the host and guest session event loops.
//!
//! Owns the pieces of `MultiplayerWindow` session handling that are identical
//! for both roles: wiring the replication bridge after `Joined`, keeping the
//! engine participant list in sync, applying chat / peer-left / profile updates
//! and forwarding replication messages. The role-specific flows live in
//! `peer_state` (guest, `join_session`) and `sync_protocol` (host,
//! `create_session`). The `tag` argument is the log prefix of the calling flow.

use gpui::*;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::net::multiuser::{ClientMessage, MultiuserClient, PeerProfile};
use crate::screen::MultiplayerWindow;
use crate::session_logic;
use crate::utils::types::*;
use engine_state::EngineContext;

/// Contents of the server's `Joined` reply that both session flows need.
pub(crate) struct JoinedReply {
    pub peer_id: String,
    pub participants: Vec<String>,
    pub server_join_token: Option<String>,
    pub participant_profiles: Option<Vec<PeerProfile>>,
}

impl MultiplayerWindow {
    /// Records a connection error in the UI status and the engine multiuser context.
    /// Callers are responsible for `cx.notify()`.
    pub(crate) fn set_connection_error(&mut self, text: String) {
        self.connection_status = ConnectionStatus::Error(text.clone());
        self.sync_engine_multiuser_error(text);
    }

    /// Finishes setup after the server's `Joined` reply: presence, replication
    /// bridge, engine sync and the filesystem event forwarder.
    pub(crate) fn finish_joined_setup(
        &mut self,
        tag: &str,
        client: &Arc<RwLock<MultiuserClient>>,
        server_address: &str,
        session_id: &str,
        joined: &JoinedReply,
        cx: &mut Context<Self>,
    ) {
        let peer_id = joined.peer_id.as_str();
        let participants = joined.participants.as_slice();

        // Initialize presence for all participants
        self.update_presence_from_participants(cx);

        // Log project root
        if let Some(project_root) = &self.project_root {
            tracing::debug!("{}: Project root at {:?}", tag, project_root);
        }

        self.setup_replication_bridge(client, session_id, peer_id, participants, cx);

        tracing::debug!("{}: Initialized replication bridge", tag);
        self.sync_engine_multiuser_connected(server_address, session_id, peer_id, participants);
        self.start_fs_event_forwarder(
            client.clone(),
            session_id.to_string(),
            peer_id.to_string(),
            cx,
        );
        if let Some(profiles) = joined.participant_profiles.clone() {
            self.sync_engine_multiuser_profiles(profiles);
        }

        cx.notify();
    }

    /// Initializes UI replication, routes outgoing replication messages to the
    /// server and registers every current participant as a user.
    fn setup_replication_bridge(
        &mut self,
        client: &Arc<RwLock<MultiuserClient>>,
        session_id: &str,
        peer_id: &str,
        participants: &[String],
        cx: &mut Context<Self>,
    ) {
        // Initialize UI replication system
        ui::replication::init(cx);

        // Set up replication bridge
        let integration = ui::replication::MultiuserIntegration::new(cx);
        let host_peer_id = session_logic::host_peer_id_or(participants, peer_id);

        // Set up message sender
        let client_clone = client.clone();
        let session_id_clone = session_id.to_string();
        let peer_id_clone = peer_id.to_string();
        integration.start_session(
            peer_id.to_string(),
            host_peer_id,
            move |replication_msg| {
                let client = client_clone.clone();
                let session_id = session_id_clone.clone();
                let peer_id = peer_id_clone.clone();
                if let Ok(data) = serde_json::to_string(&replication_msg) {
                    tokio::spawn(async move {
                        let client_guard = client.read().await;
                        let _ = client_guard
                            .send(ClientMessage::ReplicationUpdate {
                                session_id,
                                peer_id,
                                data,
                            })
                            .await;
                    });
                }
            },
            cx,
        );

        // Add user presence for all participants
        for (i, participant_id) in participants.iter().enumerate() {
            let color = Self::generate_user_color(participant_id);
            integration.add_user(
                participant_id.clone(),
                session_logic::user_label(i + 1),
                color,
                cx,
            );
        }
    }

    /// Pushes the active session's participant list to the engine multiuser context.
    pub(crate) fn resync_engine_participants(&self) {
        let session_snapshot = self.active_session.as_ref().map(|s| {
            (
                s.server_address.clone(),
                s.session_id.clone(),
                s.connected_users.clone(),
            )
        });
        if let (Some(peer_id), Some((server_address, session_id, participants))) =
            (self.current_peer_id.as_ref(), session_snapshot)
        {
            self.sync_engine_multiuser_connected(
                &server_address,
                &session_id,
                peer_id,
                &participants,
            );
        }
    }

    /// Merges a peer's profile into the engine multiuser context.
    pub(crate) fn merge_engine_peer_profile(profile: &PeerProfile) {
        if let Some(engine) = EngineContext::global() {
            let _ = engine.update_multiuser(|mu| {
                session_logic::upsert_profile(&mut mu.participant_profiles, profile);
            });
            engine.notify_multiuser_changed();
        }
    }

    /// Applies a `PeerLeft` message to session, presence, replication and engine state.
    pub(crate) fn apply_peer_left(&mut self, left_peer_id: &str, cx: &mut Context<Self>) {
        if let Some(session) = &mut self.active_session {
            session_logic::remove_peer(&mut session.connected_users, left_peer_id);
            self.user_presences.retain(|p| p.peer_id != left_peer_id);
            // Remove from replication system
            let integration = ui::replication::MultiuserIntegration::new(cx);
            integration.remove_user(left_peer_id, cx);
            cx.notify();
        }

        self.resync_engine_participants();
        if let Some(engine) = EngineContext::global() {
            let _ = engine.update_multiuser(|mu| {
                session_logic::remove_profile(&mut mu.participant_profiles, left_peer_id);
            });
            engine.notify_multiuser_changed();
        }
    }

    /// Appends an incoming chat message to the chat log.
    pub(crate) fn apply_chat_message(
        &mut self,
        tag: &str,
        sender_peer_id: String,
        message: String,
        timestamp: u64,
        cx: &mut Context<Self>,
    ) {
        let is_self = session_logic::is_self_peer(self.current_peer_id.as_deref(), &sender_peer_id);
        tracing::debug!(
            "{}: Adding chat message. is_self: {}, current chat count: {}",
            tag,
            is_self,
            self.chat_messages.len()
        );
        self.chat_messages.push(ChatMessage {
            peer_id: sender_peer_id,
            message,
            timestamp,
            is_self,
        });
        tracing::debug!("{}: Chat messages now: {}", tag, self.chat_messages.len());
        cx.notify();
    }

    /// Re-emits a remote file change on the local engine filesystem event bus.
    pub(crate) fn emit_remote_file_change(path: String, kind: &str) {
        engine_fs::events::emit_remote(path, session_logic::fs_change_kind(kind));
    }

    /// Feeds an incoming replication message to the UI replication system and
    /// sends back its response, if any.
    pub(crate) async fn forward_replication_update(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        client: &Arc<RwLock<MultiuserClient>>,
        data: &str,
    ) {
        // Handle replication message and get response
        let response = if let Ok(rep_msg) = serde_json::from_str(data) {
            cx.update(|cx| {
                let integration = ui::replication::MultiuserIntegration::new(cx);
                integration.handle_incoming_message(rep_msg, cx)
            })
        } else {
            None
        };

        // Send response if we got one
        if let Some(response) = response {
            let (session_id_result, our_peer_id_result) = cx
                .update(|cx| {
                    this.update(cx, |this, _cx| {
                        let session_id = this.active_session.as_ref().map(|s| s.session_id.clone());
                        let peer_id = this.current_peer_id.clone();
                        (session_id, peer_id)
                    })
                })
                .unwrap_or((None, None));

            if let (Some(session_id), Some(our_peer_id)) = (session_id_result, our_peer_id_result) {
                if let Ok(response_data) = serde_json::to_string(&response) {
                    let client_guard = client.read().await;
                    let _ = client_guard
                        .send(ClientMessage::ReplicationUpdate {
                            session_id,
                            peer_id: our_peer_id,
                            data: response_data,
                        })
                        .await;
                }
            }
        }
    }

    /// Reads the local project root from the window, if one is set.
    pub(crate) fn read_project_root(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
    ) -> Option<std::path::PathBuf> {
        cx.update(|cx| this.update(cx, |this, _cx| this.project_root.clone()))
            .ok()
            .flatten()
    }

    /// Reads our own peer id from the window, if connected.
    pub(crate) fn read_own_peer_id(this: &WeakEntity<Self>, cx: &mut AsyncApp) -> Option<String> {
        cx.update(|cx| this.update(cx, |this, _cx| this.current_peer_id.clone()))
            .ok()
            .flatten()
    }

    /// True while the window still has an active session (the event loops stop otherwise).
    pub(crate) fn session_still_active(this: &WeakEntity<Self>, cx: &mut AsyncApp) -> bool {
        cx.update(|cx| this.update(cx, |this, _cx| this.active_session.is_some()))
            .unwrap_or(false)
    }

    /// Reports the current connection latency to the engine after each handled message.
    pub(crate) async fn report_latency(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        client: &Arc<RwLock<MultiuserClient>>,
    ) {
        let latency_ms = {
            let client_guard = client.read().await;
            client_guard.latency_ms().await
        };
        cx.update(|cx| {
            this.update(cx, |this, _| {
                this.sync_engine_multiuser_latency(latency_ms);
            });
        });
    }
}
