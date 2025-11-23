//! Main Editor UI
//!
//! Main editor window with tabs and drawers

// Drawers and tabs are in the main editor module
pub mod drawers;
pub mod tabs;
pub mod editors;

// Re-export main types
pub use drawers::{FileManagerDrawer, TerminalDrawer, ProblemsDrawer};
pub use ui_file_manager::{FileSelected, FileType as DrawerFileType, PopoutFileManagerEvent};
pub use tabs::{
    ScriptEditorPanel, LevelEditorPanel, BlueprintEditorPanel, DawEditorPanel,
    TextEditorEvent,
};
