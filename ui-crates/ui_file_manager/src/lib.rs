//! File Manager UI
//!
//! File browser and management

pub mod drawer;
mod file_manager_drawer;
pub mod window;

// Re-export main types
pub use file_manager_drawer::FileManagerDrawer;
pub use drawer::{FileSelected, PopoutFileManagerEvent};
pub use window::FileManagerWindow;
