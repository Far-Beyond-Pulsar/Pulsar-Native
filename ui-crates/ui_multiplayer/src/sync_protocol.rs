//! Session creation - host connection flow

//! Session creation (host path) for multiplayer sessions

use futures::StreamExt;
use gpui::*;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::state::MultiplayerWindow;
use super::types::*;
use engine_backend::subsystems::networking::multiuser::{
    ClientMessage, MultiuserClient, ServerMessage,
};
use engine_backend::subsystems::networking::simple_sync;

impl MultiplayerWindow {
    pub(super) fn create_session(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let server_address = self.server_address_input.read(cx).text().to_string();

        if server_address.is_empty() {
            self.connection_status =
                ConnectionStatus::Error("Server address is required".to_string());
            cx.notify();
            return;
        }

        self.connection_status = ConnectionStatus::Connecting;
        cx.notify();

        // Create client
        let client = Arc::new(RwLock::new(MultiuserClient::new(server_address.clone())));
        self.client = Some(client.clone());

        let _session_id_input = self.session_id_input.clone();
        let _session_password_input = self.session_password_input.clone();

        cx.spawn(async move |this, cx| {
            // Call create_session on the client
            let result = {
                let client_guard = client.read().await;
                client_guard.create_session().await
            };

            match result {
                Ok((session_id, join_token)) => {
                    // Store credentials for later display
                    let session_id_for_display = session_id.clone();
                    let join_token_for_display = join_token.clone();

                    // Update UI state
                    cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            this.connection_status = ConnectionStatus::Connected;
                            this.active_session = Some(ActiveSession {
                                session_id: session_id_for_display.clone(),
                                join_token: join_token_for_display.clone(),
                                server_address: server_address.clone(),
                                connected_users: vec!["You (Host)".to_string()],
                            });
                            cx.notify();
                        }).ok();
                    }).ok();

                    // Now connect via WebSocket
                    let event_rx_result = {
                        let mut client_guard = client.write().await;
                        client_guard.connect(session_id.clone(), join_token).await
                    }; // client_guard dropped here, releasing the lock

                    match event_rx_result {
                        Ok(mut event_rx) => {
                            // Wait for the initial response (Joined or Error)
                            match event_rx.recv().await {
                                Some(ServerMessage::Joined { peer_id, participants, .. }) => {
                                    cx.update(|cx| {
                                        this.update(cx, |this, cx| {
                                            // Store our peer_id first
                                            this.current_peer_id = Some(peer_id.clone());

                                            tracing::debug!(
                                                "CREATE_SESSION: Received Joined - our peer_id: {}, participants: {:?}",
                                                peer_id,
                                                participants
                                            );

                                            if let Some(session) = &mut this.active_session {
                                                // Store raw participant list
                                                session.connected_users = participants.clone();
                                            }

                                            // Initialize presence for all participants
                                            this.update_presence_from_participants(cx);

                                            // Log project root for host
                                            if let Some(project_root) = &this.project_root {
                                                tracing::debug!("CREATE_SESSION: Project root at {:?}", project_root);
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

                                            tracing::debug!("CREATE_SESSION: Initialized replication bridge");

                                            cx.notify();
                                        }).ok();
                                    }).ok();

                                    // Continue listening for PeerJoined/PeerLeft events
                                    while let Some(msg) = event_rx.recv().await {
                                        // Check if we're still connected - break if disconnected
                                        let still_connected = cx.update(|cx| {
                                            this.update(cx, |this, _cx| {
                                                this.active_session.is_some()
                                            }).unwrap_or(false)
                                        }).unwrap_or(false);

                                        if !still_connected {
                                            tracing::debug!("CREATE_SESSION: Session disconnected, stopping event loop");
                                            break;
                                        }

                                        match msg {
                                            ServerMessage::PeerJoined { peer_id: joined_peer_id, .. } => {
                                                cx.update(|cx| {
                                                    this.update(cx, |this, cx| {
                                                        tracing::debug!(
                                                            "CREATE_SESSION: Received PeerJoined - joined_peer_id: {}, our peer_id: {:?}",
                                                            joined_peer_id,
                                                            this.current_peer_id
                                                        );

                                                        // Ignore PeerJoined about ourselves
                                                        if this.current_peer_id.as_ref() == Some(&joined_peer_id) {
                                                            tracing::debug!("CREATE_SESSION: Ignoring PeerJoined about ourselves");
                                                            return;
                                                        }

                                                        if let Some(session) = &mut this.active_session {
                                                            // Add raw peer_id if not already present
                                                            if !session.connected_users.contains(&joined_peer_id) {
                                                                tracing::debug!(
                                                                    "CREATE_SESSION: Adding peer {} to list. List before: {:?}",
                                                                    joined_peer_id,
                                                                    session.connected_users
                                                                );
                                                                session.connected_users.push(joined_peer_id.clone());
                                                            }
                                                            let user_index = session.connected_users.len();
                                                            // Session borrow dropped here

                                                            if !this.active_session.as_ref().is_none_or(|s| s.connected_users.contains(&joined_peer_id)) {
                                                                return;
                                                            }

                                                            // Update presence
                                                                this.update_presence_from_participants(cx);
                                                                // Add to replication system
                                                                let integration = ui::replication::MultiuserIntegration::new(cx);
                                                                let color = Self::generate_user_color(&joined_peer_id);
                                                                integration.add_user(
                                                                    joined_peer_id.clone(),
                                                                    format!("User {}", user_index),
                                                                    color,
                                                                    cx,
                                                                );
                                                                cx.notify();
                                                        } else {
                                                            tracing::debug!("CREATE_SESSION: Peer {} already in list", joined_peer_id);
                                                        }
                                                    }).ok();
                                                }).ok();
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
                                                    }).ok();
                                                }).ok();
                                            }
                                            ServerMessage::Kicked { reason, .. } => {
                                                tracing::warn!("CREATE_SESSION: You were kicked from the session: {}", reason);
                                                cx.update(|cx| {
                                                    this.update(cx, |this, cx| {
                                                        // Disconnect and show error
                                                        this.connection_status = ConnectionStatus::Error(
                                                            format!("Kicked from session: {}", reason)
                                                        );
                                                        this.active_session = None;
                                                        this.client = None;
                                                        this.user_presences.clear();
                                                        cx.notify();
                                                    }).ok();
                                                }).ok();
                                                break; // Exit the message loop
                                            }
                                            ServerMessage::ChatMessage { peer_id: sender_peer_id, message, timestamp, .. } => {
                                                tracing::debug!(
                                                    "CREATE_SESSION: Received ChatMessage from {} at {}: {}",
                                                    sender_peer_id, timestamp, message
                                                );
                                                cx.update(|cx| {
                                                    this.update(cx, |this, cx| {
                                                        let is_self = this.current_peer_id.as_ref() == Some(&sender_peer_id);
                                                        tracing::debug!(
                                                            "CREATE_SESSION: Adding chat message. is_self: {}, current chat count: {}",
                                                            is_self, this.chat_messages.len()
                                                        );
                                                        this.chat_messages.push(ChatMessage {
                                                            peer_id: sender_peer_id,
                                                            message,
                                                            timestamp,
                                                            is_self,
                                                        });
                                                        tracing::debug!("CREATE_SESSION: Chat messages now: {}", this.chat_messages.len());
                                                        cx.notify();
                                                    }).ok();
                                                }).ok();
                                            }
                                            ServerMessage::Error { message } => {
                                                cx.update(|cx| {
                                                    this.update(cx, |this, cx| {
                                                        this.connection_status = ConnectionStatus::Error(format!("Server error: {}", message));
                                                        cx.notify();
                                                    }).ok();
                                                }).ok();
                                            }
                                            ServerMessage::RequestFileManifest { from_peer_id, session_id: req_session_id, .. } => {
                                                tracing::debug!("CREATE_SESSION: Received RequestFileManifest from {}", from_peer_id);

                                                // Create and send file manifest
                                                let project_root_result = cx.update(|cx| {
                                                    this.update(cx, |this, _cx| {
                                                        this.project_root.clone()
                                                    }).ok()
                                                }).ok().flatten().flatten();

                                                if let Some(project_root) = project_root_result {
                                                    tracing::debug!("CREATE_SESSION: Project root is {:?}", project_root);

                                                    let our_peer_id_result = cx.update(|cx| {
                                                        this.update(cx, |this, _cx| {
                                                            this.current_peer_id.clone()
                                                        }).ok()
                                                    }).ok().flatten().flatten();

                                                    if let Some(our_peer_id) = our_peer_id_result {
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
                                                                            tracing::debug!("HOST: Serialized manifest to {} bytes", manifest_json.len());
                                                                            tokio::runtime::Runtime::new().unwrap().block_on(async {
                                                                                let client_guard = client_clone.read().await;
                                                                                let _ = client_guard.send(ClientMessage::FileManifest {
                                                                                    session_id: session_id_clone,
                                                                                    peer_id: our_peer_id,
                                                                                    manifest_json,
                                                                                }).await;
                                                                                tracing::debug!("HOST: Sent file manifest");
                                                                            });
                                                                        }
                                                                        Err(e) => tracing::error!("HOST: Failed to serialize manifest: {}", e),
                                                                    }
                                                                }
                                                                Err(e) => tracing::error!("HOST: Failed to create manifest: {}", e),
                                                            }
                                                        });
                                                    } else {
                                                        tracing::error!("CREATE_SESSION: No peer_id available");
                                                    }
                                                } else {
                                                    tracing::error!("CREATE_SESSION: No project_root available");
                                                }
                                            }
                                            ServerMessage::RequestFiles { from_peer_id, file_paths, session_id: req_session_id, .. } => {
                                                tracing::debug!("CREATE_SESSION: Received RequestFiles for {} files from {}", file_paths.len(), from_peer_id);

                                                // Read and send requested files
                                                let project_root_result = cx.update(|cx| {
                                                    this.update(cx, |this, _cx| {
                                                        this.project_root.clone()
                                                    }).ok()
                                                }).ok().flatten().flatten();

                                                if let Some(project_root) = project_root_result {
                                                    let our_peer_id_result = cx.update(|cx| {
                                                        this.update(cx, |this, _cx| {
                                                            this.current_peer_id.clone()
                                                        }).ok()
                                                    }).ok().flatten().flatten();

                                                    if let Some(our_peer_id) = our_peer_id_result {
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
                                                                                let _ = client_guard.send(ClientMessage::FilesChunk {
                                                                                    session_id: session_id_clone,
                                                                                    peer_id: our_peer_id,
                                                                                    files_json,
                                                                                    chunk_index: 0,
                                                                                    total_chunks: 1,
                                                                                }).await;
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
                                                }
                                            }
                                            ServerMessage::RequestFile { from_peer_id, file_path, session_id: req_session_id, .. } => {
                                                tracing::debug!("CREATE_SESSION: Received RequestFile for {} from {}", file_path, from_peer_id);

                                                // Read and send file in chunks
                                                let project_root_result = cx.update(|cx| {
                                                    this.update(cx, |this, _cx| {
                                                        this.project_root.clone()
                                                    }).ok()
                                                }).ok().flatten().flatten();

                                                if let Some(project_root) = project_root_result {
                                                    let full_path = project_root.join(&file_path);

                                                    if let Ok(data) = std::fs::read(&full_path) {
                                                        const CHUNK_SIZE: usize = 8192; // 8KB chunks

                                                        let our_peer_id_result = cx.update(|cx| {
                                                            this.update(cx, |this, _cx| {
                                                                this.current_peer_id.clone()
                                                            }).ok()
                                                        }).ok().flatten().flatten();

                                                        if let Some(our_peer_id) = our_peer_id_result {
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
                                                            tracing::debug!("CREATE_SESSION: Sent file {}", file_path);
                                                        }
                                                    }
                                                }
                                            }
                                            ServerMessage::ReplicationUpdate { from_peer_id, data, .. } => {
                                                tracing::debug!("CREATE_SESSION: Received ReplicationUpdate from {}", from_peer_id);
                                                // Handle replication message and get response
                                                let response = cx.update(|cx| {
                                                    if let Ok(rep_msg) = serde_json::from_str(&data) {
                                                        let integration = ui::replication::MultiuserIntegration::new(cx);
                                                        integration.handle_incoming_message(rep_msg, cx)
                                                    } else {
                                                        None
                                                    }
                                                }).ok().flatten();

                                                // Send response if we got one
                                                if let Some(response) = response {
                                                    let session_id_result = cx.update(|cx| {
                                                        this.update(cx, |this, _cx| {
                                                            this.active_session.as_ref().map(|s| s.session_id.clone())
                                                        }).ok()
                                                    }).ok().flatten().flatten();

                                                    let our_peer_id_result = cx.update(|cx| {
                                                        this.update(cx, |this, _cx| {
                                                            this.current_peer_id.clone()
                                                        }).ok()
                                                    }).ok().flatten().flatten();

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
                                    }
                                }
                                Some(ServerMessage::Error { message }) => {
                                    cx.update(|cx| {
                                        this.update(cx, |this, cx| {
                                            this.connection_status = ConnectionStatus::Error(format!("Server error: {}", message));
                                            this.active_session = None;
                                            cx.notify();
                                        }).ok();
                                    }).ok();
                                }
                                Some(_) => {
                                    cx.update(|cx| {
                                        this.update(cx, |this, cx| {
                                            this.connection_status = ConnectionStatus::Error("Unexpected server response".to_string());
                                            this.active_session = None;
                                            cx.notify();
                                        }).ok();
                                    }).ok();
                                }
                                None => {
                                    cx.update(|cx| {
                                        this.update(cx, |this, cx| {
                                            this.connection_status = ConnectionStatus::Error("Connection closed before response".to_string());
                                            this.active_session = None;
                                            cx.notify();
                                        }).ok();
                                    }).ok();
                                }
                            }
                        }
                        Err(e) => {
                            cx.update(|cx| {
                                this.update(cx, |this, cx| {
                                    this.connection_status = ConnectionStatus::Error(format!("Connection failed: {}", e));
                                    cx.notify();
                                }).ok();
                            }).ok();
                        }
                    }
                }
                Err(e) => {
                    cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            this.connection_status = ConnectionStatus::Error(format!("Failed to create session: {}", e));
                            cx.notify();
                        }).ok();
                    }).ok();
                }
            }
        }).detach();
    }
}
