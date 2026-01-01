//! Simple hash-based file synchronization functionality

use gpui::*;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::state::MultiplayerWindow;
use super::types::*;
use engine_backend::subsystems::networking::multiuser::{ClientMessage, MultiuserClient};

impl MultiplayerWindow {
    pub(super) fn approve_file_sync(&mut self, cx: &mut Context<Self>) {
        if let Some((diff, host_peer_id)) = self.pending_file_sync.take() {
            tracing::debug!(
                "File sync approved - requesting {} files from {}",
                diff.change_count(),
                host_peer_id
            );

            self.file_sync_in_progress = true;
            self.sync_progress_message = Some("Requesting files from host...".to_string());
            self.sync_progress_percent = Some(0.0);

            // Collect all files we need
            let mut files_needed = Vec::new();
            files_needed.extend(diff.files_to_add.clone());
            files_needed.extend(diff.files_to_update.clone());

            // Request files from host
            if let (Some(client), Some(session), Some(peer_id)) =
                (&self.client, &self.active_session, &self.current_peer_id)
            {
                let client = client.clone();
                let session_id = session.session_id.clone();
                let peer_id = peer_id.clone();

                cx.spawn(async move |this, mut cx| {
                    tracing::debug!("Sending RequestFiles for {} files", files_needed.len());

                    let client_guard = client.read().await;
                    let _ = client_guard.send(ClientMessage::RequestFiles {
                        session_id,
                        peer_id,
                        file_paths: files_needed,
                    }).await;

                    tracing::debug!("File request sent, waiting for response...");
                }).detach();
            }

            self.pending_file_sync = None;
            cx.notify();
        }
    }

    pub(super) fn cancel_file_sync(&mut self, cx: &mut Context<Self>) {
        tracing::debug!("Git sync cancelled");
        self.pending_file_sync = None;
        cx.notify();
    }
}
