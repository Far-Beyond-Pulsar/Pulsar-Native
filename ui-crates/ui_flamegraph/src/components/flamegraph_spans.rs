//! Span-based flamegraph rendering using divs instead of canvas

use gpui::*;
use gpui::prelude::FluentBuilder;
use std::sync::Arc;
use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use crate::trace_data::TraceFrame;
use crate::state::ViewState;
use crate::constants::{ROW_HEIGHT, THREAD_LABEL_WIDTH};
use crate::coordinates::time_to_x;
use ui::ActiveTheme;

/// Render the flamegraph using pure spans/divs
pub fn render_flamegraph_spans(
    frame: Arc<TraceFrame>,
    thread_offsets: Arc<BTreeMap<u64, f32>>,
    view_state: ViewState,
    palette: Vec<Hsla>,
    viewport_width: f32,
    cx: &mut Context<impl Render>,
) -> impl IntoElement {
    let theme = cx.theme();
    
    // Build list of visible spans
    let mut visible_spans = Vec::new();
    
    for (idx, span) in frame.spans.iter().enumerate() {
        let thread_y_offset = thread_offsets.get(&span.thread_id).copied().unwrap_or(0.0);
        let y = thread_y_offset + (span.depth as f32 * ROW_HEIGHT) + view_state.pan_y;
        
        // Calculate X positions
        let x1 = time_to_x(span.start_ns, &frame, viewport_width, &view_state);
        let x2 = time_to_x(span.end_ns(), &frame, viewport_width, &view_state);
        let width = x2 - x1;
        
        // Simple culling: skip if too small or off-screen
        if width < 0.5 {
            continue;
        }
        
        if x2 < THREAD_LABEL_WIDTH || x1 > viewport_width {
            continue;
        }
        
        // Get color from palette using hash
        let mut hasher = DefaultHasher::new();
        span.name.hash(&mut hasher);
        let color_idx = (hasher.finish() as usize) % palette.len();
        let color = palette[color_idx];
        
        visible_spans.push((idx, x1, y, width, color, span.name.clone()));
    }
    
    div()
        .absolute()
        .left(px(THREAD_LABEL_WIDTH))
        .top_0()
        .w(px(viewport_width - THREAD_LABEL_WIDTH))
        .h_full()
        .overflow_hidden()
        .children(
            visible_spans.into_iter().map(|(idx, x, y, width, color, name)| {
                let is_hovered = view_state.hovered_span == Some(idx);
                
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(width.max(1.0)))
                    .h(px(ROW_HEIGHT - 1.0))
                    .bg(color)
                    .when(is_hovered, |this| {
                        this.border_2()
                            .border_color(theme.accent)
                    })
                    .when(width > 50.0, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(hsla(0.0, 0.0, 1.0, 0.9))
                                .px_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(name)
                        )
                    })
            })
        )
}
