//! Guest connection flow: joining an existing multiplayer session.
//!
//! Owns `MultiplayerWindow::join_session` (the entry point, called from the join
//! form) and the named steps it is built from: validate input, connect, wait for
//! the `Joined` reply, apply it to UI state, request the host's file manifest,
//! then run the event loop that dispatches each `ServerMessage` to a
//! `handle_guest_*` function. Pure logic lives in `session_logic`; handlers
//! shared with the host flow live in `session_events`.

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

/// Log prefix used by every message in the guest flow.
const TAG: &str = "JOIN_SESSION";

impl MultiplayerWindow {
    /// Validates the join form, creates the client and spawns the connection task.
    pub(super) fn join_session(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let server_address = self.server_address_input.read(cx).text().to_string();
        let session_id = self.session_id_input.read(cx).text().to_string();
        let join_token = self.session_password_input.read(cx).text().to_string();

        if let Err(message) =
            session_logic::validate_join_input(&server_address, &session_id, &join_token)
        {
            self.connection_status = ConnectionStatus::Error(message.to_string());
            cx.notify();
            return;
        }

        self.connection_status = ConnectionStatus::Connecting;
        self.sync_engine_multiuser_connecting(&server_address, &session_id);
        cx.notify();

        // Create client
        let client = Arc::new(RwLock::new(MultiuserClient::new(server_address.clone())));
        self.client = Some(client.clone());
        let identity = self.current_peer_identity();

        cx.spawn(async move |this, cx| {
            Self::run_join_flow(
                &this,
                cx,
                client,
                server_address,
                session_id,
                join_token,
                identity,
            )
            .await;
        })
        .detach();
    }

    /// Connects, waits for the initial server reply and, on `Joined`, runs the session.
    async fn run_join_flow(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        client: Arc<RwLock<MultiuserClient>>,
        server_address: String,
        session_id: String,
        join_token: String,
        identity: Option<PeerIdentity>,
    ) {
        let event_rx_result = {
            let mut client_guard = client.write().await;
            client_guard
                .connect(session_id.clone(), join_token.clone(), identity)
                .await
        }; // client_guard dropped here, releasing the lock

        let mut event_rx = match event_rx_result {
            Ok(event_rx) => event_rx,
            Err(e) => {
                Self::report_join_error(this, cx, session_logic::connection_failed_text(e));
                return;
            }
        };

        // Wait for the initial response (Joined or Error)
        match event_rx.recv().await {
            Some(ServerMessage::Joined {
                peer_id,
                participants,
                join_token: server_join_token,
                participant_profiles,
                ..
            }) => {
                let joined = JoinedReply {
                    peer_id,
                    participants,
                    server_join_token,
                    participant_profiles,
                };
                Self::run_joined_session(
                    this,
                    cx,
                    &client,
                    &mut event_rx,
                    &server_address,
                    &session_id,
                    &join_token,
                    &joined,
                )
                .await;
            }
            Some(ServerMessage::Error { message }) => {
                Self::report_join_error(this, cx, session_logic::server_error_text(&message));
            }
            Some(_) => {
                Self::report_join_error(this, cx, "Unexpected server response".to_string());
            }
            None => {
                Self::report_join_error(this, cx, "Connection closed before response".to_string());
            }
        }
    }

    /// Shows a join failure in the UI and the engine status.
    fn report_join_error(this: &WeakEntity<Self>, cx: &mut AsyncApp, text: String) {
        cx.update(|cx| {
            this.update(cx, |this, cx| {
                this.set_connection_error(text);
                cx.notify();
            });
        });
    }

