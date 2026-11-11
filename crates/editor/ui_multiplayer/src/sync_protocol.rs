//! Host connection flow: creating a new multiplayer session.
//!
//! Owns `MultiplayerWindow::create_session` (the entry point, called from the
//! create form) and the named steps it is built from: validate input, create the
//! session over HTTP, connect, wait for the `Joined` reply, apply it to UI state
//! and run the event loop that dispatches each `ServerMessage` to a
//! `handle_host_*` function. Pure logic lives in `session_logic`; handlers
//! shared with the guest flow live in `session_events`.

use gpui::*;
use std::ops::ControlFlow;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::RwLock;

use crate::net::multiuser::{
    ClientMessage, MultiuserClient, PeerIdentity, PeerProfile, ServerMessage,
};
use crate::net::simple_sync;
use crate::screen::MultiplayerWindow;
use crate::session_events::JoinedReply;
use crate::session_logic;
use crate::utils::types::*;

/// Log prefix used by every message in the host flow.
const TAG: &str = "CREATE_SESSION";

impl MultiplayerWindow {
    /// Validates the form, creates the client and spawns the session-creation task.
    pub(super) fn create_session(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let server_address = self.server_address_input.read(cx).text().to_string();

        if let Err(message) = session_logic::validate_server_address(&server_address) {
            self.connection_status = ConnectionStatus::Error(message.to_string());
            cx.notify();
            return;
        }

        self.connection_status = ConnectionStatus::Connecting;
        cx.notify();

        // Create client
        let client = Arc::new(RwLock::new(MultiuserClient::new(server_address.clone())));
        self.client = Some(client.clone());

        let identity = self.current_peer_identity();

        cx.spawn(async move |this, cx| {
            Self::run_host_flow(&this, cx, client, server_address, identity).await;
        })
        .detach();
    }

