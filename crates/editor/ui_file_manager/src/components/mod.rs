//! Views of the file manager: file list (`FileManagerDrawer`), sidebar, context menus and
//! the commit picker.

pub mod commit_picker;
pub mod context_menus;
pub mod file_list;
pub mod sidebar;

pub use file_list::FileManagerDrawer;
