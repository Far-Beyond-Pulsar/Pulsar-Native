//! Thread labels component showing thread names on the left

use gpui::*;
use std::sync::Arc;
use std::collections::BTreeMap;
use crate::trace_data::TraceFrame;
use crate::state::ViewState;
use crate::constants::{THREAD_LABEL_WIDTH, ROW_HEIGHT};
use crate::colors::get_thread_color;
use ui::ActiveTheme;

/// Render the thread labels on the left side
pub fn render_thread_labels(
    frame: &Arc<TraceFrame>,
    thread_offsets: &BTreeMap<u64, f32>,
    view_state: &ViewState,
    cx: &mut Context<impl Render>,
) -> impl IntoElement {
    let setup_start = std::time::Instant::now();
    let thread_offsets = thread_offsets.clone();
    let view_state = view_state.clone();
    let theme = cx.theme();

    let render_start = std::time::Instant::now();
    let result = div()
        .absolute()
        .left_0()
        .top_0()
        .w(px(THREAD_LABEL_WIDTH))
        .h_full()
        .bg(theme.sidebar)
        .border_r_2()
        .border_color(theme.sidebar_border)
        .overflow_hidden()
        .children(
            thread_offsets.iter().map(|(thread_id, y_offset)| {
                let thread = frame.threads.get(thread_id).unwrap();
                let y = y_offset + view_state.pan_y;
                let thread_color = get_thread_color(*thread_id);

                div()
                    .absolute()
                    .top(px(y))
                    .left_0()
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .child(
                        // Color indicator dot
                        div()
                            .w(px(8.0))
                            .h(px(8.0))
                            .rounded(px(4.0))
                            .bg(thread_color)
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.sidebar_foreground)
                            .child(thread.name.clone())
                    )
            })
        );
    result
}