    /// Creates the session on the server, then connects to it as the host.
    async fn run_host_flow(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        client: Arc<RwLock<MultiuserClient>>,
        server_address: String,
        identity: Option<PeerIdentity>,
    ) {
        // Call create_session on the client
        let result = {
            let client_guard = client.read().await;
            client_guard.create_session().await
        };

        let (session_id, join_token) = match result {
            Ok(credentials) => credentials,
            Err(e) => {
                Self::report_host_error(
                    this,
                    cx,
                    format!("Failed to create session: {}", e),
                    false,
                );
                return;
            }
        };

        // Update UI state - still connecting, not yet fully connected
        cx.update(|cx| {
            this.update(cx, |this, cx| {
                this.apply_host_session_created(&server_address, &session_id, &join_token, cx);
            });
        });

        // Now connect via WebSocket
        let event_rx_result = {
            let mut client_guard = client.write().await;
            client_guard
                .connect(session_id.clone(), join_token, identity)
                .await
        }; // client_guard dropped here, releasing the lock

        let mut event_rx = match event_rx_result {
            Ok(event_rx) => event_rx,
            Err(e) => {
                Self::report_host_error(this, cx, session_logic::connection_failed_text(e), false);
                return;
            }
        };

        // Wait for the initial response (Joined or Error)
        match event_rx.recv().await {
            Some(ServerMessage::Joined {
                peer_id,
                participants,
                join_token,
                participant_profiles,
                ..
            }) => {
                let joined = JoinedReply {
                    peer_id,
                    participants,
                    server_join_token: join_token,
                    participant_profiles,
                };
                Self::run_hosted_session(
                    this,
                    cx,
                    &client,
                    &mut event_rx,
                    &server_address,
                    &session_id,
                    &joined,
                )
                .await;
            }
            Some(ServerMessage::Error { message }) => {
                let text = session_logic::server_error_text(&message);
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.set_connection_error(text);
                        this.active_session = None;
                        cx.notify();
                    });
                });
            }
            Some(_) => {
                Self::report_host_error(this, cx, "Unexpected server response".to_string(), true);
            }
            None => {
                Self::report_host_error(
                    this,
                    cx,
                    "Connection closed before response".to_string(),
                    true,
                );
            }
        }
    }

    /// Shows a host-flow failure; `clear_session` also stops the fs forwarder and drops the session.
    fn report_host_error(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        text: String,
        clear_session: bool,
    ) {
        cx.update(|cx| {
            this.update(cx, |this, cx| {
                this.set_connection_error(text);
                if clear_session {
                    this.fs_event_forwarder = None;
                    this.active_session = None;
                }
                cx.notify();
            });
        });
    }

    /// Records the freshly created session (still connecting) in UI and engine state.
    fn apply_host_session_created(
        &mut self,
        server_address: &str,
        session_id: &str,
        join_token: &str,
        cx: &mut Context<Self>,
    ) {
        self.connection_status = ConnectionStatus::Connecting;
        self.active_session = Some(ActiveSession {
            session_id: session_id.to_string(),
            join_token: join_token.to_string(),
            server_address: server_address.to_string(),
            connected_users: vec!["You (Host)".to_string()],
        });
        self.sync_engine_multiuser_connecting(server_address, session_id);
        cx.notify();
    }

    /// Applies the `Joined` reply, then pumps events until the session ends.
    async fn run_hosted_session(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        client: &Arc<RwLock<MultiuserClient>>,
        event_rx: &mut UnboundedReceiver<ServerMessage>,
        server_address: &str,
        session_id: &str,
        joined: &JoinedReply,
    ) {
        cx.update(|cx| {
            this.update(cx, |this, cx| {
                this.apply_host_joined(client, server_address, session_id, joined, cx);
            });
        });

        // Continue listening for PeerJoined/PeerLeft events
        Self::run_host_event_loop(this, cx, client, event_rx).await;
    }

    /// Stores our peer id and participants, then finishes setup (presence, replication, engine sync).
    fn apply_host_joined(
        &mut self,
        client: &Arc<RwLock<MultiuserClient>>,
        server_address: &str,
        session_id: &str,
        joined: &JoinedReply,
        cx: &mut Context<Self>,
    ) {
        // Store our peer_id first
        self.current_peer_id = Some(joined.peer_id.clone());

        tracing::debug!(
            "{}: Received Joined - our peer_id: {}, participants: {:?}",
            TAG,
            joined.peer_id,
            joined.participants
        );

        if let Some(session) = &mut self.active_session {
            // Store raw participant list
            session.connected_users = joined.participants.clone();
            if let Some(join_token) = joined.server_join_token.clone() {
                session.join_token = join_token;
            }
        }

        self.finish_joined_setup(TAG, client, server_address, session_id, joined, cx);
    }

    /// Handles server messages until the session ends, we are kicked or the channel closes.
    async fn run_host_event_loop(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        client: &Arc<RwLock<MultiuserClient>>,
        event_rx: &mut UnboundedReceiver<ServerMessage>,
    ) {
        while let Some(msg) = event_rx.recv().await {
            // Check if we're still connected - break if disconnected
            if !Self::session_still_active(this, cx) {
                tracing::debug!("{}: Session disconnected, stopping event loop", TAG);
                break;
            }

            if Self::handle_host_message(this, cx, client, msg)
                .await
                .is_break()
            {
                break;
            }

            Self::report_latency(this, cx, client).await;
        }
    }

    /// Dispatches one server message; `Break` ends the event loop.
    async fn handle_host_message(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        client: &Arc<RwLock<MultiuserClient>>,
        msg: ServerMessage,
    ) -> ControlFlow<()> {
        match msg {
            ServerMessage::PeerJoined {
                peer_id: joined_peer_id,
                profile,
                ..
            } => {
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.apply_host_peer_joined(joined_peer_id, profile, cx);
                    });
                });
            }
            ServerMessage::PeerLeft {
                peer_id: left_peer_id,
                ..
            } => {
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.apply_peer_left(&left_peer_id, cx);
                    });
                });
            }
            ServerMessage::Kicked { reason, .. } => {
                tracing::warn!("{}: You were kicked from the session: {}", TAG, reason);
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.handle_kicked(format!("Kicked from session: {}", reason), cx);
                    });
                });
                return ControlFlow::Break(()); // Exit the message loop
            }
            ServerMessage::ChatMessage {
                peer_id: sender_peer_id,
                message,
                timestamp,
                ..
            } => {
                tracing::debug!(
                    "{}: Received ChatMessage from {} at {}: {}",
                    TAG,
                    sender_peer_id,
                    timestamp,
                    message
                );
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.apply_chat_message(TAG, sender_peer_id, message, timestamp, cx);
                    });
                });
            }
            ServerMessage::Error { message } => {
                let text = session_logic::server_error_text(&message);
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.set_connection_error(text);
                        this.fs_event_forwarder = None;
                        cx.notify();
                    });
                });
            }
            ServerMessage::FileChanged { path, kind, .. } => {
                Self::emit_remote_file_change(path, &kind);
            }
            ServerMessage::RequestFileManifest {
                from_peer_id,
                session_id: req_session_id,
                ..
            } => {
                tracing::debug!(
                    "{}: Received RequestFileManifest from {}",
                    TAG,
                    from_peer_id
                );
                Self::host_serve_manifest(this, cx, client, req_session_id);
            }
            ServerMessage::RequestFiles {
                from_peer_id,
                file_paths,
                session_id: req_session_id,
                ..
            } => {
                tracing::debug!(
                    "{}: Received RequestFiles for {} files from {}",
                    TAG,
                    file_paths.len(),
                    from_peer_id
                );
                Self::host_serve_files(this, cx, client, req_session_id, file_paths);
            }
            ServerMessage::RequestFile {
                from_peer_id,
                file_path,
                session_id: req_session_id,
                ..
            } => {
                tracing::debug!(
                    "{}: Received RequestFile for {} from {}",
                    TAG,
                    file_path,
                    from_peer_id
                );
                Self::host_serve_file(this, cx, client, req_session_id, file_path).await;
            }
            ServerMessage::ReplicationUpdate {
                from_peer_id, data, ..
            } => {
                tracing::debug!("{}: Received ReplicationUpdate from {}", TAG, from_peer_id);
                Self::forward_replication_update(this, cx, client, &data).await;
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }

    /// Adds a newly joined peer to the session, presence, replication system and engine state.
    fn apply_host_peer_joined(
        &mut self,
        joined_peer_id: String,
        profile: Option<PeerProfile>,
        cx: &mut Context<Self>,
    ) {
        tracing::debug!(
            "{}: Received PeerJoined - joined_peer_id: {}, our peer_id: {:?}",
            TAG,
            joined_peer_id,
            self.current_peer_id
        );

        // Ignore PeerJoined about ourselves
        if session_logic::is_self_peer(self.current_peer_id.as_deref(), &joined_peer_id) {
            tracing::debug!("{}: Ignoring PeerJoined about ourselves", TAG);
            return;
        }

        if let Some(session) = &mut self.active_session {
            // Add raw peer_id if not already present
            if session_logic::add_peer(&mut session.connected_users, &joined_peer_id) {
                tracing::debug!(
                    "{}: Added peer {} to list. List now: {:?}",
                    TAG,
                    joined_peer_id,
                    session.connected_users
                );
            }
            let user_index = session.connected_users.len();

            // Update presence
            self.update_presence_from_participants(cx);
            // Add to replication system
            let integration = ui::replication::MultiuserIntegration::new(cx);
            let color = Self::generate_user_color(&joined_peer_id);
            integration.add_user(
                joined_peer_id.clone(),
                session_logic::user_label(user_index),
                color,
                cx,
            );
            cx.notify();
        } else {
            tracing::debug!("{}: Peer {} already in list", TAG, joined_peer_id);
        }

        self.resync_engine_participants();
        if let Some(profile) = profile {
            Self::merge_engine_peer_profile(&profile);
        }
    }

    /// Builds the project manifest on a background thread and sends it to the requester.
    fn host_serve_manifest(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        client: &Arc<RwLock<MultiuserClient>>,
        req_session_id: String,
    ) {
        // Create and send file manifest
        let Some(project_root) = Self::read_project_root(this, cx) else {
            tracing::error!("{}: No project_root available", TAG);
            return;
        };
        tracing::debug!("{}: Project root is {:?}", TAG, project_root.display());

        let Some(our_peer_id) = Self::read_own_peer_id(this, cx) else {
            tracing::error!("{}: No peer_id available", TAG);
            return;
        };

        // Create manifest in background thread
        let client_clone = client.clone();
        let session_id_clone = req_session_id.clone();
        std::thread::spawn(move || {
            tracing::debug!("HOST: Creating file manifest for {:?}", project_root);
            match simple_sync::create_manifest(&project_root) {
                Ok(manifest) => {
                    tracing::debug!("HOST: Created manifest with {} files", manifest.files.len());
                    match serde_json::to_string(&manifest) {
                        Ok(manifest_json) => {
                            tracing::debug!(
                                "HOST: Serialized manifest to {} bytes",
                                manifest_json.len()
                            );
                            tokio::runtime::Runtime::new().unwrap().block_on(async {
                                let client_guard = client_clone.read().await;
                                let _ = client_guard
                                    .send(ClientMessage::FileManifest {
                                        session_id: session_id_clone,
                                        peer_id: our_peer_id,
                                        manifest_json,
                                    })
                                    .await;
                                tracing::debug!("HOST: Sent file manifest");
                            });
                        }
                        Err(e) => tracing::error!("HOST: Failed to serialize manifest: {}", e),
                    }
                }
                Err(e) => tracing::error!("HOST: Failed to create manifest: {}", e),
            }
        });
    }

    /// Reads the requested files on a background thread and sends them as one `FilesChunk`.
    fn host_serve_files(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        client: &Arc<RwLock<MultiuserClient>>,
        req_session_id: String,
        file_paths: Vec<String>,
    ) {
        // Read and send requested files
        let Some(project_root) = Self::read_project_root(this, cx) else {
            return;
        };
        let Some(our_peer_id) = Self::read_own_peer_id(this, cx) else {
            return;
        };

        // Read files in background thread
        let client_clone = client.clone();
        let session_id_clone = req_session_id.clone();
        std::thread::spawn(move || {
            tracing::debug!("HOST: Reading {} files", file_paths.len());
            match simple_sync::read_files(&project_root, file_paths.clone()) {
                Ok(files_data) => {
                    // Serialize files data
                    match serde_json::to_string(&files_data) {
                        Ok(files_json) => {
                            tokio::runtime::Runtime::new().unwrap().block_on(async {
                                let client_guard = client_clone.read().await;
                                let _ = client_guard
                                    .send(ClientMessage::FilesChunk {
                                        session_id: session_id_clone,
                                        peer_id: our_peer_id,
                                        files_json,
                                        chunk_index: 0,
                                        total_chunks: 1,
                                    })
                                    .await;
                                tracing::debug!("HOST: Sent {} files", file_paths.len());
                            });
                        }
                        Err(e) => tracing::error!("HOST: Failed to serialize files: {}", e),
                    }
                }
                Err(e) => tracing::error!("HOST: Failed to read files: {}", e),
            }
        });
    }

    /// Reads one file and sends it to the requester in `FILE_CHUNK_SIZE` chunks.
    async fn host_serve_file(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        client: &Arc<RwLock<MultiuserClient>>,
        req_session_id: String,
        file_path: String,
    ) {
        // Read and send file in chunks
        let Some(project_root) = Self::read_project_root(this, cx) else {
            return;
        };
        let full_path = match simple_sync::project_relative(&project_root, &file_path) {
            Ok(path) => path,
            Err(error) => {
                tracing::warn!("{}: Refusing file request: {}", TAG, error);
                return;
            }
        };

        let Ok(data) = std::fs::read(&full_path) else {
            return;
        };
        let Some(our_peer_id) = Self::read_own_peer_id(this, cx) else {
            return;
        };

        for (i, chunk) in data.chunks(session_logic::FILE_CHUNK_SIZE).enumerate() {
            let offset = i * session_logic::FILE_CHUNK_SIZE;
            let is_last = session_logic::is_last_chunk(offset, chunk.len(), data.len());

            let client_guard = client.read().await;
            let _ = client_guard
                .send(ClientMessage::FileChunk {
                    session_id: req_session_id.clone(),
                    peer_id: our_peer_id.clone(),
                    file_path: file_path.clone(),
                    offset: offset as u64,
                    data: chunk.to_vec(),
                    is_last,
                })
                .await;
        }
        tracing::debug!("{}: Sent file {}", TAG, file_path);
    }
}
