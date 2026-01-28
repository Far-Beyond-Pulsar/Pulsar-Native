//! Window content creation
//!
//! This module contains the factory function for creating window-type-specific
//! content views based on WindowRequest type.

use gpui::*;
use engine_state::{EngineContext, WindowRequest, ProjectContext};
use ui_core::{PulsarApp, PulsarRoot};
use ui_entry::create_entry_component;
use ui_settings::create_settings_component;
use ui_loading_screen::create_loading_component;
use ui_about::create_about_window;
use ui_documentation;
use ui;
use std::path::PathBuf;

/// Create window content based on window type
///
/// Returns a GPUI Root entity configured for the specific window type.
///
/// # Arguments
/// * `window_type` - The type of window being created
/// * `captured_window_id` - The window ID for this window
/// * `engine_context` - Typed engine context for cross-window communication
/// * `window` - GPUI window handle
/// * `cx` - GPUI app context
///
/// # Returns
/// A configured Root entity for the window
pub fn create_window_content(
    window_type: &Option<WindowRequest>,
    captured_window_id: u64,
    engine_context: &EngineContext,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ui::Root> {
    profiling::profile_scope!("Window::CreateContent");

    match window_type {
        Some(WindowRequest::Entry) => {
            create_entry_component(window, cx, engine_context, captured_window_id)
        }
        Some(WindowRequest::Settings) => {
            create_settings_component(window, cx, engine_context)
        }
        Some(WindowRequest::About) => {
            create_about_window(window, cx)
        }
        Some(WindowRequest::Documentation) => {
            // Get the current project path from engine context if available
            let project_path = engine_context.project.read()
                .as_ref()
                .map(|p| p.path.clone());

            ui_documentation::create_documentation_window_with_project(window, cx, project_path)
        }
        Some(WindowRequest::ProjectSplash { project_path }) => {
            // Store the current project path in engine context for other windows to access
            let project_ctx = ProjectContext::new(PathBuf::from(project_path.clone()));
            *engine_context.project.write() = Some(project_ctx);

            // Create loading screen for project loading
            create_loading_component(
                PathBuf::from(project_path),
                captured_window_id,
                window,
                cx
            )
        }
        Some(WindowRequest::ProjectEditor { project_path }) => {
            // Store the current project path in engine context for other windows to access
            let project_ctx = ProjectContext::new(PathBuf::from(project_path.clone()));
            *engine_context.project.write() = Some(project_ctx);

            // Use the captured window_id to ensure consistency
            // Create the actual PulsarApp editor with the project
            let app = cx.new(|cx| PulsarApp::new_with_project_and_window_id(
                PathBuf::from(project_path),
                captured_window_id,
                window,
                cx
            ));
            let pulsar_root = cx.new(|cx| PulsarRoot::new("Pulsar Engine", app, window, cx));
            cx.new(|cx| ui::Root::new(pulsar_root.into(), window, cx))
        }
        Some(WindowRequest::CloseWindow { .. }) | None => {
            // Fallback to entry screen if window_type is None or CloseWindow
            create_entry_component(window, cx, engine_context, captured_window_id)
        }
    }
}
