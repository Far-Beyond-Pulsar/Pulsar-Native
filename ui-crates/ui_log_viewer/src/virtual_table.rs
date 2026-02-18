//! Virtual scrolling table for displaying log lines efficiently

use gpui::{prelude::*, *};
use ui::{h_flex, ActiveTheme as _, StyledExt};
use crate::log_reader::LogLine;

/// Virtual scroll state
#[derive(Clone)]
pub struct VirtualScrollState {
    pub scroll_offset: f32,
    pub viewport_height: f32,
    pub line_height: f32,
    pub total_lines: usize,
    pub visible_start: usize,
    pub visible_end: usize,
    pub cache_start: usize,
    pub cache_end: usize,
}

impl VirtualScrollState {
    pub fn new() -> Self {
        Self {
            scroll_offset: 0.0,
            viewport_height: 600.0,
            line_height: 20.0,
            total_lines: 0,
            visible_start: 0,
            visible_end: 0,
            cache_start: 0,
            cache_end: 0,
        }
    }
    
    /// Update visible range based on scroll position
    pub fn update_visible_range(&mut self) {
        let start_line = (self.scroll_offset / self.line_height).floor() as usize;
        let visible_count = (self.viewport_height / self.line_height).ceil() as usize;
        
        self.visible_start = start_line;
        self.visible_end = (start_line + visible_count).min(self.total_lines);
    }
    
    /// Get the total content height
    pub fn content_height(&self) -> f32 {
        self.total_lines as f32 * self.line_height
    }
    
    /// Scroll to the bottom
    pub fn scroll_to_bottom(&mut self) {
        let max_scroll = (self.content_height() - self.viewport_height).max(0.0);
        self.scroll_offset = max_scroll;
        self.update_visible_range();
    }
    
    /// Check if currently at bottom
    pub fn is_at_bottom(&self) -> bool {
        let max_scroll = (self.content_height() - self.viewport_height).max(0.0);
        (self.scroll_offset - max_scroll).abs() < 1.0
    }
    
    /// Handle scroll event
    pub fn on_scroll(&mut self, delta_y: f32) {
        let old_offset = self.scroll_offset;
        self.scroll_offset = (self.scroll_offset + delta_y)
            .max(0.0)
            .min((self.content_height() - self.viewport_height).max(0.0));
        
        if (self.scroll_offset - old_offset).abs() > 0.1 {
            self.update_visible_range();
        }
    }
}

/// Render a virtual scrolling log table
pub fn render_virtual_log_table<V: 'static>(
    lines: &[LogLine],
    scroll_state: &VirtualScrollState,
    cx: &mut Context<V>,
) -> impl IntoElement {
    div()
        .id("log-scroll-container")
        .flex_1()
        .overflow_y_scroll()
        .bg(cx.theme().background)
        .child(
            div()
                .h(px(scroll_state.content_height()))
                .w_full()
                .relative()
                .child(
                    div()
                        .absolute()
                        .top(px(scroll_state.visible_start as f32 * scroll_state.line_height))
                        .w_full()
                        .children(lines.iter().skip(
                            scroll_state.visible_start.saturating_sub(scroll_state.cache_start)
                        ).take(
                            scroll_state.visible_end - scroll_state.visible_start
                        ).map(|line| {
                            render_log_line(line, cx)
                        }))
                )
        )
}

fn render_log_line<V: 'static>(line: &LogLine, cx: &mut Context<V>) -> impl IntoElement {
    // Parse log level from content
    let (level_color, level_icon) = if line.content.contains("ERROR") {
        (cx.theme().danger, "●")
    } else if line.content.contains("WARN") {
        (cx.theme().warning, "●")
    } else if line.content.contains("INFO") {
        (cx.theme().primary, "●")
    } else if line.content.contains("DEBUG") {
        (cx.theme().success, "●")
    } else {
        (cx.theme().muted_foreground, "●")
    };
    
    h_flex()
        .w_full()
        .h(px(20.))
        .px_2()
        .items_center()
        .gap_2()
        .border_b_1()
        .border_color(cx.theme().border.opacity(0.3))
        .hover(|s| s.bg(cx.theme().muted.opacity(0.5)))
        .child(
            // Line number
            div()
                .w(px(60.))
                .flex_shrink_0()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .text_right()
                .child(format!("{}", line.line_number))
        )
        .child(
            // Level indicator
            div()
                .w(px(12.))
                .flex_shrink_0()
                .text_xs()
                .font_bold()
                .text_color(level_color)
                .child(level_icon)
        )
        .child(
            // Log content
            div()
                .flex_1()
                .text_xs()
                .text_color(cx.theme().foreground)
                .overflow_hidden()
                .whitespace_nowrap()
                .child(line.content.clone())
        )
}


