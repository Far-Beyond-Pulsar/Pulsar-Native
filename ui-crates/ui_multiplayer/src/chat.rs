//! Chat functionality for multiplayer sessions

use gpui::*;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::state::MultiplayerWindow;
use super::types::*;
use engine_backend::subsystems::networking::multiuser::{ClientMessage, MultiuserClient};

impl MultiplayerWindow {
    pub(super) fn send_chat_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let message = self.chat_input.read(cx).text().to_string();
        if message.trim().is_empty() {
            tracing::warn!("Attempted to send empty chat message");
            return;
        }

        tracing::debug!("Preparing to send chat message: {}", message);

        if let (Some(client), Some(session), Some(peer_id)) = (&self.client, &self.active_session, &self.current_peer_id) {
            let client = client.clone();
            let session_id = session.session_id.clone();
            let peer_id = peer_id.clone();
            let message_to_send = message.clone();

            tracing::debug!("Sending chat message from peer {} in session {}", peer_id, session_id);

            // Clear input immediately
            self.chat_input.update(cx, |state, cx| {
                state.set_value("".to_string(), window, cx);
            });

            cx.spawn(async move |_this, _cx| {
                let client_guard = client.read().await;
                match client_guard.send(ClientMessage::ChatMessage {
                    session_id: session_id.clone(),
                    peer_id: peer_id.clone(),
                    message: message_to_send.clone(),
                }).await {
                    Ok(_) => {
                        tracing::debug!("Successfully sent chat message to server");
                    }
                    Err(e) => {
                        tracing::error!("Failed to send chat message: {}", e);
                    }
                }
            }).detach();
        } else {
            tracing::error!("Cannot send chat message - client, session, or peer_id is None");
        }
    }

}
