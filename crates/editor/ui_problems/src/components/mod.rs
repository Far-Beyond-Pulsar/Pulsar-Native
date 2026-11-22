//! Render functions of the Problems drawer: the header (tabs, filters,
//! search), the list (groups, one-line items, the open item's details) and
//! the code details (file preview, quick-fix diffs).

pub use crate::screen::ProblemsDrawer;

pub use content::{render_file_preview, render_hint_diff};
pub use header::{render_header, severity_style};
pub use list::render_list;

mod content;
mod header;
mod list;

use gpui::{prelude::*, *};
use ui::{v_flex, ActiveTheme as _};

/// The whole drawer.
pub fn render_drawer(
    drawer: &mut ProblemsDrawer,
    _window: &mut Window,
    cx: &mut Context<ProblemsDrawer>,
) -> impl IntoElement {
    v_flex()
        .size_full()
        .bg(cx.theme().background)
        .on_action(cx.listener(crate::handlers::on_filter_all))
        .on_action(cx.listener(crate::handlers::on_filter_errors))
        .on_action(cx.listener(crate::handlers::on_filter_warnings))
        .on_action(cx.listener(crate::handlers::on_filter_info))
        .on_action(cx.listener(crate::handlers::on_filter_source))
        .on_action(cx.listener(crate::handlers::on_clear_source_filter))
        .child(render_header(drawer, cx))
        .child(render_list(drawer, cx))
}