    /// Applies the `Joined` reply, asks the host for its manifest, then pumps events.
    #[allow(clippy::too_many_arguments)]
    async fn run_joined_session(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        client: &Arc<RwLock<MultiuserClient>>,
        event_rx: &mut UnboundedReceiver<ServerMessage>,
        server_address: &str,
        session_id: &str,
        join_token: &str,
        joined: &JoinedReply,
    ) {
        cx.update(|cx| {
            this.update(cx, |this, cx| {
                this.apply_guest_joined(client, server_address, session_id, join_token, joined, cx);
            });
        });

        Self::request_host_manifest(this, cx, client, session_id, &joined.participants).await;

        // Continue listening for PeerJoined/PeerLeft events
        Self::run_guest_event_loop(this, cx, client, event_rx).await;
    }

    /// Stores the connected state and finishes setup (presence, replication, engine sync).
    fn apply_guest_joined(
        &mut self,
        client: &Arc<RwLock<MultiuserClient>>,
        server_address: &str,
        session_id: &str,
        join_token: &str,
        joined: &JoinedReply,
        cx: &mut Context<Self>,
    ) {
        self.connection_status = ConnectionStatus::Connected;
        // Store our peer_id first
        self.current_peer_id = Some(joined.peer_id.clone());

        tracing::debug!(
            "{}: Received Joined - our peer_id: {}, participants: {:?}",
            TAG,
            joined.peer_id,
            joined.participants
        );

        self.active_session = Some(ActiveSession {
            session_id: session_id.to_string(),
            join_token: joined
                .server_join_token
                .clone()
                .unwrap_or_else(|| join_token.to_string()),
            server_address: server_address.to_string(),
            // Store raw participant list
            connected_users: joined.participants.clone(),
        });

        self.finish_joined_setup(TAG, client, server_address, session_id, joined, cx);
    }

    /// Requests the file manifest from the host (the first participant).
    async fn request_host_manifest(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        client: &Arc<RwLock<MultiuserClient>>,
        session_id: &str,
        participants: &[String],
    ) {
        if let Some(host_peer_id) = participants.first() {
            tracing::debug!(
                "{}: Requesting file manifest from host {}",
                TAG,
                host_peer_id
            );

            let our_peer_id = Self::read_own_peer_id(this, cx);

            if let Some(our_peer_id) = our_peer_id {
                let client_guard = client.read().await;
                let _ = client_guard
                    .send(ClientMessage::RequestFileManifest {
                        session_id: session_id.to_string(),
                        peer_id: our_peer_id,
                    })
                    .await;
                tracing::debug!("{}: Sent RequestFileManifest", TAG);
            }
        }
    }

