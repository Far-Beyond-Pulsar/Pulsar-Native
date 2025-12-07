//! Connection management for multiplayer sessions

use futures::StreamExt;
use gpui::*;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::state::MultiplayerWindow;
use super::types::*;
use engine_backend::subsystems::networking::simple_sync;
use engine_backend::subsystems::networking::multiuser::{ClientMessage, MultiuserClient, ServerMessage};

impl MultiplayerWindow {
    pub(super) fn create_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let server_address = self.server_address_input.read(cx).text().to_string();

        if server_address.is_empty() {
            self.connection_status = ConnectionStatus::Error("Server address is required".to_string());
            cx.notify();
            return;
        }

        self.connection_status = ConnectionStatus::Connecting;
        cx.notify();

        // Create client
        let client = Arc::new(RwLock::new(MultiuserClient::new(server_address.clone())));
        self.client = Some(client.clone());

        let session_id_input = self.session_id_input.clone();
        let session_password_input = self.session_password_input.clone();

        cx.spawn(async move |this, mut cx| {
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

                                            tracing::info!(
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
                                                tracing::info!("CREATE_SESSION: Project root at {:?}", project_root);
                                            }

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
                                            tracing::info!("CREATE_SESSION: Session disconnected, stopping event loop");
                                            break;
                                        }

                                        match msg {
                                            ServerMessage::PeerJoined { peer_id: joined_peer_id, .. } => {
                                                cx.update(|cx| {
                                                    this.update(cx, |this, cx| {
                                                        tracing::info!(
                                                            "CREATE_SESSION: Received PeerJoined - joined_peer_id: {}, our peer_id: {:?}",
                                                            joined_peer_id,
                                                            this.current_peer_id
                                                        );

                                                        // Ignore PeerJoined about ourselves
                                                        if this.current_peer_id.as_ref() == Some(&joined_peer_id) {
                                                            tracing::info!("CREATE_SESSION: Ignoring PeerJoined about ourselves");
                                                            return;
                                                        }

                                                        if let Some(session) = &mut this.active_session {
                                                            // Add raw peer_id if not already present
                                                            if !session.connected_users.contains(&joined_peer_id) {
                                                                tracing::info!(
                                                                    "CREATE_SESSION: Adding peer {} to list. List before: {:?}",
                                                                    joined_peer_id,
                                                                    session.connected_users
                                                                );
                                                                session.connected_users.push(joined_peer_id.clone());
                                                                // Update presence
                                                                this.update_presence_from_participants(cx);
                                                                cx.notify();
                                                            } else {
                                                                tracing::info!("CREATE_SESSION: Peer {} already in list", joined_peer_id);
                                                            }
                                                        }
                                                    }).ok();
                                                }).ok();
                                            }
                                            ServerMessage::PeerLeft { peer_id: left_peer_id, .. } => {
                                                cx.update(|cx| {
                                                    this.update(cx, |this, cx| {
                                                        if let Some(session) = &mut this.active_session {
                                                            session.connected_users.retain(|p| p != &left_peer_id);
                                                            cx.notify();
                                                        }
                                                    }).ok();
                                                }).ok();
                                            }
                                            ServerMessage::ChatMessage { peer_id: sender_peer_id, message, timestamp, .. } => {
                                                tracing::info!(
                                                    "CREATE_SESSION: Received ChatMessage from {} at {}: {}",
                                                    sender_peer_id, timestamp, message
                                                );
                                                cx.update(|cx| {
                                                    this.update(cx, |this, cx| {
                                                        let is_self = this.current_peer_id.as_ref() == Some(&sender_peer_id);
                                                        tracing::info!(
                                                            "CREATE_SESSION: Adding chat message. is_self: {}, current chat count: {}",
                                                            is_self, this.chat_messages.len()
                                                        );
                                                        this.chat_messages.push(ChatMessage {
                                                            peer_id: sender_peer_id,
                                                            message,
                                                            timestamp,
                                                            is_self,
                                                        });
                                                        tracing::info!("CREATE_SESSION: Chat messages now: {}", this.chat_messages.len());
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
                                                tracing::info!("CREATE_SESSION: Received RequestFileManifest from {}", from_peer_id);

                                                // Create and send file manifest
                                                let project_root_result = cx.update(|cx| {
                                                    this.update(cx, |this, _cx| {
                                                        this.project_root.clone()
                                                    }).ok()
                                                }).ok().flatten().flatten();

                                                if let Some(project_root) = project_root_result {
                                                    tracing::info!("CREATE_SESSION: Project root is {:?}", project_root);
                                                    
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
                                                            tracing::info!("HOST: Creating file manifest for {:?}", project_root);
                                                            match simple_sync::create_manifest(&project_root) {
                                                                Ok(manifest) => {
                                                                    tracing::info!("HOST: Created manifest with {} files", manifest.files.len());
                                                                    match serde_json::to_string(&manifest) {
                                                                        Ok(manifest_json) => {
                                                                            tracing::info!("HOST: Serialized manifest to {} bytes", manifest_json.len());
                                                                            tokio::runtime::Runtime::new().unwrap().block_on(async {
                                                                                let client_guard = client_clone.read().await;
                                                                                let _ = client_guard.send(ClientMessage::FileManifest {
                                                                                    session_id: session_id_clone,
                                                                                    peer_id: our_peer_id,
                                                                                    manifest_json,
                                                                                }).await;
                                                                                tracing::info!("HOST: Sent file manifest");
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
                                                tracing::info!("CREATE_SESSION: Received RequestFiles for {} files from {}", file_paths.len(), from_peer_id);

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
                                                            tracing::info!("HOST: Reading {} files", file_paths.len());
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
                                                                                tracing::info!("HOST: Sent {} files", file_paths.len());
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
                                                tracing::info!("CREATE_SESSION: Received RequestFile for {} from {}", file_path, from_peer_id);

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
                                                            tracing::info!("CREATE_SESSION: Sent file {}", file_path);
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


    pub(super) fn join_session(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let server_address = self.server_address_input.read(cx).text().to_string();
        let session_id = self.session_id_input.read(cx).text().to_string();
        let join_token = self.session_password_input.read(cx).text().to_string();

        if server_address.is_empty() {
            self.connection_status = ConnectionStatus::Error("Server address is required".to_string());
            cx.notify();
            return;
        }

        if session_id.is_empty() || join_token.is_empty() {
            self.connection_status = ConnectionStatus::Error("Session ID and password are required".to_string());
            cx.notify();
            return;
        }

        self.connection_status = ConnectionStatus::Connecting;
        cx.notify();

        // Create client
        let client = Arc::new(RwLock::new(MultiuserClient::new(server_address.clone())));
        self.client = Some(client.clone());

        cx.spawn(async move |this, mut cx| {
            let join_token_clone = join_token.clone();

            let event_rx_result = {
                let mut client_guard = client.write().await;
                client_guard.connect(session_id.clone(), join_token_clone).await
            }; // client_guard dropped here, releasing the lock

            match event_rx_result {
                Ok(mut event_rx) => {
                    // Wait for the initial response (Joined or Error)
                    match event_rx.recv().await {
                        Some(ServerMessage::Joined { peer_id, participants, .. }) => {
                            cx.update(|cx| {
                                this.update(cx, |this, cx| {
                                    this.connection_status = ConnectionStatus::Connected;
                                    // Store our peer_id first
                                    this.current_peer_id = Some(peer_id.clone());

                                    tracing::info!(
                                        "JOIN_SESSION: Received Joined - our peer_id: {}, participants: {:?}",
                                        peer_id,
                                        participants
                                    );

                                    this.active_session = Some(ActiveSession {
                                        session_id: session_id.clone(),
                                        join_token: join_token.clone(),
                                        server_address: server_address.clone(),
                                        // Store raw participant list
                                        connected_users: participants.clone(),
                                    });

                                    // Initialize presence for all participants
                                    this.update_presence_from_participants(cx);

                                    // Log project root
                                    if let Some(project_root) = &this.project_root {
                                        tracing::info!("JOIN_SESSION: Project root at {:?}", project_root);
                                    }

                                    cx.notify();
                                }).ok();
                            }).ok();

                            // Request file manifest from host (first participant)
                            if let Some(host_peer_id) = participants.first() {
                                tracing::info!("JOIN_SESSION: Requesting file manifest from host {}", host_peer_id);

                                let our_peer_id_result = cx.update(|cx| {
                                    this.update(cx, |this, _cx| {
                                        this.current_peer_id.clone()
                                    }).ok()
                                }).ok().flatten().flatten();

                                if let Some(our_peer_id) = our_peer_id_result {
                                    let client_guard = client.read().await;
                                    let _ = client_guard.send(ClientMessage::RequestFileManifest {
                                        session_id: session_id.clone(),
                                        peer_id: our_peer_id,
                                    }).await;
                                    tracing::info!("JOIN_SESSION: Sent RequestFileManifest");
                                }
                            }

                            // Continue listening for PeerJoined/PeerLeft events
                            while let Some(msg) = event_rx.recv().await {
                                // Check if we're still connected - break if disconnected
                                let still_connected = cx.update(|cx| {
                                    this.update(cx, |this, _cx| {
                                        this.active_session.is_some()
                                    }).unwrap_or(false)
                                }).unwrap_or(false);

                                if !still_connected {
                                    tracing::info!("JOIN_SESSION: Session disconnected, stopping event loop");
                                    break;
                                }

                                match msg {
                                    ServerMessage::PeerJoined { peer_id: joined_peer_id, .. } => {
                                        cx.update(|cx| {
                                            this.update(cx, |this, cx| {
                                                tracing::info!(
                                                    "JOIN_SESSION: Received PeerJoined - joined_peer_id: {}, our peer_id: {:?}",
                                                    joined_peer_id,
                                                    this.current_peer_id
                                                );

                                                // Ignore PeerJoined about ourselves
                                                if this.current_peer_id.as_ref() == Some(&joined_peer_id) {
                                                    tracing::info!("JOIN_SESSION: Ignoring PeerJoined about ourselves");
                                                    return;
                                                }

                                                if let Some(session) = &mut this.active_session {
                                                    // Add raw peer_id if not already present
                                                    if !session.connected_users.contains(&joined_peer_id) {
                                                        tracing::info!(
                                                            "JOIN_SESSION: Adding peer {} to list. List before: {:?}",
                                                            joined_peer_id,
                                                            session.connected_users
                                                        );
                                                        session.connected_users.push(joined_peer_id);
                                                        cx.notify();
                                                    } else {
                                                        tracing::info!("JOIN_SESSION: Peer {} already in list", joined_peer_id);
                                                    }
                                                }
                                            }).ok();
                                        }).ok();
                                    }
                                    ServerMessage::PeerLeft { peer_id: left_peer_id, .. } => {
                                        cx.update(|cx| {
                                            this.update(cx, |this, cx| {
                                                if let Some(session) = &mut this.active_session {
                                                    session.connected_users.retain(|p| p != &left_peer_id);
                                                    cx.notify();
                                                }
                                            }).ok();
                                        }).ok();
                                    }
                                    ServerMessage::ChatMessage { peer_id: sender_peer_id, message, timestamp, .. } => {
                                        tracing::info!(
                                            "JOIN_SESSION: Received ChatMessage from {} at {}: {}",
                                            sender_peer_id, timestamp, message
                                        );
                                        cx.update(|cx| {
                                            this.update(cx, |this, cx| {
                                                let is_self = this.current_peer_id.as_ref() == Some(&sender_peer_id);
                                                tracing::info!(
                                                    "JOIN_SESSION: Adding chat message. is_self: {}, current chat count: {}",
                                                    is_self, this.chat_messages.len()
                                                );
                                                this.chat_messages.push(ChatMessage {
                                                    peer_id: sender_peer_id,
                                                    message,
                                                    timestamp,
                                                    is_self,
                                                });
                                                tracing::info!("JOIN_SESSION: Chat messages now: {}", this.chat_messages.len());
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
                                    ServerMessage::FileManifest { from_peer_id, manifest_json, .. } => {
                                        tracing::info!("JOIN_SESSION: Received file manifest from {} ({} bytes)", 
                                            from_peer_id, manifest_json.len());

                                        // Parse manifest and compute diff
                                        let project_root_result = cx.update(|cx| {
                                            this.update(cx, |this, _cx| {
                                                this.project_root.clone()
                                            }).ok()
                                        }).ok().flatten().flatten();

                                        if let Some(project_root) = project_root_result {
                                            tracing::info!("JOIN_SESSION: Project root is {:?}", project_root);
                                            
                                            // Deserialize manifest
                                            match serde_json::from_str::<simple_sync::FileManifest>(&manifest_json) {
                                                Ok(remote_manifest) => {
                                                    tracing::info!("JOIN_SESSION: Parsed manifest with {} files", remote_manifest.files.len());

                                                    // Compute diff synchronously (fast enough for most cases)
                                                    match simple_sync::compute_diff(&project_root, &remote_manifest) {
                                                        Ok(diff) => {
                                                            tracing::info!("JOIN_SESSION: Computed diff - {}", diff.summary());

                                                            if diff.has_changes() {
                                                                tracing::info!("JOIN_SESSION: Changes detected, showing sync UI");

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
                                                                        tracing::info!("JOIN_SESSION: Set pending_file_sync and switched to FileSync tab");
                                                                        cx.notify();
                                                                    }).ok()
                                                                }).ok();

                                                                // Request file contents for preview (don't write to disk yet)
                                                                if !files_to_preview.is_empty() {
                                                                    tracing::info!("JOIN_SESSION: Requesting {} files for preview", files_to_preview.len());

                                                                    let client_guard = client.read().await;
                                                                    let session_id_result = cx.update(|cx| {
                                                                        this.update(cx, |this, _cx| {
                                                                            this.active_session.as_ref().map(|s| s.session_id.clone())
                                                                        }).ok()
                                                                    }).ok().flatten().flatten();

                                                                    let peer_id_result = cx.update(|cx| {
                                                                        this.update(cx, |this, _cx| {
                                                                            this.current_peer_id.clone()
                                                                        }).ok()
                                                                    }).ok().flatten().flatten();

                                                                    if let (Some(session_id), Some(peer_id)) = (session_id_result, peer_id_result) {
                                                                        let _ = client_guard.send(ClientMessage::RequestFiles {
                                                                            session_id,
                                                                            peer_id,
                                                                            file_paths: files_to_preview,
                                                                        }).await;
                                                                    }
                                                                }
                                                            } else {
                                                                tracing::info!("JOIN_SESSION: No changes needed, already in sync");
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
                                        tracing::info!("JOIN_SESSION: Received FileChunk for {} from {} (offset: {}, is_last: {})",
                                            file_path, from_peer_id, offset, is_last);

                                        // Write chunk to file
                                        let project_root_result = cx.update(|cx| {
                                            this.update(cx, |this, _cx| {
                                                this.project_root.clone()
                                            }).ok()
                                        }).ok().flatten().flatten();

                                        if let Some(project_root) = project_root_result {
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
                                                tracing::info!("JOIN_SESSION: Completed download of {}", file_path);
                                            }
                                        }
                                    }
                                    ServerMessage::FilesChunk { from_peer_id, files_json, chunk_index, total_chunks, .. } => {
                                        tracing::info!("JOIN_SESSION: Received FilesChunk from {} (chunk {}/{})",
                                            from_peer_id, chunk_index + 1, total_chunks);

                                        // Set progress indicator - receiving data
                                        cx.update(|cx| {
                                            this.update(cx, |this, cx| {
                                                this.sync_progress_message = Some("Receiving files...".to_string());
                                                this.sync_progress_percent = Some(0.1);
                                                cx.notify();
                                            }).ok();
                                        }).ok();

                                        // For now, handle single chunk only
                                        // TODO: Handle multi-chunk transfers by buffering
                                        if chunk_index == 0 && total_chunks == 1 {
                                            let project_root_result = cx.update(|cx| {
                                                this.update(cx, |this, _cx| {
                                                    this.project_root.clone()
                                                }).ok()
                                            }).ok().flatten().flatten();

                                            if let Some(project_root) = project_root_result {
                                                // Update progress - starting sync
                                                cx.update(|cx| {
                                                    this.update(cx, |this, cx| {
                                                        this.sync_progress_message = Some("Processing files...".to_string());
                                                        this.sync_progress_percent = Some(0.2);
                                                        cx.notify();
                                                    }).ok();
                                                }).ok();

                                                // Deserialize and apply files synchronously (fast enough for most cases)
                                                tracing::info!("SYNC_TASK: Deserializing files from JSON ({} bytes)", files_json.len());
                                                match serde_json::from_str::<Vec<(String, Vec<u8>)>>(&files_json) {
                                                    Ok(files_data) => {
                                                        tracing::info!("SYNC_TASK: Successfully deserialized {} files", files_data.len());

                                                        // Update FileSyncUI with file contents for preview
                                                        for (file_path, data) in &files_data {
                                                            if let Ok(content) = String::from_utf8(data.clone()) {
                                                                let file_path_clone = file_path.clone();
                                                                let content_clone = content.clone();
                                                                cx.update(|cx| {
                                                                    this.update(cx, |this, cx| {
                                                                        this.queue_file_content_update(file_path_clone, content_clone, cx);
                                                                    }).ok()
                                                                }).ok();
                                                            }
                                                        }

                                                        // Apply files using simple_sync
                                                        match simple_sync::apply_files(&project_root, files_data) {
                                                            Ok(written_count) => {
                                                                tracing::info!("SYNC_SUCCESS: Applied {} files successfully", written_count);

                                                                // Update UI
                                                                cx.update(|cx| {
                                                                    this.update(cx, |this, cx| {
                                                                        tracing::info!("SYNC_SUCCESS: Updating UI state - clearing progress and pending sync");
                                                                        this.file_sync_in_progress = false;
                                                                        this.pending_file_sync = None;
                                                                        this.sync_progress_message = Some(format!("Synced {} files successfully", written_count));
                                                                        this.sync_progress_percent = Some(1.0);

                                                                        tracing::info!("SYNC_SUCCESS: State updated - file_sync_in_progress={}, pending_file_sync={:?}",
                                                                            this.file_sync_in_progress, this.pending_file_sync.is_some());
                                                                        cx.notify();
                                                                    }).ok()
                                                                }).ok();

                                                                tracing::info!("SYNC_SUCCESS: File synchronization complete!");
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
                                                                }).ok();
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
                                                        }).ok();
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    ServerMessage::RequestFileManifest { from_peer_id, session_id: req_session_id, .. } => {
                                        tracing::info!("JOIN_SESSION: Received RequestFileManifest from {}", from_peer_id);

                                        // Create and send file manifest (same logic as in CREATE_SESSION)
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
                                                // Create manifest in background thread
                                                let client_clone = client.clone();
                                                let session_id_clone = req_session_id.clone();
                                                std::thread::spawn(move || {
                                                    tracing::info!("PEER: Creating file manifest");
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
                                                                        tracing::info!("PEER: Sent file manifest");
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
                                        tracing::info!("JOIN_SESSION: Received RequestFiles for {} files from {}", file_paths.len(), from_peer_id);

                                        // Read and send requested files (same logic as in CREATE_SESSION)
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
                                                    tracing::info!("PEER: Reading {} files", file_paths.len());
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
                                                                        tracing::info!("PEER: Sent {} files", file_paths.len());
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
                                        tracing::info!("JOIN_SESSION: Received RequestFile for {} from {}", file_path, from_peer_id);

                                        // Read and send file in chunks
                                        let project_root_result = cx.update(|cx| {
                                            this.update(cx, |this, _cx| {
                                                this.project_root.clone()
                                            }).ok()
                                        }).ok().flatten().flatten();

                                        if let Some(project_root) = project_root_result {
                                            let full_path = project_root.join(&file_path);

                                            if let Ok(data) = std::fs::read(&full_path) {
                                                const CHUNK_SIZE: usize = 8192;

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
                                    cx.notify();
                                }).ok();
                            }).ok();
                        }
                        Some(_) => {
                            cx.update(|cx| {
                                this.update(cx, |this, cx| {
                                    this.connection_status = ConnectionStatus::Error("Unexpected server response".to_string());
                                    cx.notify();
                                }).ok();
                            }).ok();
                        }
                        None => {
                            cx.update(|cx| {
                                this.update(cx, |this, cx| {
                                    this.connection_status = ConnectionStatus::Error("Connection closed before response".to_string());
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
        }).detach();
    }


    pub(super) fn disconnect(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let (Some(client), Some(session)) = (&self.client, &self.active_session) {
            let client = client.clone();
            let session_id = session.session_id.clone();
            let peer_id = self.current_peer_id.clone().unwrap_or_default();

            cx.spawn(async move |this, mut cx| {
                let mut client_guard = client.write().await;
                let _ = client_guard.disconnect(session_id, peer_id).await;

                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.connection_status = ConnectionStatus::Disconnected;
                        this.active_session = None;
                        this.client = None;
                        this.current_peer_id = None;
                        this.chat_messages.clear();
                        this.current_tab = SessionTab::Info;
                        cx.notify();
                    }).ok();
                }).ok();
            }).detach();
        } else {
            self.connection_status = ConnectionStatus::Disconnected;
            self.active_session = None;
            self.client = None;
            self.current_peer_id = None;
            self.chat_messages.clear();
            self.current_tab = SessionTab::Info;
            cx.notify();
        }
    }

}
