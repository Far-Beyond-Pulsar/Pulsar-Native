//! Git Manager UI views — module declarations

pub mod branches;
pub mod changes;
pub mod commit_detail;
pub mod diff_viewer;
pub mod file_panel;
pub mod history;
pub mod toolbar;

pub use branches::render_branches_view;
pub use changes::render_changes_view;
pub use commit_detail::render_commit_detail;
pub use diff_viewer::render_side_by_side_diff;
pub(crate) use diff_viewer::{compute_aligned_rows, render_aligned_row, AlignedRow};
pub use file_panel::render_file_panel;
pub use history::render_history_view;
