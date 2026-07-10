//! Peer state management - guest connection flow

//! Session creation (host path) for multiplayer sessions

use futures::StreamExt;
use gpui::*;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::screen::MultiplayerWindow;
use crate::utils::types::*;
use engine_backend::subsystems::networking::multiuser::{
    ClientMessage, MultiuserClient, ServerMessage,
};
use engine_backend::subsystems::networking::simple_sync;
use engine_state::{EngineContext, MultiuserParticipant};

impl MultiplayerWindow {
    pub(super) fn join_session(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let server_address = self.server_address_input.read(cx).text().to_string();
        let session_id = self.session_id_input.read(cx).text().to_string();
        let join_token = self.session_password_input.read(cx).text().to_string();

        if server_address.is_empty() {
            self.connection_status =
                ConnectionStatus::Error("Server address is required".to_string());
            cx.notify();
            return;
        }

        if session_id.is_empty() || join_token.is_empty() {
            self.connection_status =
                ConnectionStatus::Error("Session ID and password are required".to_string());
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
            let join_token_clone = join_token.clone();

            let event_rx_result = {
                let mut client_guard = client.write().await;
                client_guard
                    .connect(session_id.clone(), join_token_clone, identity)
                    .await
            }; // client_guard dropped here, releasing the lock

            match event_rx_result {
                Ok(mut event_rx) => {
                    // Wait for the initial response (Joined or Error)
                    match event_rx.recv().await {
                        Some(ServerMessage::Joined {
                            peer_id,
                            participants,
                            join_token: server_join_token,
                            participant_profiles,
                            ..
                        }) => {
                            cx.update(|cx| {
                                this.update(cx, |this, cx| {
                                    this.connection_status = ConnectionStatus::Connected;
                                    // Store our peer_id first
                                    this.current_peer_id = Some(peer_id.clone());

                                    tracing::debug!(
                                        "JOIN_SESSION: Received Joined - our peer_id: {}, participants: {:?}",
                                        peer_id,
                                        participants
                                    );

                                    this.active_session = Some(ActiveSession {
                                        session_id: session_id.clone(),
                                        join_token: server_join_token
                                            .clone()
                                            .unwrap_or_else(|| join_token.clone()),
                                        server_address: server_address.clone(),
                                        // Store raw participant list
                                        connected_users: participants.clone(),
                                    });

                                    // Initialize presence for all participants
                                    this.update_presence_from_participants(cx);

                                    // Log project root
                                    if let Some(project_root) = &this.project_root {
                                        tracing::debug!("JOIN_SESSION: Project root at {:?}", project_root);
                                    }

                                    // Initialize UI replication system
                                    ui::replication::init(cx);

                                    // Set up replication bridge
                                    let integration = ui::replication::MultiuserIntegration::new(cx);
                                    let host_peer_id = participants.first().cloned().unwrap_or_else(|| peer_id.clone());

                                    // Set up message sender
                                    let client_clone = client.clone();
                                    let session_id_clone = session_id.clone();
                                    let peer_id_clone = peer_id.clone();
                                    integration.start_session(
                                        peer_id.clone(),
                                        host_peer_id,
                                        move |replication_msg| {
                                            let client = client_clone.clone();
                                            let session_id = session_id_clone.clone();
                                            let peer_id = peer_id_clone.clone();
                                            if let Ok(data) = serde_json::to_string(&replication_msg) {
                                                tokio::spawn(async move {
                                                    let client_guard = client.read().await;
                                                    let _ = client_guard.send(ClientMessage::ReplicationUpdate {
                                                        session_id,
                                                        peer_id,
                                                        data,
                                                    }).await;
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
                                            format!("User {}", i + 1),
                                            color,
                                            cx,
                                        );
                                    }

                                    tracing::debug!("JOIN_SESSION: Initialized replication bridge");
                                    this.sync_engine_multiuser_connected(
                                        &server_address,
                                        &session_id,
                                        &peer_id,
                                        &participants,
                                    );
                                    this.start_fs_event_forwarder(
                                        client.clone(),
                                        session_id.clone(),
                                        peer_id.clone(),
                                        cx,
                                    );
                                    if let Some(profiles) = participant_profiles {
                                        this.sync_engine_multiuser_profiles(profiles);
                                    }

                                    cx.notify();
                                });
                            });

                            // Request file manifest from host (first participant)
                            if let Some(host_peer_id) = participants.first() {
                                tracing::debug!("JOIN_SESSION: Requesting file manifest from host {}", host_peer_id);

                                let our_peer_id = cx.update(|cx| {
                                    this.update(cx, |this, _cx| {
                                        this.current_peer_id.clone()
                                    })
                                }).ok().flatten();

                                if let Some(our_peer_id) = our_peer_id {
                                    let client_guard = client.read().await;
                                    let _ = client_guard.send(ClientMessage::RequestFileManifest {
                                        session_id: session_id.clone(),
                                        peer_id: our_peer_id,
                                    }).await;
                                    tracing::debug!("JOIN_SESSION: Sent RequestFileManifest");
                                }
                            }

                            // Continue listening for PeerJoined/PeerLeft events
                            while let Some(msg) = event_rx.recv().await {
                                // Check if we're still connected - break if disconnected
                                let still_connected = cx.update(|cx| {
                                    this.update(cx, |this, _cx| {
                                        this.active_session.is_some()
                                    })
                                }).unwrap_or(false);

                                if !still_connected {
                                    tracing::debug!("JOIN_SESSION: Session disconnected, stopping event loop");
                                    break;
                                }

                                match msg {
                                    ServerMessage::PeerJoined {
                                        peer_id: joined_peer_id,
                                        profile,
                                        ..
                                    } => {
                                        cx.update(|cx| {
                                            this.update(cx, |this, cx| {
                                                tracing::debug!(
                                                    "JOIN_SESSION: Received PeerJoined - joined_peer_id: {}, our peer_id: {:?}",
                                                    joined_peer_id,
                                                    this.current_peer_id
                                                );

                                                // Ignore PeerJoined about ourselves
                                                if this.current_peer_id.as_ref() == Some(&joined_peer_id) {
                                                    tracing::debug!("JOIN_SESSION: Ignoring PeerJoined about ourselves");
                                                    return;
                                                }

                                                if let Some(session) = &mut this.active_session {
                                                    // Add raw peer_id if not already present
                                                    if !session.connected_users.contains(&joined_peer_id) {
                                                        tracing::debug!(
                                                            "JOIN_SESSION: Adding peer {} to list. List before: {:?}",
                                                            joined_peer_id,
                                                            session.connected_users
                                                        );
                                                        session.connected_users.push(joined_peer_id.clone());
                                                        // Add to replication system
                                                        let integration = ui::replication::MultiuserIntegration::new(cx);
                                                        let color = Self::generate_user_color(&joined_peer_id);
                                                        let user_index = session.connected_users.len();
                                                        integration.add_user(
                                                            joined_peer_id,
                                                            format!("User {}", user_index),
                                                            color,
                                                            cx,
                                                        );
                                                        cx.notify();
                                                    } else {
                                                        tracing::debug!("JOIN_SESSION: Peer {} already in list", joined_peer_id);
                                                    }
                                                }

                                                let session_snapshot =
                                                    this.active_session.as_ref().map(|s| {
                                                        (
                                                            s.server_address.clone(),
                                                            s.session_id.clone(),
                                                            s.connected_users.clone(),
                                                        )
                                                    });
                                                if let (
                                                    Some(peer_id),
                                                    Some((server_address, session_id, participants)),
                                                ) = (this.current_peer_id.as_ref(), session_snapshot)
                                                {
                                                    this.sync_engine_multiuser_connected(
                                                        &server_address,
                                                        &session_id,
                                                        peer_id,
                                                        &participants,
                                                    );
                                                }
                                                if let Some(profile) = profile {
                                                    if let Some(engine) = EngineContext::global() {
                                                        let _ = engine.update_multiuser(|mu| {
                                                            if let Some(existing) = mu
                                                                .participant_profiles
                                                                .iter_mut()
                                                                .find(|p| p.peer_id == profile.peer_id)
                                                            {
                                                                existing.display_name = profile.display_name.clone();
                                                                existing.avatar_url = profile.avatar_url.clone();
                                                                existing.github_login = profile.github_login.clone();
                                                            } else {
                                                                mu.participant_profiles.push(MultiuserParticipant {
                                                                    peer_id: profile.peer_id.clone(),
                                                                    display_name: profile.display_name.clone(),
                                                                    avatar_url: profile.avatar_url.clone(),
                                                                    github_login: profile.github_login.clone(),
                                                                    ping_ms: None,
                                                                });
                                                            }
                                                        });
                                                        engine.notify_multiuser_changed();
                                                    }
                                                }
                                            });
                                        });
                                    }
                                    ServerMessage::PeerLeft { peer_id: left_peer_id, .. } => {
                                        cx.update(|cx| {
                                            this.update(cx, |this, cx| {
                                                if let Some(session) = &mut this.active_session {
                                                    session.connected_users.retain(|p| p != &left_peer_id);
                                                    this.user_presences.retain(|p| p.peer_id != left_peer_id);
                                                    // Remove from replication system
                                                    let integration = ui::replication::MultiuserIntegration::new(cx);
                                                    integration.remove_user(&left_peer_id, cx);
                                                    cx.notify();
                                                }

                                                let session_snapshot =
                                                    this.active_session.as_ref().map(|s| {
                                                        (
                                                            s.server_address.clone(),
                                                            s.session_id.clone(),
                                                            s.connected_users.clone(),
                                                        )
                                                    });
                                                if let (
                                                    Some(peer_id),
                                                    Some((server_address, session_id, participants)),
                                                ) = (this.current_peer_id.as_ref(), session_snapshot)
                                                {
                                                    this.sync_engine_multiuser_connected(
                                                        &server_address,
                                                        &session_id,
                                                        peer_id,
                                                        &participants,
                                                    );
                                                }
                                                if let Some(engine) = EngineContext::global() {
                                                    let _ = engine.update_multiuser(|mu| {
                                                        mu.participant_profiles
                                                            .retain(|p| p.peer_id != left_peer_id);
                                                    });
                                                    engine.notify_multiuser_changed();
                                                }
                                            });
                                        });
                                    }
                                    ServerMessage::Kicked { reason, .. } => {
                                        tracing::warn!("JOIN_SESSION: You were kicked from the session: {}", reason);
                                        cx.update(|cx| {
                                            this.update(cx, |this, cx| {
                                                this.handle_kicked(
                                                    format!("Kicked from session: {}", reason),
                                                    cx,
                                                );
                                            });
                                        });
                                        break; // Exit the message loop
                                    }
                                    ServerMessage::ChatMessage { peer_id: sender_peer_id, message, timestamp, .. } => {
                                        tracing::debug!(
                                            "JOIN_SESSION: Received ChatMessage from {} at {}: {}",
                                            sender_peer_id, timestamp, message
                                        );
                                        cx.update(|cx| {
                                            this.update(cx, |this, cx| {
                                                let is_self = this.current_peer_id.as_ref() == Some(&sender_peer_id);
                                                tracing::debug!(
                                                    "JOIN_SESSION: Adding chat message. is_self: {}, current chat count: {}",
                                                    is_self, this.chat_messages.len()
                                                );
                                                this.chat_messages.push(ChatMessage {
                                                    peer_id: sender_peer_id,
                                                    message,
                                                    timestamp,
                                                    is_self,
                                                });
                                                tracing::debug!("JOIN_SESSION: Chat messages now: {}", this.chat_messages.len());
                                                cx.notify();
                                            });
                                        });
                                    }
                                    ServerMessage::Error { message } => {
                                        cx.update(|cx| {
                                            this.update(cx, |this, cx| {
                                                this.connection_status = ConnectionStatus::Error(format!("Server error: {}", message));
                                                this.sync_engine_multiuser_error(format!(
                                                    "Server error: {}",
                                                    message
                                                ));
                                                cx.notify();
                                            });
                                        });
                                    }
                                    ServerMessage::FileManifest { from_peer_id, manifest_json, .. } => {
                                        tracing::debug!("JOIN_SESSION: Received file manifest from {} ({} bytes)",
                                            from_peer_id, manifest_json.len());

                                        // Parse manifest and compute diff
                                        let project_root = cx.update(|cx| {
                                            this.update(cx, |this, _cx| {
                                                this.project_root.clone()
                                            })
                                        }).ok().flatten();

                                        if let Some(project_root) = project_root {
                                            tracing::debug!("JOIN_SESSION: Project root is {:?}", project_root.display());

                                            // Deserialize manifest
                                            match serde_json::from_str::<simple_sync::FileManifest>(&manifest_json) {
                                                Ok(remote_manifest) => {
                                                    tracing::debug!("JOIN_SESSION: Parsed manifest with {} files", remote_manifest.files.len());

                                                    // Compute diff synchronously (fast enough for most cases)
                                                    match simple_sync::compute_diff(&project_root, &remote_manifest) {
                                                        Ok(diff) => {
                                                            tracing::debug!("JOIN_SESSION: Computed diff - {}", diff.summary());

                                                            if diff.has_changes() {
                                                                tracing::debug!("JOIN_SESSION: Changes detected, showing sync UI");

                                                                // Collect files to request for preview
                                                                let mut files_to_preview = Vec::new();
                                                                files_to_preview.extend(diff.files_to_add.clone());
                                                                files_to_preview.extend(diff.files_to_update.clone());

                                                                // Update UI to show pending sync
                                                                let diff_clone = diff.clone();
                                                                cx.update(|cx| {
                                                                    this.update(cx, |this, cx| {
                                                                        // Queue the diff populate (will be processed on next render with window access)
                                                                        this.queue_diff_populate(diff_clone.clone(), cx);

                                                                        this.pending_file_sync = Some((diff_clone, from_peer_id.clone()));
                                                                        this.current_tab = SessionTab::FileSync;
                                                                        tracing::debug!("JOIN_SESSION: Set pending_file_sync and switched to FileSync tab");
                                                                        cx.notify();
                                                                    }).ok()
                                                                });

                                                                // Request file contents for preview (don't write to disk yet)
                                                                if !files_to_preview.is_empty() {
                                                                    tracing::debug!("JOIN_SESSION: Requesting {} files for preview", files_to_preview.len());

                                                                    let client_guard = client.read().await;
                                                                    let (session_id_result, peer_id_result) = cx.update(|cx| {
                                                                        this.update(cx, |this, _cx| {
                                                                            let session_id = this.active_session.as_ref().map(|s| s.session_id.clone());
                                                                            let peer_id = this.current_peer_id.clone();
                                                                            (session_id, peer_id)
                                                                        })
                                                                    }).unwrap_or((None, None));

                                                                    if let (Some(session_id), Some(peer_id)) = (session_id_result, peer_id_result) {
                                                                        let _ = client_guard.send(ClientMessage::RequestFiles {
                                                                            session_id,
                                                                            peer_id,
                                                                            file_paths: files_to_preview,
                                                                        }).await;
                                                                    }
                                                                }
                                                            } else {
                                                                tracing::debug!("JOIN_SESSION: No changes needed, already in sync");
                                                            }
                                                        }
                                                        Err(e) => {
                                                            tracing::error!("JOIN_SESSION: Failed to compute diff: {}", e);
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    tracing::error!("JOIN_SESSION: Failed to parse manifest: {}", e);
                                                }
                                            }
                                        } else {
                                            tracing::error!("JOIN_SESSION: No project_root available");
                                        }
                                    }
                                    ServerMessage::FileChunk { from_peer_id, file_path, offset, data, is_last, .. } => {
                                        tracing::debug!("JOIN_SESSION: Received FileChunk for {} from {} (offset: {}, is_last: {})",
                                            file_path, from_peer_id, offset, is_last);

                                        // Write chunk to file
                                        let project_root = cx.update(|cx| {
                                            this.update(cx, |this, _cx| {
                                                this.project_root.clone()
                                            })
                                        }).ok().flatten();

                                        if let Some(project_root) = project_root {
                                            let full_path = project_root.join(&file_path);

                                            // Create parent directories if needed
                                            if let Some(parent) = full_path.parent() {
                                                let _ = std::fs::create_dir_all(parent);
                                            }

                                            // Write chunk
                                            use std::io::{Write, Seek, SeekFrom};
                                            if let Ok(mut file) = std::fs::OpenOptions::new()
                                                .create(true)
                                                .write(true)
                                                .open(&full_path)
                                            {
                                                let _ = file.seek(SeekFrom::Start(offset));
                                                let _ = file.write_all(&data);
                                            }

                                            if is_last {
                                                tracing::debug!("JOIN_SESSION: Completed download of {}", file_path);
                                            }
                                        }
                                    }
                                    ServerMessage::FilesChunk { from_peer_id, files_json, chunk_index, total_chunks, .. } => {
                                        tracing::debug!("JOIN_SESSION: Received FilesChunk from {} (chunk {}/{})",
                                            from_peer_id, chunk_index + 1, total_chunks);

                                        // Set progress indicator - receiving data
                                        cx.update(|cx| {
                                            this.update(cx, |this, cx| {
                                                this.sync_progress_message = Some("Receiving files...".to_string());
                                                this.sync_progress_percent = Some(0.1);
                                                cx.notify();
                                            });
                                        });

                                        // For now, handle single chunk only
                                        // TODO: Handle multi-chunk transfers by buffering
                                        if chunk_index == 0 && total_chunks == 1 {
                                            let project_root = cx.update(|cx| {
                                                this.update(cx, |this, _cx| {
                                                    this.project_root.clone()
                                                })
                                            }).ok().flatten();

                                            if let Some(project_root) = project_root {
                                                // Update progress - starting sync
                                                cx.update(|cx| {
                                                    this.update(cx, |this, cx| {
                                                        this.sync_progress_message = Some("Processing files...".to_string());
                                                        this.sync_progress_percent = Some(0.2);
                                                        cx.notify();
                                                    });
                                                });

                                                // Deserialize and apply files synchronously (fast enough for most cases)
                                                tracing::debug!("SYNC_TASK: Deserializing files from JSON ({} bytes)", files_json.len());
                                                match serde_json::from_str::<Vec<(String, Vec<u8>)>>(&files_json) {
                                                    Ok(files_data) => {
                                                        tracing::debug!("SYNC_TASK: Successfully deserialized {} files", files_data.len());

                                                        // Update FileSyncUI with file contents for preview
                                                        for (file_path, data) in &files_data {
                                                            if let Ok(content) = String::from_utf8(data.clone()) {
                                                                let file_path_clone = file_path.clone();
                                                                let content_clone = content.clone();
                                                                cx.update(|cx| {
                                                                    this.update(cx, |this, cx| {
                                                                        this.queue_file_content_update(file_path_clone, content_clone, cx);
                                                                    }).ok()
                                                                });
                                                            }
                                                        }

                                                        // Apply files using simple_sync
                                                        match simple_sync::apply_files(&project_root, files_data) {
                                                            Ok(written_count) => {
                                                                tracing::debug!("SYNC_SUCCESS: Applied {} files successfully", written_count);

                                                                // Update UI
                                                                cx.update(|cx| {
                                                                    this.update(cx, |this, cx| {
                                                                        tracing::debug!("SYNC_SUCCESS: Updating UI state - clearing progress and pending sync");
                                                                        this.file_sync_in_progress = false;
                                                                        this.pending_file_sync = None;
                                                                        this.sync_progress_message = Some(format!("Synced {} files successfully", written_count));
                                                                        this.sync_progress_percent = Some(1.0);

                                                                        tracing::debug!("SYNC_SUCCESS: State updated - file_sync_in_progress={}, pending_file_sync={:?}",
                                                                            this.file_sync_in_progress, this.pending_file_sync.is_some());
                                                                        cx.notify();
                                                                    }).ok()
                                                                });

                                                                tracing::debug!("SYNC_SUCCESS: File synchronization complete!");
                                                            }
                                                            Err(e) => {
                                                                tracing::error!("SYNC_ERROR: Failed to apply files: {}", e);
                                                                cx.update(|cx| {
                                                                    this.update(cx, |this, cx| {
                                                                        this.file_sync_in_progress = false;
                                                                        this.pending_file_sync = None;
                                                                        this.sync_progress_message = Some(format!("Sync failed: {}", e));
                                                                        this.sync_progress_percent = None;
                                                                        cx.notify();
                                                                    }).ok()
                                                                });
                                                            }
                                                        }
                                                    }
                                                    Err(e) => {
                                                        tracing::error!("SYNC_ERROR: Failed to deserialize files: {}", e);
                                                        cx.update(|cx| {
                                                            this.update(cx, |this, cx| {
                                                                this.file_sync_in_progress = false;
                                                                this.pending_file_sync = None;
                                                                this.sync_progress_message = Some(format!("Sync failed: {}", e));
                                                                this.sync_progress_percent = None;
                                                                cx.notify();
                                                            }).ok()
                                                        });
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    ServerMessage::FileChanged { path, kind, .. } => {
                                        let change_kind = match kind.as_str() {
                                            "created" => engine_fs::events::FsChangeKind::Created,
                                            "deleted" => engine_fs::events::FsChangeKind::Deleted,
                                            _ => engine_fs::events::FsChangeKind::Modified,
                                        };
                                        engine_fs::events::emit_remote(path, change_kind);
                                    }
                                    ServerMessage::RequestFileManifest { from_peer_id, session_id: req_session_id, .. } => {
                                        tracing::debug!("JOIN_SESSION: Received RequestFileManifest from {}", from_peer_id);

                                        // Create and send file manifest (same logic as in CREATE_SESSION)
                                        let project_root = cx.update(|cx| {
                                            this.update(cx, |this, _cx| {
                                                this.project_root.clone()
                                            })
                                        }).ok().flatten();

                                        if let Some(project_root) = project_root {
                                            let our_peer_id = cx.update(|cx| {
                                                this.update(cx, |this, _cx| {
                                                    this.current_peer_id.clone()
                                                })
                                            }).ok().flatten();

                                            if let Some(our_peer_id) = our_peer_id {
                                                // Create manifest in background thread
                                                let client_clone = client.clone();
                                                let session_id_clone = req_session_id.clone();
                                                std::thread::spawn(move || {
                                                    tracing::debug!("PEER: Creating file manifest");
                                                    match simple_sync::create_manifest(&project_root) {
                                                        Ok(manifest) => {
                                                            match serde_json::to_string(&manifest) {
                                                                Ok(manifest_json) => {
                                                                    tokio::runtime::Runtime::new().unwrap().block_on(async {
                                                                        let client_guard = client_clone.read().await;
                                                                        let _ = client_guard.send(ClientMessage::FileManifest {
                                                                            session_id: session_id_clone,
                                                                            peer_id: our_peer_id,
                                                                            manifest_json,
                                                                        }).await;
                                                                        tracing::debug!("PEER: Sent file manifest");
                                                                    });
                                                                }
                                                                Err(e) => tracing::error!("PEER: Failed to serialize manifest: {}", e),
                                                            }
                                                        }
                                                        Err(e) => tracing::error!("PEER: Failed to create manifest: {}", e),
                                                    }
                                                });
                                            }
                                        }
                                    }
                                    ServerMessage::RequestFiles { from_peer_id, file_paths, session_id: req_session_id, .. } => {
                                        tracing::debug!("JOIN_SESSION: Received RequestFiles for {} files from {}", file_paths.len(), from_peer_id);

                                        // Read and send requested files (same logic as in CREATE_SESSION)
                                        let project_root = cx.update(|cx| {
                                            this.update(cx, |this, _cx| {
                                                this.project_root.clone()
                                            })
                                        }).ok().flatten();

                                        if let Some(project_root) = project_root {
                                            let our_peer_id = cx.update(|cx| {
                                                this.update(cx, |this, _cx| {
                                                    this.current_peer_id.clone()
                                                })
                                            }).ok().flatten();

                                            if let Some(our_peer_id) = our_peer_id {
                                                // Read files in background thread
                                                let client_clone = client.clone();
                                                let session_id_clone = req_session_id.clone();
                                                std::thread::spawn(move || {
                                                    tracing::debug!("PEER: Reading {} files", file_paths.len());
                                                    match simple_sync::read_files(&project_root, file_paths.clone()) {
                                                        Ok(files_data) => {
                                                            // Serialize files data
                                                            match serde_json::to_string(&files_data) {
                                                                Ok(files_json) => {
                                                                    tokio::runtime::Runtime::new().unwrap().block_on(async {
                                                                        let client_guard = client_clone.read().await;
                                                                        let _ = client_guard.send(ClientMessage::FilesChunk {
                                                                            session_id: session_id_clone,
                                                                            peer_id: our_peer_id,
                                                                            files_json,
                                                                            chunk_index: 0,
                                                                            total_chunks: 1,
                                                                        }).await;
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
                                        }
                                    }
                                    ServerMessage::RequestFile { from_peer_id, file_path, session_id: req_session_id, .. } => {
                                        tracing::debug!("JOIN_SESSION: Received RequestFile for {} from {}", file_path, from_peer_id);

                                        // Read and send file in chunks
                                        let project_root = cx.update(|cx| {
                                            this.update(cx, |this, _cx| {
                                                this.project_root.clone()
                                            })
                                        }).ok().flatten();

                                        if let Some(project_root) = project_root {
                                            let full_path = project_root.join(&file_path);

                                            if let Ok(data) = std::fs::read(&full_path) {
                                                const CHUNK_SIZE: usize = 8192;

                                                let our_peer_id = cx.update(|cx| {
                                                    this.update(cx, |this, _cx| {
                                                        this.current_peer_id.clone()
                                                    })
                                                }).ok().flatten();

                                                if let Some(our_peer_id) = our_peer_id {
                                                    for (i, chunk) in data.chunks(CHUNK_SIZE).enumerate() {
                                                        let offset = i * CHUNK_SIZE;
                                                        let is_last = offset + chunk.len() >= data.len();

                                                        let client_guard = client.read().await;
                                                        let _ = client_guard.send(ClientMessage::FileChunk {
                                                            session_id: req_session_id.clone(),
                                                            peer_id: our_peer_id.clone(),
                                                            file_path: file_path.clone(),
                                                            offset: offset as u64,
                                                            data: chunk.to_vec(),
                                                            is_last,
                                                        }).await;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    ServerMessage::ReplicationUpdate { from_peer_id, data, .. } => {
                                        tracing::debug!("JOIN_SESSION: Received ReplicationUpdate from {}", from_peer_id);
                                        // Handle replication message and get response
                                        let response = if let Ok(rep_msg) = serde_json::from_str(&data) {
                                            cx.update(|cx| {
                                                let integration = ui::replication::MultiuserIntegration::new(cx);
                                                integration.handle_incoming_message(rep_msg, cx)
                                            })
                                        } else {
                                            None
                                        };

                                        // Send response if we got one
                                        if let Some(response) = response {
                                            let (session_id_result, our_peer_id_result) = cx.update(|cx| {
                                                this.update(cx, |this, _cx| {
                                                    let session_id = this.active_session.as_ref().map(|s| s.session_id.clone());
                                                    let peer_id = this.current_peer_id.clone();
                                                    (session_id, peer_id)
                                                })
                                            }).unwrap_or((None, None));

                                            if let (Some(session_id), Some(our_peer_id)) = (session_id_result, our_peer_id_result) {
                                                if let Ok(response_data) = serde_json::to_string(&response) {
                                                    let client_guard = client.read().await;
                                                    let _ = client_guard.send(ClientMessage::ReplicationUpdate {
                                                        session_id,
                                                        peer_id: our_peer_id,
                                                        data: response_data,
                                                    }).await;
                                                }
                                            }
                                        }
                                    }
                                    _ => {}
                                }

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
                        Some(ServerMessage::Error { message }) => {
                            cx.update(|cx| {
                                this.update(cx, |this, cx| {
                                    this.connection_status = ConnectionStatus::Error(format!("Server error: {}", message));
                                    this.sync_engine_multiuser_error(format!(
                                        "Server error: {}",
                                        message
                                    ));
                                    cx.notify();
                                });
                            });
                        }
                        Some(_) => {
                            cx.update(|cx| {
                                this.update(cx, |this, cx| {
                                    this.connection_status = ConnectionStatus::Error("Unexpected server response".to_string());
                                    this.sync_engine_multiuser_error(
                                        "Unexpected server response".to_string(),
                                    );
                                    cx.notify();
                                });
                            });
                        }
                        None => {
                            cx.update(|cx| {
                                this.update(cx, |this, cx| {
                                    this.connection_status = ConnectionStatus::Error("Connection closed before response".to_string());
                                    this.sync_engine_multiuser_error(
                                        "Connection closed before response".to_string(),
                                    );
                                    cx.notify();
                                });
                            });
                        }
                    }
                }
                Err(e) => {
                    cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            this.connection_status = ConnectionStatus::Error(format!("Connection failed: {}", e));
                            this.sync_engine_multiuser_error(format!(
                                "Connection failed: {}",
                                e
                            ));
                            cx.notify();
                        });
                    });
                }
            }
        }).detach();
    }
}
