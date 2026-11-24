//! Routing of the client messages `handle_client_message` hands on: host
//! requests, session messages and the transport relays.

use anyhow::Result;

use super::{ClientMessage, RendezvousCoordinator};

impl RendezvousCoordinator {
    /// A guest's request for something the host holds, forwarded to the host.
    pub(super) async fn forward_request_to_host(&self, msg: ClientMessage) -> Result<()> {
        match msg {
            ClientMessage::RequestFileManifest {
                session_id: sid,
                peer_id: pid,
            } => {
                self.forward_to_host(
                    &sid,
                    super::ServerMessage::RequestFileManifest {
                        session_id: sid.clone(),
                        from_peer_id: pid,
                    },
                )
                .await
            }
            ClientMessage::RequestFiles {
                session_id: sid,
                peer_id: pid,
                file_paths,
            } => {
                self.forward_to_host(
                    &sid,
                    super::ServerMessage::RequestFiles {
                        session_id: sid.clone(),
                        from_peer_id: pid,
                        file_paths,
                    },
                )
                .await
            }
            ClientMessage::RequestProjectTree {
                session_id: sid,
                peer_id: pid,
            } => {
                self.forward_to_host(
                    &sid,
                    super::ServerMessage::RequestProjectTree {
                        session_id: sid.clone(),
                        from_peer_id: pid,
                    },
                )
                .await
            }
            ClientMessage::RequestFile {
                session_id: sid,
                peer_id: pid,
                file_path,
            } => {
                self.forward_to_host(
                    &sid,
                    super::ServerMessage::RequestFile {
                        session_id: sid.clone(),
                        from_peer_id: pid,
                        file_path,
                    },
                )
                .await
            }
            other => anyhow::bail!("{other:?} is not a request for the host"),
        }
    }

    /// Leaving, kicking, chat and the file manifest and change messages.
    pub(super) async fn handle_session_message(&self, msg: ClientMessage) -> Result<()> {
        match msg {
            ClientMessage::Leave {
                session_id: sid,
                peer_id: pid,
            } => self.handle_leave(&sid, &pid).await,
            ClientMessage::KickUser {
                session_id: sid,
                peer_id: pid,
                target_peer_id,
            } => self.handle_kick(&sid, &pid, target_peer_id).await,
            ClientMessage::ChatMessage {
                session_id: sid,
                peer_id: pid,
                message,
            } => self.relay_chat_message(&sid, &pid, message).await,
            ClientMessage::FileManifest {
                session_id: sid,
                peer_id: pid,
                manifest_json,
            } => self.relay_file_manifest(&sid, &pid, manifest_json).await,
            ClientMessage::FilesChunk {
                session_id: sid,
                peer_id: pid,
                files_json,
                chunk_index,
                total_chunks,
            } => {
                self.relay_files_chunk(&sid, &pid, files_json, chunk_index, total_chunks)
                    .await
            }
            ClientMessage::FileChanged {
                session_id: sid,
                peer_id: pid,
                path,
                kind,
            } => self.relay_file_changed(&sid, &pid, path, kind).await,
            other => anyhow::bail!("{other:?} is not a session message"),
        }
    }

    /// The peer-to-peer connection, binary proxy, project tree and file chunk
    /// messages a peer relays to the others.
    pub(super) async fn relay_transport_message(&self, msg: ClientMessage) -> Result<()> {
        match msg {
            ClientMessage::P2PConnectionRequest {
                session_id: sid,
                peer_id: pid,
                public_ip,
                public_port,
            } => {
                self.relay_p2p_request(&sid, &pid, public_ip, public_port)
                    .await
            }
            ClientMessage::P2PConnectionResponse {
                session_id: sid,
                peer_id: pid,
                public_ip,
                public_port,
            } => {
                self.relay_p2p_response(&sid, &pid, public_ip, public_port)
                    .await
            }
            ClientMessage::RequestBinaryProxy {
                session_id: sid,
                peer_id: pid,
            } => self.relay_binary_proxy_request(&sid, &pid).await,
            ClientMessage::BinaryProxyData {
                session_id: sid,
                peer_id: pid,
                sequence,
                is_git_protocol,
            } => {
                self.relay_binary_proxy_data(&sid, &pid, sequence, is_git_protocol)
                    .await
            }
            ClientMessage::ProjectTreeResponse {
                session_id: sid,
                peer_id: pid,
                tree_json,
            } => self.relay_project_tree(&sid, &pid, tree_json).await,
            ClientMessage::FileChunk {
                session_id: sid,
                peer_id: pid,
                file_path,
                offset,
                data,
                is_last,
            } => {
                self.relay_file_chunk(&sid, &pid, file_path, offset, data, is_last)
                    .await
            }
            other => anyhow::bail!("{other:?} is not a relayed transport message"),
        }
    }
}