    /// Handles server messages until the session ends, we are kicked or the channel closes.
    async fn run_guest_event_loop(
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

            if Self::handle_guest_message(this, cx, client, msg)
                .await
                .is_break()
            {
                break;
            }

            Self::report_latency(this, cx, client).await;
        }
    }

    /// Dispatches one server message; `Break` ends the event loop.
    async fn handle_guest_message(
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
                        this.apply_guest_peer_joined(joined_peer_id, profile, cx);
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
                Self::report_join_error(this, cx, session_logic::server_error_text(&message));
            }
            ServerMessage::FileManifest {
                from_peer_id,
                manifest_json,
                ..
            } => {
                Self::handle_guest_file_manifest(this, cx, client, from_peer_id, manifest_json)
                    .await;
            }
            ServerMessage::FileChunk {
                from_peer_id,
                file_path,
                offset,
                data,
                is_last,
                ..
            } => {
                tracing::debug!(
                    "{}: Received FileChunk for {} from {} (offset: {}, is_last: {})",
                    TAG,
                    file_path,
                    from_peer_id,
                    offset,
                    is_last
                );
                Self::handle_guest_file_chunk(this, cx, file_path, offset, data, is_last);
            }
            ServerMessage::FilesChunk {
                from_peer_id,
                files_json,
                chunk_index,
                total_chunks,
                ..
            } => {
                tracing::debug!(
                    "{}: Received FilesChunk from {} (chunk {}/{})",
                    TAG,
                    from_peer_id,
                    chunk_index + 1,
                    total_chunks
                );
                Self::handle_guest_files_chunk(this, cx, files_json, chunk_index, total_chunks);
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
                Self::guest_serve_manifest(this, cx, client, req_session_id);
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
                Self::guest_serve_files(this, cx, client, req_session_id, file_paths);
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
                Self::guest_serve_file(this, cx, client, req_session_id, file_path).await;
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

    /// Adds a newly joined peer to the session, replication system and engine state.
    fn apply_guest_peer_joined(
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
                // Add to replication system
                let integration = ui::replication::MultiuserIntegration::new(cx);
                let color = Self::generate_user_color(&joined_peer_id);
                let user_index = session.connected_users.len();
                integration.add_user(
                    joined_peer_id,
                    session_logic::user_label(user_index),
                    color,
                    cx,
                );
                cx.notify();
            } else {
                tracing::debug!("{}: Peer {} already in list", TAG, joined_peer_id);
            }
        }

        self.resync_engine_participants();
        if let Some(profile) = profile {
            Self::merge_engine_peer_profile(&profile);
        }
    }

    /// Parses the host's manifest, diffs it against the local project and, when
    /// changes exist, shows the file-sync UI and requests contents for preview.
    async fn handle_guest_file_manifest(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        client: &Arc<RwLock<MultiuserClient>>,
        from_peer_id: String,
        manifest_json: String,
    ) {
        tracing::debug!(
            "{}: Received file manifest from {} ({} bytes)",
            TAG,
            from_peer_id,
            manifest_json.len()
        );

        // Parse manifest and compute diff
        let Some(project_root) = Self::read_project_root(this, cx) else {
            tracing::error!("{}: No project_root available", TAG);
            return;
        };
        tracing::debug!("{}: Project root is {:?}", TAG, project_root.display());

        // Deserialize manifest
        let remote_manifest =
            match serde_json::from_str::<simple_sync::FileManifest>(&manifest_json) {
                Ok(remote_manifest) => remote_manifest,
                Err(e) => {
                    tracing::error!("{}: Failed to parse manifest: {}", TAG, e);
                    return;
                }
            };
        tracing::debug!(
            "{}: Parsed manifest with {} files",
            TAG,
            remote_manifest.files.len()
        );

        // Compute diff synchronously (fast enough for most cases)
        let diff = match simple_sync::compute_diff(&project_root, &remote_manifest) {
            Ok(diff) => diff,
            Err(e) => {
                tracing::error!("{}: Failed to compute diff: {}", TAG, e);
                return;
            }
        };
        tracing::debug!("{}: Computed diff - {}", TAG, diff.summary());

        if !diff.has_changes() {
            tracing::debug!("{}: No changes needed, already in sync", TAG);
            return;
        }
        tracing::debug!("{}: Changes detected, showing sync UI", TAG);

        // Collect files to request for preview
        let files_to_preview =
            session_logic::preview_paths(&diff.files_to_add, &diff.files_to_update);

        // Update UI to show pending sync
        cx.update(|cx| {
            this.update(cx, |this, cx| {
                // Queue the diff populate (will be processed on next render with window access)
                this.queue_diff_populate(diff.clone(), cx);

                this.pending_file_sync = Some((diff, from_peer_id));
                this.current_tab = SessionTab::FileSync;
                tracing::debug!(
                    "{}: Set pending_file_sync and switched to FileSync tab",
                    TAG
                );
                cx.notify();
            })
            .ok()
        });

        // Request file contents for preview (don't write to disk yet)
        if !files_to_preview.is_empty() {
            Self::request_preview_files(this, cx, client, files_to_preview).await;
        }
    }

    /// Asks the host for the given files so they can be previewed before writing to disk.
    async fn request_preview_files(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        client: &Arc<RwLock<MultiuserClient>>,
        files_to_preview: Vec<String>,
    ) {
        tracing::debug!(
            "{}: Requesting {} files for preview",
            TAG,
            files_to_preview.len()
        );

        let client_guard = client.read().await;
        let (session_id_result, peer_id_result) = cx
            .update(|cx| {
                this.update(cx, |this, _cx| {
                    let session_id = this.active_session.as_ref().map(|s| s.session_id.clone());
                    let peer_id = this.current_peer_id.clone();
                    (session_id, peer_id)
                })
            })
            .unwrap_or((None, None));

        if let (Some(session_id), Some(peer_id)) = (session_id_result, peer_id_result) {
            let _ = client_guard
                .send(ClientMessage::RequestFiles {
                    session_id,
                    peer_id,
                    file_paths: files_to_preview,
                })
                .await;
        }
    }

    /// Writes one received chunk of a file into the project directory.
    fn handle_guest_file_chunk(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        file_path: String,
        offset: u64,
        data: Vec<u8>,
        is_last: bool,
    ) {
        // Write chunk to file
        if let Some(project_root) = Self::read_project_root(this, cx) {
            session_logic::write_file_chunk(&project_root, &file_path, offset, &data);

            if is_last {
                tracing::debug!("{}: Completed download of {}", TAG, file_path);
            }
        }
    }

    /// Handles a `FilesChunk` carrying file contents: updates progress, previews
    /// text files and applies the files to the project.
    fn handle_guest_files_chunk(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        files_json: String,
        chunk_index: usize,
        total_chunks: usize,
    ) {
        // Set progress indicator - receiving data
        cx.update(|cx| {
            this.update(cx, |this, cx| {
                this.set_sync_progress("Receiving files...".to_string(), 0.1, cx);
            });
        });

        // For now, handle single chunk only
        // TODO: Handle multi-chunk transfers by buffering
        if session_logic::is_single_chunk_transfer(chunk_index, total_chunks) {
            if let Some(project_root) = Self::read_project_root(this, cx) {
                // Update progress - starting sync
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.set_sync_progress("Processing files...".to_string(), 0.2, cx);
                    });
                });

                Self::apply_received_files(this, cx, &project_root, &files_json);
            }
        }
    }

    /// Deserializes a files payload, feeds previews to the sync UI and writes the
    /// files with `simple_sync::apply_files`, reporting success or failure in the UI.
    fn apply_received_files(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        project_root: &std::path::Path,
        files_json: &str,
    ) {
        // Deserialize and apply files synchronously (fast enough for most cases)
        tracing::debug!(
            "SYNC_TASK: Deserializing files from JSON ({} bytes)",
            files_json.len()
        );
        let files_data = match session_logic::parse_files_payload(files_json) {
            Ok(files_data) => files_data,
            Err(e) => {
                tracing::error!("SYNC_ERROR: Failed to deserialize files: {}", e);
                Self::finish_sync_in_ui(this, cx, session_logic::sync_failed_message(e), None);
                return;
            }
        };
        tracing::debug!(
            "SYNC_TASK: Successfully deserialized {} files",
            files_data.len()
        );

        // Update FileSyncUI with file contents for preview
        for (file_path, content) in session_logic::text_previews(&files_data) {
            cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.queue_file_content_update(file_path, content, cx);
                })
                .ok()
            });
        }

        // Apply files using simple_sync
        match simple_sync::apply_files(project_root, files_data) {
            Ok(written_count) => {
                tracing::debug!("SYNC_SUCCESS: Applied {} files successfully", written_count);

                // Update UI
                tracing::debug!(
                    "SYNC_SUCCESS: Updating UI state - clearing progress and pending sync"
                );
                Self::finish_sync_in_ui(
                    this,
                    cx,
                    session_logic::synced_message(written_count),
                    Some(1.0),
                );

                tracing::debug!("SYNC_SUCCESS: File synchronization complete!");
            }
            Err(e) => {
                tracing::error!("SYNC_ERROR: Failed to apply files: {}", e);
                Self::finish_sync_in_ui(this, cx, session_logic::sync_failed_message(e), None);
            }
        }
    }

    /// Sets the sync progress message and fraction shown in the file-sync tab.
    fn set_sync_progress(&mut self, message: String, percent: f32, cx: &mut Context<Self>) {
        self.sync_progress_message = Some(message);
        self.sync_progress_percent = Some(percent);
        cx.notify();
    }

    /// Ends a sync attempt: clears the in-progress flag and pending diff and shows `message`.
    fn finish_sync_in_ui(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        message: String,
        percent: Option<f32>,
    ) {
        cx.update(|cx| {
            this.update(cx, |this, cx| {
                this.file_sync_in_progress = false;
                this.pending_file_sync = None;
                this.sync_progress_message = Some(message);
                this.sync_progress_percent = percent;
                cx.notify();
            })
            .ok()
        });
    }

    /// Builds our file manifest on a background thread and sends it to the requester.
    fn guest_serve_manifest(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        client: &Arc<RwLock<MultiuserClient>>,
        req_session_id: String,
    ) {
        // Create and send file manifest (same logic as in CREATE_SESSION)
        let Some(project_root) = Self::read_project_root(this, cx) else {
            return;
        };
        let Some(our_peer_id) = Self::read_own_peer_id(this, cx) else {
            return;
        };

        // Create manifest in background thread
        let client_clone = client.clone();
        let session_id_clone = req_session_id.clone();
        editor_task_queue::spawn_named(editor_task_queue::ThreadSpec::new("ui multiplayer: guest serve manifest", editor_task_queue::ThreadRole::Io).owner("ui_multiplayer"), move || {
            tracing::debug!("PEER: Creating file manifest");
            match simple_sync::create_manifest(&project_root) {
                Ok(manifest) => match serde_json::to_string(&manifest) {
                    Ok(manifest_json) => {
                        tokio::runtime::Runtime::new().unwrap().block_on(async {
                            let client_guard = client_clone.read().await;
                            let _ = client_guard
                                .send(ClientMessage::FileManifest {
                                    session_id: session_id_clone,
                                    peer_id: our_peer_id,
                                    manifest_json,
                                })
                                .await;
                            tracing::debug!("PEER: Sent file manifest");
                        });
                    }
                    Err(e) => tracing::error!("PEER: Failed to serialize manifest: {}", e),
                },
                Err(e) => tracing::error!("PEER: Failed to create manifest: {}", e),
            }
        });
    }

    /// Reads the requested files on a background thread and sends them as one `FilesChunk`.
    fn guest_serve_files(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        client: &Arc<RwLock<MultiuserClient>>,
        req_session_id: String,
        file_paths: Vec<String>,
    ) {
        // Read and send requested files (same logic as in CREATE_SESSION)
        let Some(project_root) = Self::read_project_root(this, cx) else {
            return;
        };
        let Some(our_peer_id) = Self::read_own_peer_id(this, cx) else {
            return;
        };

        // Read files in background thread
        let client_clone = client.clone();
        let session_id_clone = req_session_id.clone();
        editor_task_queue::spawn_named(editor_task_queue::ThreadSpec::new("ui multiplayer: guest serve files", editor_task_queue::ThreadRole::Io).owner("ui_multiplayer"), move || {
            tracing::debug!("PEER: Reading {} files", file_paths.len());
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
                                tracing::debug!("PEER: Sent {} files", file_paths.len());
                            });
                        }
                        Err(e) => tracing::error!("PEER: Failed to serialize files: {}", e),
                    }
                }
                Err(e) => tracing::error!("PEER: Failed to read files: {}", e),
            }
        });
    }

    /// Reads one file and sends it to the requester in `FILE_CHUNK_SIZE` chunks.
    async fn guest_serve_file(
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

        let Ok(data) = engine_fs::virtual_fs::read_file(&full_path) else {
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
    }
}
