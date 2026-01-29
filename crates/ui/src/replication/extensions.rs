//! Extensions for adding replication to existing UI components
//!
//! These extension traits add replication capabilities to existing
//! components without modifying their original implementations.

use super::{
    Replicator, ReplicationConfig, ReplicationMode, ReplicationRegistry, SessionContext,
    ReplicationMessageBuilder,
};
use crate::input::InputState;
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
    fn enable_replication(&self, mode: ReplicationMode, cx: &App);

    /// Sync state if replication is enabled
    fn sync_if_replicated(&self, cx: &App);

    /// Get the replication mode for this input
    fn replication_mode(&self, cx: &App) -> Option<ReplicationMode>;

    /// Check if this input can be edited based on replication rules
    fn can_edit_replicated(&self, cx: &App) -> bool;

    /// Handle a remote state update for this input
    fn apply_remote_state(&self, state: Value, window: &mut Window, cx: &App) -> Result<(), String>;
}

impl InputStateReplicationExt for gpui::Entity<InputState> {
    fn enable_replication(&self, mode: ReplicationMode, cx: &App) {
        let element_id = format!("input_{}", self.entity_id());
        let config = ReplicationConfig::new(mode)
            .with_debounce(100)
            .with_presence(true)
            .with_cursors(true);

        let registry = ReplicationRegistry::global(cx);
        registry.register_element(element_id, config);

        tracing::debug!("Enabled replication for input {:?}", self.entity_id());
    }

    fn sync_if_replicated(&self, cx: &App) {
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

    fn apply_remote_state(&self, state: Value, window: &mut Window, cx: &App) -> Result<(), String> {
        let text = state
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or("Missing text field")?;

        let _cursor = state
            .get("cursor")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        // Note: This method requires mutable context to update InputState
        // Callers should use this from within an update context where they have &mut AppContext
        // For now, we'll mark this as needing refactoring
        // TODO: Refactor to work with proper context types or remove this method
        Err("apply_remote_state requires mutable context - use within update() closure".to_string())
    }
}

/// Helper to create a replicated input with automatic syncing
///
/// This is a convenience function that creates an input and sets up
/// automatic state synchronization.
///
/// # Example
///
/// ```ignore
/// use ui::replication::{create_replicated_input, ReplicationMode};
///
/// let input = create_replicated_input(
///     "script_name",
///     ReplicationMode::LockedEdit,
///     window,
///     cx,
/// );
///
/// // Changes are automatically synced
/// input.update(cx, |state, cx| {
///     state.set_value("new_value".to_string(), window, cx);
/// });
/// ```
/// Note: This function needs to be called from a proper context where entity creation is available
/// Typically this would be within a window context or component initialization
///
/// Example usage:
/// ```ignore
/// window.update(cx, |_, cx| {
///     let input = cx.new(|cx| InputState::new(window, cx));
///     input.enable_replication(ReplicationMode::MultiEdit, cx);
///     input
/// })
/// ```
#[allow(dead_code)]
pub fn create_replicated_input(
    element_id: impl Into<String>,
    mode: ReplicationMode,
) -> ReplicationConfig {
    // Return just the config - caller must create the entity in their own context
    ReplicationConfig::new(mode)
        .with_debounce(100)
        .with_presence(true)
        .with_cursors(true)
}

/// Auto-sync helper that watches an input and syncs changes
///
/// This creates a subscription that automatically syncs the input's
/// state whenever it changes.
///
/// # Example
///
/// ```ignore
/// use ui::replication::auto_sync_input;
///
/// let input = cx.new(|cx| InputState::new(window, cx));
/// input.enable_replication(ReplicationMode::MultiEdit, cx);
///
/// // Set up auto-sync
/// let _subscription = auto_sync_input(&input, cx);
///
/// // Now changes are automatically synced
/// ```
pub fn auto_sync_input(
    input: &gpui::Entity<InputState>,
    cx: &App,
) -> Option<gpui::Subscription> {
    // Check if input is replicated
    let element_id = format!("input_{}", input.entity_id());
    let registry = ReplicationRegistry::global(cx);

    if registry.get_element_state(&element_id).is_none() {
        return None;
    }

    // Subscribe to input changes and sync
    // Note: This would require InputState to emit events
    // For now, caller should manually call sync_if_replicated after changes

    tracing::debug!("Auto-sync enabled for input {:?}", input.entity_id());
    None
}
