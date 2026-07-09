//! Git Manager UI views — module declarations

pub mod branches;
pub mod changes;
pub mod commit_detail;
pub mod file_panel;
pub mod history;
pub mod toolbar;

pub use branches::render_branches_view;
pub use changes::render_changes_view;
pub use commit_detail::render_commit_detail;
pub use file_panel::render_file_panel;
pub use history::render_history_view;
