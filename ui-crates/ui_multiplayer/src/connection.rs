//! Connection lifecycle - disconnect handling

//! Session creation (host path) for multiplayer sessions

use gpui::*;

use super::state::MultiplayerWindow;
use super::types::*;

impl MultiplayerWindow {
    pub(super) fn disconnect(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let (Some(client), Some(session)) = (&self.client, &self.active_session) {
            let client = client.clone();
            let session_id = session.session_id.clone();
            let peer_id = self.current_peer_id.clone().unwrap_or_default();

            cx.spawn(async move |this, cx| {
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
                        this.sync_engine_multiuser_disconnected();
                        cx.notify();
                    });
                });
            })
            .detach();
        } else {
            self.connection_status = ConnectionStatus::Disconnected;
            self.active_session = None;
            self.client = None;
            self.current_peer_id = None;
            self.chat_messages.clear();
            self.current_tab = SessionTab::Info;
            self.sync_engine_multiuser_disconnected();
            cx.notify();
        }
    }

    pub(super) fn handle_kicked(&mut self, reason: String, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let session_id = self.active_session.as_ref().map(|s| s.session_id.clone());
        let peer_id = self.current_peer_id.clone();
        self.connection_status = ConnectionStatus::Error(reason.clone());
        self.sync_engine_multiuser_error(reason.clone());

        cx.spawn(async move |this, cx| {
            if let Some(client) = client {
                let mut client_guard = client.write().await;
                client_guard.force_disconnect().await;
            }

            cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.active_session = None;
                    this.client = None;
                    this.current_peer_id = None;
                    this.chat_messages.clear();
                    this.user_presences.clear();
                    this.current_tab = SessionTab::Info;
                    this.file_sync_in_progress = false;
                    this.sync_progress_message = None;
                    this.sync_progress_percent = None;
                    this.pending_file_sync = None;
                    this.pending_diff_populate = None;
                    this.pending_file_updates.clear();
                    this.fs_event_forwarder = None;
                    if let (Some(session_id), Some(peer_id)) = (session_id, peer_id) {
                        let integration = ui::replication::MultiuserIntegration::new(cx);
                        tracing::debug!(
                            "Kicked from session {}; ended local replication session for {}",
                            session_id,
                            peer_id
                        );
                        integration.end_session(cx);
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}
