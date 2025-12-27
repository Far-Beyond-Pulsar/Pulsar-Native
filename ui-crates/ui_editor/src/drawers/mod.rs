//! Drawer Components
//!
//! Drawers are collapsible sidebar panels that can be shown/hidden in the main editor.
//! They provide auxiliary functionality like file browsing, problem lists, and terminal access.

// Re-export from other ui crates
pub use ui_file_manager::FileManagerDrawer;
pub use ui_problems::ProblemsDrawer;
pub use ui_terminal::TerminalDrawer;
