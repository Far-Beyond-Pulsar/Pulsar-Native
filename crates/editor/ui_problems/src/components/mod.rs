pub use crate::screen::ProblemsDrawer;

pub use content::{
    render_diagnostic_item, render_file_preview, render_flat_view, render_grouped_view,
    render_hint_diff,
};
pub use header::{render_empty_state, render_header, render_severity_badge};

mod content;
mod header;
