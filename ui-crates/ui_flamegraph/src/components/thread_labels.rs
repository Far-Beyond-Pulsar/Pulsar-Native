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
    let thread_offsets = thread_offsets.clone();
    let view_state = view_state.clone();
    let theme = cx.theme();

    div()
        .absolute()
        .left_0()
        .top_0()
        .w(px(THREAD_LABEL_WIDTH))
        .h_full()
        .bg(theme.popover)
        .border_r_1()
        .border_color(theme.border)
        .overflow_hidden()
        .children(
            thread_offsets.iter().map(|(thread_id, y_offset)| {
                let thread = frame.threads.get(thread_id).unwrap();
                let y = y_offset + view_state.pan_y;

                div()
                    .absolute()
                    .top(px(y))
                    .left_0()
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .px_2()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(get_thread_color(*thread_id))
                            .child(thread.name.clone())
                    )
            })
        )
}
