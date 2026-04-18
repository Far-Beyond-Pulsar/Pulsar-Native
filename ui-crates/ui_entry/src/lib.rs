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

        // Build the Root now so we have a handle to swap its view later.
        let root_entity = cx.new(|cx| Root::new(intro_screen.clone().into(), window, cx));

        // Clone all callbacks so the IntroComplete subscriber can swap the view in-place.
        let ec_oobe = engine_context.clone();
        let on_proj_oobe = on_project_selected.clone();
        let on_git_oobe = on_git_manager.clone();
        let on_set_oobe = on_settings.clone();
        let on_fab_oobe = on_fab_search.clone();
        let root_weak = root_entity.downgrade();

        cx.subscribe(&intro_screen, move |_view: Entity<IntroScreen>, _event: &IntroComplete, cx: &mut App| {
            println!("✅ [OOBE subscriber] IntroComplete received — beginning swap");
            mark_intro_seen();

            let on_proj2 = on_proj_oobe.clone();
            let on_git2  = on_git_oobe.clone();
            let on_set2  = on_set_oobe.clone();
            let on_fab2  = on_fab_oobe.clone();
            let ec2      = ec_oobe.clone();
            let root_weak2 = root_weak.clone();

            cx.spawn(async move |mut cx| {
                println!("✅ [OOBE subscriber] inside spawn, calling update_window");
                let result = cx.update_window(window_handle, move |_, window, cx| {
                    println!("✅ [OOBE subscriber] inside update_window, creating EntryScreen");
                    let entry_screen = cx.new(|cx| EntryScreen::new(window, cx));

                    // Wire up exactly the same subscriptions as the normal (non-OOBE) path.
                    let ec_sub = ec2.clone();
                    let on_proj3 = on_proj2.clone();
                    cx.subscribe(&entry_screen, move |_view: Entity<EntryScreen>, event: &crate::entry_screen::project_selector::ProjectSelected, cx: &mut App| {
                        on_proj3(event.path.clone(), cx);
                        let _ = ec_sub.clone();
                    }).detach();

                    let on_git3 = on_git2.clone();
                    cx.subscribe(&entry_screen, move |_view: Entity<EntryScreen>, event: &crate::entry_screen::GitManagerRequested, cx: &mut App| {
                        on_git3(event.path.clone(), cx);
                    }).detach();

                    let on_set3 = on_set2.clone();
                    cx.subscribe(&entry_screen, move |_view: Entity<EntryScreen>, _event: &crate::entry_screen::SettingsRequested, cx: &mut App| {
                        on_set3(cx);
                    }).detach();

                    let on_fab3 = on_fab2.clone();
                    cx.subscribe(&entry_screen, move |_view: Entity<EntryScreen>, _event: &crate::entry_screen::FabSearchRequested, cx: &mut App| {
                        on_fab3(cx);
                    }).detach();

                    if let Some(root) = root_weak2.upgrade() {
                        root.update(cx, |r, cx| r.set_view(entry_screen.into(), cx));
                        println!("✅ [OOBE] View swapped to entry screen");
                    } else {
                        println!("❌ [OOBE] root_weak upgrade failed — cannot swap view");
                    }
                });
                println!("✅ [OOBE subscriber] update_window result: {:?}", result.is_ok());
            }).detach();
        }).detach();

        return root_entity;
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
