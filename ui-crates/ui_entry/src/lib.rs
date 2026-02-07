//! Entry Screen UI
//!
//! Project selection and startup screens

pub mod entry_screen;
pub mod oobe;
pub mod window;
pub mod dependency_setup_window;

// Re-export main types
pub use window::EntryWindow;
pub use entry_screen::{EntryScreen, project_selector::ProjectSelected};
pub use dependency_setup_window::{DependencySetupWindow, SetupComplete};
pub use oobe::{IntroScreen, IntroComplete, has_seen_intro, mark_intro_seen, reset_intro};

// Re-export engine types that UI needs
pub use engine_state::{EngineContext, WindowRequest};

// Re-export actions from ui crate
pub use ui::OpenSettings;

use gpui::*;
use ui::Root;

// Component config
#[derive(Clone)]
pub struct EntryScreenConfig {
    // Configuration options
}

impl Default for EntryScreenConfig {
    fn default() -> Self {
        Self {}
    }
}

/// Create an entry screen component as a composable piece
pub fn create_entry_component(
    window: &mut Window,
    cx: &mut App,
    engine_context: &EngineContext,
    window_id: u64,
) -> Entity<Root> {
    
    // Check if we should show OOBE intro first
    let seen_intro = has_seen_intro();
    tracing::debug!("🎯 [ENTRY] has_seen_intro() = {}", seen_intro);
    
    if !seen_intro {
        tracing::debug!("🎉 [OOBE] Showing intro screen for first-time user");

        // Create the intro screen
        let intro_screen = cx.new(|cx| IntroScreen::new(window, cx));

        // Subscribe to intro completion - will switch to entry screen
        let engine_context_clone = engine_context.clone();
        cx.subscribe(&intro_screen, move |_view: Entity<IntroScreen>, _event: &IntroComplete, cx: &mut App| {
            tracing::debug!("🎉 [OOBE] Intro complete, marking as seen");
            mark_intro_seen();

            // Request a new entry window to replace this one
            engine_context_clone.request_window(WindowRequest::Entry);

            // Close the current window
            if window_id != 0 {
                tracing::debug!("🗑️ Closing OOBE window {}", window_id);
                engine_context_clone.request_window(WindowRequest::CloseWindow {
                    window_id,
                });
            }
        }).detach();

        return cx.new(|cx| Root::new(intro_screen.clone().into(), window, cx));
    }

    tracing::debug!("🎯 [ENTRY] Showing entry screen (intro already seen)");
    let entry_screen = cx.new(|cx| EntryScreen::new(window, cx));

    // Subscribe to ProjectSelected event - open loading window and close entry window
    let engine_context_clone = engine_context.clone();
    cx.subscribe(&entry_screen, move |_view: Entity<EntryScreen>, event: &ProjectSelected, _cx: &mut App| {
        tracing::debug!("🎯 [ENTRY] Project selected: {:?}", event.path);
        tracing::debug!("🎯 [ENTRY] Path exists: {}", event.path.exists());
        tracing::debug!("🎯 [ENTRY] Path is_dir: {}", event.path.is_dir());

        // Request loading/splash window
        engine_context_clone.request_window(WindowRequest::ProjectSplash {
            project_path: event.path.to_string_lossy().to_string(),
        });

        // Close the entry window
        if window_id != 0 {
            tracing::debug!("🗑️ Closing entry window {}", window_id);
            engine_context_clone.request_window(WindowRequest::CloseWindow {
                window_id,
            });
        }
    }).detach();
    
    cx.new(|cx| Root::new(entry_screen.clone().into(), window, cx))
}
