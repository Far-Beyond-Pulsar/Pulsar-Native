//! Virtual scrolling table for displaying log lines efficiently

use gpui::{prelude::*, *};
use ui::{h_flex, v_virtual_list, VirtualListScrollHandle, ActiveTheme as _, StyledExt};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub struct LogLine {
    pub line_number: usize,
    pub content: String,
}

const LINE_HEIGHT: f32 = 22.0;

/// Virtual scroll state with proper viewport tracking
#[derive(Clone)]
pub struct VirtualScrollState {
    pub total_lines: usize,
    pub is_locked_to_bottom: bool,
}

impl VirtualScrollState {
    pub fn new() -> Self {
        Self {
            total_lines: 0,
            is_locked_to_bottom: true,
        }
    }
    
    pub fn scroll_to_bottom(&mut self) {
        self.is_locked_to_bottom = true;
    }
}

pub struct LogTableState {
    pub lines: Vec<LogLine>,
    pub scroll_handle: VirtualListScrollHandle,
}

impl LogTableState {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            scroll_handle: VirtualListScrollHandle::new(),
        }
    }
    
    pub fn update_lines(&mut self, lines: Vec<LogLine>) {
        self.lines = lines;
    }
    
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_handle.scroll_to_bottom();
    }
    
    pub fn total_lines(&self) -> usize {
        self.lines.len()
    }
}

/// Render a virtual scrolling log table using VirtualList
pub fn render_virtual_log_table<V: 'static + Render>(
    view: Entity<V>,
    table_state: &LogTableState,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let line_count = table_state.lines.len();
    
    if line_count == 0 {
        return div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No log entries")
            )
            .into_any_element();
    }
    
    // Create item sizes - all lines have the same height
    let item_sizes: Rc<Vec<Size<Pixels>>> = Rc::new(
        (0..line_count)
            .map(|_| Size {
                width: px(1000.0), // Will be overridden by container width
                height: px(LINE_HEIGHT),
            })
            .collect()
    );
    
    let lines = table_state.lines.clone();
    
    v_virtual_list(
        view,
        "log-lines",
        item_sizes,
        move |_view, visible_range, _window, cx| {
            visible_range
                .into_iter()
                .filter_map(|ix| lines.get(ix).cloned())
                .map(|line| render_log_line(&line, cx))
                .collect()
        },
    )
    .size_full()
    .track_scroll(&table_state.scroll_handle)
    .into_any_element()
}

fn render_log_line<V: 'static>(
    line: &LogLine,
    cx: &mut Context<V>,
) -> impl IntoElement {
    // Parse log level from content
    let (level_color, level_text) = if line.content.contains("ERROR") {
        (cx.theme().danger, "ERR")
    } else if line.content.contains("WARN") {
        (cx.theme().warning, "WRN")
    } else if line.content.contains("INFO") {
        (cx.theme().primary, "INF")
    } else if line.content.contains("DEBUG") {
        (cx.theme().success, "DBG")
    } else if line.content.contains("TRACE") {
        (cx.theme().muted_foreground, "TRC")
    } else {
        (cx.theme().muted_foreground, "---")
    };
    
    h_flex()
        .w_full()
        .h(px(LINE_HEIGHT))
        .px_3()
        .py_1()
        .items_center()
        .gap_3()
        .border_b_1()
        .border_color(cx.theme().border.opacity(0.1))
        .hover(|s| s.bg(cx.theme().muted.opacity(0.5)))
        .child(
            // Line number
            div()
                .w(px(50.))
                .flex_shrink_0()
                .text_xs()
                .text_color(cx.theme().muted_foreground.opacity(0.7))
                .text_right()
                .font_family("'Courier New', monospace")
                .child(format!("{}", line.line_number))
        )
        .child(
            // Level badge
            div()
                .w(px(36.))
                .h(px(16.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .bg(level_color.opacity(0.15))
                .border_1()
                .border_color(level_color.opacity(0.3))
                .child(
                    div()
                        .text_xs()
                        .font_bold()
                        .text_color(level_color)
                        .child(level_text)
                )
        )
        .child(
            // Log content
            div()
                .flex_1()
                .text_xs()
                .text_color(cx.theme().foreground)
                .font_family("'Courier New', monospace")
                .child(line.content.clone())
        )
}
