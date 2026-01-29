//! Extensions for adding replication to existing UI components
//!
//! These extension traits add replication capabilities to existing
//! components without modifying their original implementations.

use super::{
    Replicator, ReplicationConfig, ReplicationMode, ReplicationRegistry, SessionContext,
    ReplicationMessageBuilder,
};
use crate::input::{InputState, RopeExt};
use gpui::{App, Window};
use serde_json::{json, Value};

/// Extension trait for InputState to add replication support
///
/// This demonstrates how to add replication to an existing component
/// without modifying its original source code.
///
/// # Usage
///
/// ```ignore
/// use ui::replication::InputStateReplicationExt;
///
/// let input = cx.new(|cx| InputState::new(window, cx));
///
/// // Enable replication
/// input.enable_replication(ReplicationMode::MultiEdit, cx);
///
/// // Sync when value changes
/// input.update(cx, |state, cx| {
///     state.set_value("new value".to_string(), window, cx);
///     state.sync_if_replicated(cx);
/// });
/// ```
pub trait InputStateReplicationExt {
    /// Enable replication for this input
    fn enable_replication(&self, mode: ReplicationMode, cx: &mut App);

    /// Sync state if replication is enabled
    fn sync_if_replicated(&self, cx: &mut App);

    /// Get the replication mode for this input
    fn replication_mode(&self, cx: &App) -> Option<ReplicationMode>;

    /// Check if this input can be edited based on replication rules
    fn can_edit_replicated(&self, cx: &App) -> bool;

    /// Handle a remote state update for this input
    fn apply_remote_state(&self, state: Value, window: &mut Window, cx: &mut App) -> Result<(), String>;
}

impl InputStateReplicationExt for gpui::Entity<InputState> {
    fn enable_replication(&self, mode: ReplicationMode, cx: &mut App) {
        let element_id = format!("input_{}", self.entity_id());
        let config = ReplicationConfig::new(mode)
            .with_debounce(100)
            .with_presence(true)
            .with_cursors(true);

        let registry = ReplicationRegistry::global(cx);
        registry.register_element(element_id, config);

        tracing::debug!("Enabled replication for input {:?}", self.entity_id());
    }

    fn sync_if_replicated(&self, cx: &mut App) {
        let element_id = format!("input_{}", self.entity_id());
        let registry = ReplicationRegistry::global(cx);

        // Check if this input is replicated
        if let Some(_elem_state) = registry.get_element_state(&element_id) {
            let session = SessionContext::global(cx);

            // Serialize current state
            let text_rope = self.read(cx).text();
            let cursor_pos = self.read(cx).cursor();

            let state = json!({
                "text": text_rope.to_string(),
                "cursor": cursor_pos,
            });

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;

            // Update registry
            registry.update_element_state(&element_id, state.clone(), timestamp);

            // Send to network if in session
            if session.is_active() {
                if let Some(our_peer_id) = session.our_peer_id() {
                    let message = ReplicationMessageBuilder::state_update(
                        element_id,
                        state,
                        our_peer_id,
                    );
                    session.send_message(message);
                }
            }
        }
    }

    fn replication_mode(&self, cx: &App) -> Option<ReplicationMode> {
        let element_id = format!("input_{}", self.entity_id());
        let registry = ReplicationRegistry::global(cx);

        registry
            .get_element_state(&element_id)
            .map(|state| state.config.mode)
    }

    fn can_edit_replicated(&self, cx: &App) -> bool {
        let element_id = format!("input_{}", self.entity_id());
        let registry = ReplicationRegistry::global(cx);
        let session = SessionContext::global(cx);

        // If not in a session, allow editing
        if !session.is_active() {
            return true;
        }

        // If not replicated, allow editing
        let elem_state = match registry.get_element_state(&element_id) {
            Some(state) => state,
            None => return true,
        };

        let our_peer_id = match session.our_peer_id() {
            Some(id) => id,
            None => return false,
        };

        // Check based on replication mode
        match elem_state.config.mode {
            ReplicationMode::NoRep => true,
            ReplicationMode::MultiEdit => {
                if let Some(max) = elem_state.config.max_concurrent_editors {
                    elem_state.active_editors.len() < max
                        || elem_state.active_editors.contains(&our_peer_id)
                } else {
                    true
                }
            }
            ReplicationMode::LockedEdit => {
                elem_state.locked_by.is_none()
                    || elem_state.locked_by.as_ref() == Some(&our_peer_id)
            }
            ReplicationMode::RequestEdit => elem_state.active_editors.contains(&our_peer_id),
            ReplicationMode::BroadcastOnly => session.are_we_host(),
            _ => true,
        }
    }

    fn apply_remote_state(&self, state: Value, window: &mut Window, cx: &mut App) -> Result<(), String> {
        let text = state
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or("Missing text field")?;

        let cursor = state
            .get("cursor")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        // Update the input state
        self.update(cx, |input_state, cx| {
            input_state.set_value(text.to_string(), window, cx);

            if let Some(cursor_pos) = cursor {
                // Convert offset to Position
                let position = input_state.text().offset_to_position(cursor_pos);
                input_state.set_cursor_position(position, window, cx);
            }
        });

        Ok(())
    }
}

/// Create and configure a replicated input
///
/// This shows the pattern for creating a replicated input.
/// You must call this from within a context that can create entities
/// (like within a component's constructor or window callback).
///
/// # Example
///
/// ```ignore
/// use ui::replication::{InputStateReplicationExt, ReplicationMode};
///
/// // Within a component or window context where you have &mut Context<Self>:
/// let input = cx.new(|cx| InputState::new(window, cx));
///
/// // Enable replication
/// input.enable_replication(ReplicationMode::MultiEdit, cx);
///
/// // Now sync after changes:
/// input.update(cx, |state, cx| {
///     state.set_value("new_value".to_string(), window, cx);
/// });
/// input.sync_if_replicated(cx);
/// ```
pub fn create_replicated_input_pattern() {
    // This is just documentation - see the example above
    // There's no way to create entities from arbitrary App contexts,
    // you must use the pattern shown in the example
}
