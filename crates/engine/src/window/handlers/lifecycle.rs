//! Application lifecycle event handlers
//!
//! This module handles application lifecycle events such as resumed (app start)
//! and about_to_wait (idle/frame end).

use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;
use crate::window::WinitGpuiApp;
use crate::window::initialization;
use engine_state::WindowRequest;

/// Handle application resumed event
///
/// Called when the application starts or resumes. Creates the initial main
/// entry window if no windows exist, or creates a ProjectSplash window if
/// launched via URI scheme.
///
/// # Arguments
/// * `app` - The application state
/// * `event_loop` - The active event loop
pub fn handle_resumed(
    app: &mut WinitGpuiApp,
    event_loop: &ActiveEventLoop,
) {
    profiling::profile_scope!("Lifecycle::Resumed");

    // Only create main window if no windows exist
    if !app.windows.is_empty() {
        return;
    }

    // Check for URI-launched project
    if let Some(engine_context) = engine_state::EngineContext::global() {
        let mut launch = engine_context.launch.write();
        if let Some(uri_path) = launch.uri_project_path.take() {
            tracing::debug!("Opening project from URI: {}", uri_path.display());

            // Create project splash screen instead of entry
            drop(launch); // Release lock before creating window
            app.create_window(event_loop, WindowRequest::ProjectSplash {
                project_path: uri_path.to_string_lossy().to_string()
            });
            return;
        }
    }

    tracing::debug!("✨ Creating main entry window...");

    // Default: Create the main entry window using the modular system
    app.create_window(event_loop, WindowRequest::Entry);
}

/// Handle about_to_wait event
///
/// Called when the event loop is about to wait for new events (idle state).
/// Performs lazy render checks, processes window requests, and initializes
/// any pending GPUI windows.
///
/// # Arguments
/// * `app` - The application state
/// * `event_loop` - The active event loop
pub fn handle_about_to_wait(
    app: &mut WinitGpuiApp,
    event_loop: &ActiveEventLoop,
) {
    profiling::profile_scope!("Lifecycle::AboutToWait");

    // LAZY CHECK: If GPUI windows need rendering, request redraw
    // This happens once per event loop iteration, not blocking
    for (_window_id, window_state) in &mut app.windows {
        if let Some(gpui_window_ref) = &window_state.gpui_window {
            // Only check if we're not already waiting for a redraw
            if !window_state.needs_render {
                let gpui_needs_render = window_state.gpui_app.update(|cx| {
                    gpui_window_ref.needs_render(cx)
                });
                if gpui_needs_render {
                    window_state.needs_render = true;
                    window_state.winit_window.request_redraw();
                }
            }
        }
    }

    // Check for window creation requests
    while let Ok(request) = app.window_request_rx.try_recv() {
        app.pending_window_requests.push(request);
    }

    // Process pending window requests (collect first to avoid borrow issues)
    let requests: Vec<_> = app.pending_window_requests.drain(..).collect();
    for request in requests {
        match request {
            WindowRequest::CloseWindow { window_id } => {
                // Find and close the window with this ID
                if let Some(window_id_native) = app.window_id_map.get_window_id(window_id) {
                    if app.windows.remove(&window_id_native).is_some() {
                        tracing::debug!("✨ Closed window with ID: {:?}", window_id);
                        app.window_id_map.remove(&window_id_native);
                        *app.engine_context.window_count.lock() -= 1;
                    }
                } else {
                    tracing::warn!("⚠️ Attempted to close unknown window ID: {}", window_id);
                }
            }
            _ => {
                app.create_window(event_loop, request);
            }
        }
    }

    // Initialize any uninitialized GPUI windows
    let window_ids: Vec<WindowId> = app.windows.keys().copied().collect();
    for window_id in window_ids {
        let should_initialize = {
            let window_state = app.windows.get(&window_id).expect("Window must exist");
            !window_state.gpui_window_initialized
        };

        if should_initialize {
            // Initialize GPUI window (fonts, themes, keybindings, window content)
            initialization::gpui::initialize_gpui_window(app, &window_id, &app.engine_context.clone());

            // Initialize D3D11 rendering pipeline (Windows only)
            #[cfg(target_os = "windows")]
            unsafe {
                initialization::d3d11::initialize_d3d11_pipeline(app, &window_id);
            }

            // Mark as initialized
            let window_state = app.windows.get_mut(&window_id).expect("Window must exist");
            window_state.gpui_window_initialized = true;
            tracing::debug!("✨ GPUI window opened! Ready for GPU composition!\n");
        }
    }
}
