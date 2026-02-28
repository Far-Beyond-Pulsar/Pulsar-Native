//! Window close event handling
//!
//! This module handles window close requests, including cleanup of GPU resources
//! and application exit logic when no windows remain.

use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;
use crate::window::WinitGpuiApp;

/// Handle window close requests
///
/// Cleans up window-specific resources (GPU renderers, window state) and exits
/// the application if no windows remain open.
///
/// # Arguments
/// * `app` - The application state
/// * `event_loop` - The active event loop for exit control
/// * `window_id` - ID of the window being closed
pub fn handle_close_requested(
    app: &mut WinitGpuiApp,
    event_loop: &ActiveEventLoop,
    window_id: WindowId,
) {
    tracing::debug!("\n🚪 Closing window...");

    // Clean up window-specific GPU renderer
    if let Some(window_id_u64) = app.window_id_map.get_id(&window_id) {
        app.engine_context.renderers.unregister(window_id_u64);
    }

    app.window_id_map.remove(&window_id);
    app.windows.remove(&window_id);
    *app.engine_context.window_count.lock() -= 1;

    // Exit application if no windows remain
    if app.windows.is_empty() {
        tracing::debug!("🚪 No windows remain, exiting application...");
        event_loop.exit();
    }
}
