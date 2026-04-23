use crate::{ActiveTheme, StyledExt};
use gpui::*;
use std::ops::Range;

/// Configuration for the minimap display
#[derive(Clone, Debug)]
pub struct MinimapConfig {
    /// Width of the minimap in pixels
    pub width: Pixels,
    /// Character width in the minimap (scaled down)
    pub char_width: f32,
    /// Line height in the minimap (scaled down)
    pub line_height: f32,
    /// Maximum number of characters to show per line in minimap
    pub max_chars_per_line: usize,
    /// Whether to show syntax highlighting in the minimap
    pub show_syntax_highlighting: bool,
    /// Scale factor for the minimap (smaller = more content visible)
    pub scale_factor: f32,
}

impl Default for MinimapConfig {
    fn default() -> Self {
        Self {
            width: px(120.0),
            char_width: 1.5,
            line_height: 2.0,
            max_chars_per_line: 80,
            show_syntax_highlighting: true,
            scale_factor: 0.15,
        }
    }
}

/// Minimap data that can be attached to a text editor
#[derive(Clone)]
pub struct MinimapData {
    /// Configuration
    pub config: MinimapConfig,
    /// Total number of lines
    pub total_lines: usize,
    /// Currently visible range in the main editor
    pub visible_range: Range<usize>,
    /// Content lines (truncated for minimap display)
    pub lines: Vec<String>,
}

impl MinimapData {
    pub fn new() -> Self {
        Self {
            config: MinimapConfig::default(),
            total_lines: 0,
            visible_range: 0..0,
            lines: Vec::new(),
        }
    }

    /// Update minimap data with new content
    pub fn update(&mut self, full_lines: &[String], visible_range: Range<usize>) {
        self.total_lines = full_lines.len();
        self.visible_range = visible_range;

        // Truncate lines for minimap display
        self.lines = full_lines
            .iter()
            .map(|line| {
                if line.len() > self.config.max_chars_per_line {
                    line.chars().take(self.config.max_chars_per_line).collect()
                } else {
                    line.clone()
                }
            })
            .collect();
    }

    /// Calculate the visible region bounds as a ratio (0.0 to 1.0)
    pub fn visible_region_ratio(&self) -> (f32, f32) {
        if self.total_lines == 0 {
            return (0.0, 1.0);
        }

        let start_ratio = self.visible_range.start as f32 / self.total_lines as f32;
        let height_ratio =
            (self.visible_range.end - self.visible_range.start) as f32 / self.total_lines as f32;

        (start_ratio, height_ratio)
    }
}

impl Default for MinimapData {
    fn default() -> Self {
        Self::new()
    }
}

/// Render a minimap overlay on a scrollbar
pub fn render_minimap(
    minimap_data: &MinimapData,
    bounds: Bounds<Pixels>,
    _window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let config = &minimap_data.config;
    let (visible_start_ratio, visible_height_ratio) = minimap_data.visible_region_ratio();

    div()
        .absolute()
        .top_0()
        .right_0()
        .w(config.width)
        .h_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        // Background
        .bg(cx.theme().background)
        .border_l_1()
        .border_color(cx.theme().border)
        // Content area - show truncated lines
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .h_full()
                .overflow_hidden()
                .children(minimap_data.lines.iter().enumerate().map(|(idx, line)| {
                    render_minimap_line(
                        line,
                        idx,
                        minimap_data.visible_range.contains(&idx),
                        config,
                        cx,
                    )
                })),
        )
        // Visible region overlay
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(relative(visible_start_ratio))
                .h(relative(visible_height_ratio))
                .bg(cx.theme().primary.opacity(0.15))
                .border_1()
                .border_color(cx.theme().primary.opacity(0.4)),
        )
}

fn render_minimap_line(
    text: &str,
    _line_number: usize,
    is_visible: bool,
    config: &MinimapConfig,
    cx: &App,
) -> impl IntoElement {
    let bg_color = if is_visible {
        cx.theme().primary.opacity(0.05)
    } else {
        cx.theme().transparent
    };

    div()
        .h(px(config.line_height))
        .w_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .bg(bg_color)
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().foreground.opacity(0.5))
                .child(text.to_string()),
        )
}
