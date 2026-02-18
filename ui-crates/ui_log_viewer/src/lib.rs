//! Log Viewer UI component for displaying engine logs with virtual scrolling

mod log_drawer;
mod log_reader;
mod virtual_table;

pub use log_drawer::{LogViewerDrawer, ToggleLogViewer};
pub use log_reader::LogReader;
