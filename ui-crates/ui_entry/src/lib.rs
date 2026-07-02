mod core;
mod component;
mod screen;
mod service;
mod util;
mod window;

pub use core::events::*;
pub use core::types::*;
pub use screen::EntryScreen;
pub use window::EntryWindow;

pub use engine_state::{EngineContext, WindowContext, WindowRequest};
pub use ui::OpenSettings;

use gpui::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use ui::Root;

pub static FORCE_OOBE: AtomicBool = AtomicBool::new(false);

pub fn create_entry_component(
    window: &mut Window,
    cx: &mut App,
    on_project_selected: Arc<dyn Fn(PathBuf, &mut App) + Send + Sync>,
    on_git_manager: Arc<dyn Fn(PathBuf, &mut App) + Send + Sync>,
    on_settings: Arc<dyn Fn(&mut App) + Send + Sync>,
    on_fab_search: Arc<dyn Fn(&mut App) + Send + Sync>,
) -> Entity<Root> {
    let window_handle = window.window_handle();

    let entry_window = cx.new(|cx| EntryWindow::new(window, cx));

    let on_proj = on_project_selected.clone();
    cx.subscribe(
        &entry_window,
        move |_view: Entity<EntryWindow>, event: &ProjectSelected, cx: &mut App| {
            on_proj(event.path.clone(), cx);
            let _ = cx.update_window(window_handle, |_, window, _| window.remove_window());
        },
    )
    .detach();

    let on_git = on_git_manager.clone();
    cx.subscribe(
        &entry_window,
        move |_view: Entity<EntryWindow>, event: &GitManagerRequested, cx: &mut App| {
            on_git(event.path.clone(), cx);
        },
    )
    .detach();

    let on_set = on_settings.clone();
    cx.subscribe(
        &entry_window,
        move |_view: Entity<EntryWindow>, _event: &SettingsRequested, cx: &mut App| {
            on_set(cx);
        },
    )
    .detach();

    let on_fab = on_fab_search.clone();
    cx.subscribe(
        &entry_window,
        move |_view: Entity<EntryWindow>, _event: &FabSearchRequested, cx: &mut App| {
            on_fab(cx);
        },
    )
    .detach();

    cx.new(|cx| Root::new(entry_window.clone().into(), window, cx))
}
