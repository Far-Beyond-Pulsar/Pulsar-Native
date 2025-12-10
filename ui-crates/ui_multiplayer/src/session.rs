//! Session management functionality

use super::state::MultiplayerWindow;
use super::types::*;

impl MultiplayerWindow {
    /// Format participant IDs for display with nice labels
    pub(super) fn format_participants(&self, participants: &[String]) -> Vec<String> {
        let our_peer_id = match &self.current_peer_id {
            Some(id) => id,
            None => return participants.iter().map(|p| Self::shorten_peer_id(p)).collect(),
        };

        let is_host = self.active_session.as_ref()
            .map(|s| participants.first() == Some(our_peer_id))
            .unwrap_or(false);

        participants.iter().enumerate().map(|(index, p)| {
            if p == our_peer_id {
                // Current user
                if is_host {
                    "You (Host)".to_string()
                } else {
                    "You (Guest)".to_string()
                }
            } else {
                // Other users - show their role and shortened ID
                let short_id = Self::shorten_peer_id(p);
                if index == 0 {
                    // First participant is always the host
                    format!("{} (Host)", short_id)
                } else {
                    format!("{} (Guest)", short_id)
                }
            }
        }).collect()
    }

    /// Shorten a peer ID for display (show first 8 characters)
    fn shorten_peer_id(peer_id: &str) -> String {
        if peer_id.len() <= 8 {
            peer_id.to_string()
        } else {
            format!("{}...", &peer_id[..8])
        }
    }

}
