// File Manager Drawer - Modular file browser component
//
// Organized structure:
// - actions: All file operations actions
// - types: Enums, FileItem, events
// - tree: Folder tree (FolderNode)
// - utils: Utility functions
// - operations: File system operations (integrated with engine_fs)
// - context_menus: Context menu builders
// - content: Content area rendering (header, grid, list)

pub mod actions;
pub mod types;
pub mod tree;
pub mod utils;
pub mod operations;
pub mod context_menus;
pub mod fs_metadata;
// pub mod content; // TODO: Fix lifetime issues in grid/list rendering

// Re-export commonly used types
pub use actions::*;
pub use types::{
    ViewMode, SortBy, SortOrder, DragState, DraggedFile,
    FileItem, FileSelected, PopoutFileManagerEvent,
};
pub use tree::FolderNode;
pub use operations::FileOperations;
pub use fs_metadata::FsMetadataManager;

// Public API note:
// The full FileManagerDrawer implementation is in the monolithic file_manager_drawer.rs
// This modular structure provides a clean architecture for maintainability
// Future work: Move all rendering logic into this modular structure
