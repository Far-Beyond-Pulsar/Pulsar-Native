//! Entry Screen UI
//!
//! Project selection and startup screens

pub mod entry_screen;
pub mod loading_window;
pub mod window;
pub mod dependency_setup_window;

// Re-export main types
pub use window::EntryWindow;
pub use loading_window::{LoadingWindow, LoadingComplete};
pub use entry_screen::{EntryScreen, project_selector::ProjectSelected};
pub use dependency_setup_window::{DependencySetupWindow, SetupComplete};

// Re-export engine types that UI needs
pub use engine_state::{EngineState, WindowRequest};

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
    engine_state: &EngineState,
) -> Entity<Root> {
    // Store the window ID before creating the entry screen
    let window_id = if let Some(state) = EngineState::global() {
        state.get_metadata("latest_window_id")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
    } else {
        0
    };
    
    let entry_screen = cx.new(|cx| EntryScreen::new(window, cx));
    
    // Subscribe to ProjectSelected event - open loading window and close entry window
    let engine_state_clone = engine_state.clone();
    cx.subscribe(&entry_screen, move |_view: Entity<EntryScreen>, event: &ProjectSelected, _cx: &mut App| {
        println!("🎯 [ENTRY] Project selected: {:?}", event.path);
        println!("🎯 [ENTRY] Path exists: {}", event.path.exists());
        println!("🎯 [ENTRY] Path is_dir: {}", event.path.is_dir());
        
        // Request loading/splash window
        engine_state_clone.request_window(WindowRequest::ProjectSplash {
            project_path: event.path.to_string_lossy().to_string(),
        });
        
        // Close the entry window
        if window_id != 0 {
            println!("🗑️ Closing entry window {}", window_id);
            engine_state_clone.request_window(WindowRequest::CloseWindow {
                window_id,
            });
        }
    }).detach();
    
    cx.new(|cx| Root::new(entry_screen.clone().into(), window, cx))
}
