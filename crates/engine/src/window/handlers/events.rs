//! Window event dispatcher
//!
//! This module contains the main event dispatcher that routes Winit window events
//! to appropriate specialized handlers.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;
use crate::window::WinitGpuiApp;
use crate::window::input;
use crate::window::handlers::close;

/// Dispatch window events to appropriate handlers
///
/// This is the main event router that receives all window events from Winit
/// and dispatches them to specialized handler modules based on event type.
///
/// For now, this only handles the simpler events that have been fully extracted.
/// Complex events (RedrawRequested, Resized) are left for app.rs to handle
/// in a future refactoring phase.
///
/// # Arguments
/// * `app` - The application state
/// * `event_loop` - The active event loop
/// * `window_id` - ID of the window receiving the event
/// * `event` - The window event to handle
pub fn dispatch_window_event(
    app: &mut WinitGpuiApp,
    event_loop: &ActiveEventLoop,
    window_id: WindowId,
    event: WindowEvent,
) {
    profiling::profile_scope!("Window::Event");
    
    match event {
        WindowEvent::CloseRequested => {
            profiling::profile_scope!("Window::CloseRequested");
            close::handle_close_requested(app, event_loop, window_id);
        }
        WindowEvent::KeyboardInput { event, .. } => {
            profiling::profile_scope!("Window::KeyboardInput");
            input::keyboard::handle_keyboard_input(app, window_id, event);
        }
        WindowEvent::ModifiersChanged(new_modifiers) => {
            profiling::profile_scope!("Window::ModifiersChanged");
            input::modifiers::handle_modifiers_changed(app, window_id, new_modifiers.state());
        }
        WindowEvent::CursorMoved { position, .. } => {
            profiling::profile_scope!("Window::CursorMoved");
            input::mouse::handle_cursor_moved(app, window_id, position);
        }
        WindowEvent::MouseInput { state, button, .. } => {
            profiling::profile_scope!("Window::MouseInput");
            input::mouse::handle_mouse_input(app, window_id, state, button);
        }
        WindowEvent::MouseWheel { delta, .. } => {
            profiling::profile_scope!("Window::MouseWheel");
            input::mouse::handle_mouse_wheel(app, window_id, delta);
        }
        WindowEvent::Resized(new_size) => {
            profiling::profile_scope!("Window::Resized");
            crate::window::rendering::resize::handle_resize(app, window_id, new_size);
        }
        WindowEvent::RedrawRequested => {
            profiling::profile_scope!("Window::RedrawRequested");
            #[cfg(target_os = "windows")]
            unsafe {
                crate::window::rendering::compositor::handle_redraw(app, window_id);
            }
            #[cfg(not(target_os = "windows"))]
            {
                // Non-Windows platforms don't have D3D11 compositor yet
            }
        }
        _ => {}
    }
}
