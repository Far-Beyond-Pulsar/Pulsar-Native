//! Loading screen — runs background tasks, shows progress, then opens the editor.

mod preload;
mod recent_projects;
mod screen;
mod tasks;

pub use preload::{take_preloaded_files, PreloadedFileEntry};
pub use screen::LoadingScreen;
