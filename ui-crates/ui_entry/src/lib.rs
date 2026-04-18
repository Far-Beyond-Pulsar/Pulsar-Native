//! Entry Screen UI
//!
//! Project selection and startup screens

pub mod entry_screen;
pub mod oobe;
pub mod window;
pub mod dependency_setup_window;

// Re-export main types
pub use window::EntryWindow;
pub use entry_screen::{EntryScreen, project_selector::ProjectSelected, GitManagerRequested, FabSearchRequested};
pub use dependency_setup_window::{DependencySetupWindow, SetupComplete};
pub use oobe::{IntroScreen, IntroComplete, has_seen_intro, mark_intro_seen, reset_intro};

// Re-export engine types that UI needs
pub use engine_state::{EngineContext, WindowRequest, WindowContext};

// Re-export actions from ui crate
pub use ui::OpenSettings;

use gpui::*;
use std::sync::Arc;
use std::time::Duration;
use std::path::PathBuf;
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
    on_project_selected: Arc<dyn Fn(PathBuf, &mut App) + Send + Sync>,
    on_git_manager: Arc<dyn Fn(PathBuf, &mut App) + Send + Sync>,
    on_settings: Arc<dyn Fn(&mut App) + Send + Sync>,
    on_fab_search: Arc<dyn Fn(&mut App) + Send + Sync>,
) -> Entity<Root> {
    
    // Capture a window handle that can be safely sent across closures.
    let window_handle = window.window_handle();

    // Check if we should show OOBE intro first
    let seen_intro = has_seen_intro();
    tracing::debug!("🎯 [ENTRY] has_seen_intro() = {}", seen_intro);
    
    if !seen_intro {
        tracing::debug!("🎉 [OOBE] Showing intro screen for first-time user");

        // Create the intro screen
        let intro_screen = cx.new(|cx| IntroScreen::new(window, cx));

        // Clone all callbacks so the IntroComplete subscriber can open the entry screen.
        let ec_oobe = engine_context.clone();
        let on_proj_oobe = on_project_selected.clone();
        let on_git_oobe = on_git_manager.clone();
        let on_set_oobe = on_settings.clone();
        let on_fab_oobe = on_fab_search.clone();

        cx.subscribe(&intro_screen, move |_view: Entity<IntroScreen>, _event: &IntroComplete, cx: &mut App| {
            println!("✅ [OOBE subscriber] IntroComplete received — opening entry screen");
            mark_intro_seen();

            let on_proj2 = on_proj_oobe.clone();
            let on_git2  = on_git_oobe.clone();
            let on_set2  = on_set_oobe.clone();
            let on_fab2  = on_fab_oobe.clone();
            let ec2      = ec_oobe.clone();

            let opts = gpui::WindowOptions {
                titlebar: None,
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_min_size: Some(gpui::Size {
                    width: gpui::px(600.0),
                    height: gpui::px(400.0),
                }),
                ..Default::default()
            };

            let result = cx.open_window(opts, move |window, cx| {
                println!("✅ [OOBE] Entry window opened, building component");
                create_entry_component(window, cx, &ec2, 0, on_proj2, on_git2, on_set2, on_fab2)
            });
            println!("✅ [OOBE] open_window result: {:?}", result.is_ok());
            // The OOBE window closes itself by calling window.remove_window() in skip()
        }).detach();

        return cx.new(|cx| Root::new(intro_screen.into(), window, cx));
    }

    tracing::debug!("🎯 [ENTRY] Showing entry screen (intro already seen)");
    let entry_screen = cx.new(|cx| EntryScreen::new(window, cx));

    // Subscribe to ProjectSelected event - open loading window and close entry window
    let engine_context_clone = engine_context.clone();
    let on_proj = on_project_selected.clone();
    cx.subscribe(&entry_screen, move |_view: Entity<EntryScreen>, event: &ProjectSelected, cx: &mut App| {
        tracing::debug!("🎯 [ENTRY] Project selected: {:?}", event.path);
        tracing::debug!("🎯 [ENTRY] Path exists: {}", event.path.exists());
        tracing::debug!("🎯 [ENTRY] Path is_dir: {}", event.path.is_dir());

        // invoke callback provided by engine; it will handle opening splash
        on_proj(event.path.clone(), cx);

        // Close entry window after delay
        if window_id != 0 {
            let ec2 = engine_context_clone.clone();
            let close_id = window_id;
            // use the previously computed handle rather than capturing `window`
            cx.spawn(async move |mut cx| {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                tracing::debug!("🗑️ (delayed) Closing entry window {}", close_id);
                let _ = cx.update_window(window_handle, |_, window, _| window.remove_window());
                ec2.unregister_window(&close_id);
            });
        }
    }).detach();

    // Subscribe to GitManagerRequested event - open git manager window
    let on_git = on_git_manager.clone();
    cx.subscribe(&entry_screen, move |_view: Entity<EntryScreen>, event: &GitManagerRequested, cx: &mut App| {
        tracing::debug!("🔧 [ENTRY] Git Manager requested for: {:?}", event.path);
        on_git(event.path.clone(), cx);
    }).detach();

    // Subscribe to SettingsRequested event - open settings from engine callback
    let on_set = on_settings.clone();
    cx.subscribe(&entry_screen, move |_view: Entity<EntryScreen>, _event: &crate::entry_screen::SettingsRequested, cx: &mut App| {
        tracing::debug!("⚙️ [ENTRY] Settings requested");
        on_set(cx);
    }).detach();

    // Subscribe to FabSearchRequested event - open FAB marketplace from engine callback
    let on_fab = on_fab_search.clone();
    cx.subscribe(&entry_screen, move |_view: Entity<EntryScreen>, _event: &crate::entry_screen::FabSearchRequested, cx: &mut App| {
        tracing::debug!("🛍️ [ENTRY] FAB search requested");
        on_fab(cx);
    }).detach();

    cx.new(|cx| Root::new(entry_screen.clone().into(), window, cx))
}
