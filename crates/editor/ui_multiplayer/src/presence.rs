//! User presence tracking functionality

use crate::net::multiuser::ClientMessage;
use crate::screen::MultiplayerWindow;
use crate::utils::types::*;
use gpui::*;

impl MultiplayerWindow {
    /// Kick a user from the session (host only)
    pub(super) fn kick_user(
        &mut self,
        peer_id: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Check if we're the host
        let is_host = self
            .active_session
            .as_ref()
            .and_then(|s| s.connected_users.first())
            .map(|first_peer| Some(first_peer) == self.current_peer_id.as_ref())
            .unwrap_or(false);

        if !is_host {
            tracing::warn!("Only host can kick users");
            return;
        }

        // Don't allow kicking yourself
        if Some(&peer_id) == self.current_peer_id.as_ref() {
            tracing::warn!("Cannot kick yourself");
            return;
        }

        if let (Some(client), Some(session)) = (&self.client, &self.active_session) {
            let client = client.clone();
            let session_id = session.session_id.clone();
            let our_peer_id = self.current_peer_id.clone().unwrap_or_default();

            tracing::debug!("Kicking user {} from session {}", peer_id, session_id);

            // Send kick message to server
            cx.spawn(async move |this, cx| {
                {
                    let client_guard = client.read().await;
                    let _ = client_guard
                        .send(ClientMessage::KickUser {
                            session_id,
                            peer_id: our_peer_id,
                            target_peer_id: peer_id.clone(),
                        })
                        .await;
                }

                // Update local state (will also update when server confirms via PeerLeft)
                cx.update(|cx| {
                    let _ = this.update(cx, |this, cx| {
                        // Remove from local participants list
                        if let Some(session) = &mut this.active_session {
                            session.connected_users.retain(|p| p != &peer_id);
                        }
                        // Remove presence
                        this.user_presences.retain(|p| p.peer_id != peer_id);
                        cx.notify();
                    });
                });
            })
            .detach();
        }
    }

    /// Jump to the tab/file that a user is currently viewing
    pub(super) fn jump_to_user_view(
        &mut self,
        peer_id: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(presence) = self.user_presences.iter().find(|p| p.peer_id == peer_id) {
            // Switch to their current tab if known
            if let Some(tab_name) = &presence.current_tab {
                tracing::debug!("Jumping to {}'s view: tab={}", peer_id, tab_name);

                // Map tab name to SessionTab
                let target_tab = match tab_name.as_str() {
                    "Info" | "info" | "Chat" | "chat" | "Session" | "session" => {
                        Some(SessionTab::Session)
                    }
                    "FileSync" | "file_sync" => Some(SessionTab::FileSync),
                    "Presence" | "presence" => Some(SessionTab::Session),
                    _ => None,
                };

                if let Some(tab) = target_tab {
                    self.current_tab = tab;
                    cx.notify();
                }
            }

            // If they're editing a file, we could open that file in the editor
            if let Some(file_path) = &presence.editing_file {
                tracing::debug!("{} is editing: {}", peer_id, file_path);
                // TODO: Open this file in the main editor
            }

            // If they have a cursor position, we could navigate there
            if let Some(pos) = presence.cursor_position {
                tracing::debug!("{}'s cursor at: {:?}", peer_id, pos);
                // TODO: Move camera/view to this position
            }
        } else {
            tracing::warn!("No presence data for peer {}", peer_id);
        }
    }
}
