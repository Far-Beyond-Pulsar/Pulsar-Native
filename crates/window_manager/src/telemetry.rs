use crate::commands::{WindowCommand, WindowCommandResult};
use crate::validation::errors::WindowError;
use engine_state::{WindowId, WindowRequest};

pub struct TelemetrySender {
}

impl TelemetrySender {
    pub fn new() -> Self {
        Self {}
    }

    pub fn record_window_created(&self, window_id: WindowId, window_type: &WindowRequest) {
        tracing::info!(
            window_id = %window_id,
            window_type = ?window_type,
            "Window created"
        );
    }

    pub fn record_window_closed(&self, window_id: WindowId) {
        tracing::info!(
            window_id = %window_id,
            "Window closed"
        );
    }

    pub fn record_window_focused(&self, window_id: WindowId) {
        tracing::debug!(
            window_id = %window_id,
            "Window focused"
        );
    }

    pub fn record_command_executed(&self, command: &WindowCommand) {
        tracing::debug!(
            command = ?command,
            "Window command executed"
        );
    }

    pub fn record_command_result(&self, result: &WindowCommandResult) {
        tracing::debug!(
            result_window_id = %result.window_id(),
            "Window command result"
        );
    }

    pub fn record_command_failed(&self, command: &WindowCommand, error: &WindowError) {
        tracing::error!(
            command = ?command,
            error = %error,
            "Window command failed"
        );
    }

    pub fn record_validation_failed(&self, command: &WindowCommand, error: &WindowError) {
        tracing::warn!(
            command = ?command,
            error = %error,
            "Window command validation failed"
        );
    }

    pub fn record_hook_failed(&self, hook_name: &str, error: &str) {
        tracing::warn!(
            hook_name = %hook_name,
            error = %error,
            "Window hook failed"
        );
    }

    pub fn record_window_count(&self, count: usize) {
        tracing::trace!(
            count = %count,
            "Window count"
        );
    }
}

impl Default for TelemetrySender {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for TelemetrySender {
    fn clone(&self) -> Self {
        Self::new()
    }
}
