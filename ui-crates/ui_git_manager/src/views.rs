//! Git Manager UI views — module declarations

pub mod toolbar;
pub mod changes;
pub mod history;
pub mod branches;
pub mod file_panel;
pub mod commit_detail;

pub use toolbar::render_toolbar;
pub use changes::render_changes_view;
pub use history::render_history_view;
pub use branches::render_branches_view;
pub use file_panel::render_file_panel;
pub use commit_detail::render_commit_detail;
