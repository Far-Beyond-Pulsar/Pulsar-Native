//! Entry Screen UI
//!
//! Project selection and startup screens

pub mod dependency_setup_window;
pub mod entry_screen;
pub mod oobe;
pub mod window;

// Re-export main types
pub use dependency_setup_window::{DependencySetupWindow, SetupComplete};
pub use entry_screen::{
    project_selector::ProjectSelected, EntryScreen, FabSearchRequested, GitManagerRequested,
};
pub use oobe::{has_seen_intro, mark_intro_seen, reset_intro, IntroComplete, IntroScreen};
pub use window::EntryWindow;

// Re-export engine types that UI needs
pub use engine_state::{EngineContext, WindowContext, WindowRequest};

// Re-export actions from ui crate
pub use ui::OpenSettings;

use gpui::*;
use std::path::PathBuf;
use std::sync::Arc;
use ui::Root;

// Component config
#[derive(Clone, Default)]
pub struct EntryScreenConfig {
    // Configuration options
}

/// Create an entry screen component as a composable piece
pub fn create_entry_component(
    window: &mut Window,
    cx: &mut App,
    on_project_selected: Arc<dyn Fn(PathBuf, &mut App) + Send + Sync>,
    on_git_manager: Arc<dyn Fn(PathBuf, &mut App) + Send + Sync>,
    on_settings: Arc<dyn Fn(&mut App) + Send + Sync>,
    on_fab_search: Arc<dyn Fn(&mut App) + Send + Sync>,
) -> Entity<Root> {
    // Capture a window handle that can be safely sent across closures.
    let window_handle = window.window_handle();

    tracing::debug!("🎯 [ENTRY] Showing entry screen (OOBE handled internally)");
    let entry_screen = cx.new(|cx| EntryScreen::new(window, cx));

    // Subscribe to ProjectSelected event - open loading window and close entry window
    let on_proj = on_project_selected.clone();
    cx.subscribe(
        &entry_screen,
        move |_view: Entity<EntryScreen>, event: &ProjectSelected, cx: &mut App| {
            tracing::debug!("🎯 [ENTRY] Project selected: {:?}", event.path);
            tracing::debug!("🎯 [ENTRY] Path exists: {}", event.path.exists());
            tracing::debug!("🎯 [ENTRY] Path is_dir: {}", event.path.is_dir());

            // invoke callback provided by engine; it will handle opening loading screen
            on_proj(event.path.clone(), cx);

            // Close entry window - launcher self-closes once loading window opens
            tracing::debug!("🗑️ Closing entry window");
            let _ = cx.update_window(window_handle, |_, window, _| window.remove_window());
        },
    )
    .detach();

    // Subscribe to GitManagerRequested event - open git manager window
    let on_git = on_git_manager.clone();
    cx.subscribe(
        &entry_screen,
        move |_view: Entity<EntryScreen>, event: &GitManagerRequested, cx: &mut App| {
            tracing::debug!("🔧 [ENTRY] Git Manager requested for: {:?}", event.path);
            on_git(event.path.clone(), cx);
        },
    )
    .detach();

    // Subscribe to SettingsRequested event - open settings from engine callback
    let on_set = on_settings.clone();
    cx.subscribe(
        &entry_screen,
        move |_view: Entity<EntryScreen>,
              _event: &crate::entry_screen::SettingsRequested,
              cx: &mut App| {
            tracing::debug!("⚙️ [ENTRY] Settings requested");
            on_set(cx);
        },
    )
    .detach();

    // Subscribe to FabSearchRequested event - open FAB marketplace from engine callback
    let on_fab = on_fab_search.clone();
    cx.subscribe(
        &entry_screen,
        move |_view: Entity<EntryScreen>,
              _event: &crate::entry_screen::FabSearchRequested,
              cx: &mut App| {
            tracing::debug!("🛍️ [ENTRY] FAB search requested");
            on_fab(cx);
        },
    )
    .detach();

    cx.new(|cx| Root::new(entry_screen.clone().into(), window, cx))
}
