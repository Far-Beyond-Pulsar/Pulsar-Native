//! Virtualized text editor utilities and configuration.
//!
//! This module provides helper functions and configuration for virtualized text editing.
//! The core caching is provided by OptimizedLineCache in line_cache.rs.
//!
//! For integration guide, see: INTEGRATION_GUIDE.md

use gpui::{px, Pixels, Point, Size};
use std::{cmp::min, ops::Range};

/// Configuration for the virtual text editor.
#[derive(Clone, Debug)]
pub struct VirtualEditorConfig {
    /// Show line numbers in the gutter.
    pub show_line_numbers: bool,

    /// Enable soft wrapping of long lines.
    pub soft_wrap: bool,

    /// Tab size in spaces.
    pub tab_size: usize,

    /// Font size in pixels.
    pub font_size: Pixels,

    /// Line height multiplier.
    pub line_height: f32,

    /// Number of extra lines to render above/below viewport (buffer zone).
    pub buffer_lines: usize,

    /// Maximum lines to keep in cache.
    pub max_cache_size: usize,
}

impl Default for VirtualEditorConfig {
    fn default() -> Self {
        Self {
            show_line_numbers: true,
            soft_wrap: true,
            tab_size: 4,
            font_size: px(14.0),
            line_height: 1.5,
            buffer_lines: 2, // Minimal buffer - only render visible + 2 lines above/below
            max_cache_size: 500,
        }
    }
}

/// Calculate the visible range of lines based on scroll offset and viewport height.
///
/// This is the studio-quality virtualization algorithm that determines which lines
/// need to be rendered with pixel-perfect accuracy.
///
/// # Arguments
///
/// * `scroll_offset` - Current scroll position (negative Y for scrolling down)
/// * `viewport_height` - Height of the visible viewport
/// * `line_height` - Height of each line
/// * `total_lines` - Total number of lines in the document
/// * `buffer_lines` - Number of extra lines to render above/below for smooth scrolling
///
/// # Returns
///
/// Range of line indices to render (0-based)
///
/// # Example
///
/// ```ignore
/// let visible_range = calculate_visible_range(
///     point(px(0.0), px(-200.0)),  // Scrolled down 200px
///     px(400.0),                    // 400px viewport
///     px(20.0),                     // 20px per line
///     10000,                        // 10k total lines
///     2                             // 2 line buffer (minimal)
/// );
/// // visible_range might be 8..23 (15 visible + 4 buffer total)
/// ```
pub fn calculate_visible_range(
    scroll_offset: Point<Pixels>,
    viewport_height: Pixels,
    line_height: Pixels,
    total_lines: usize,
    buffer_lines: usize,
) -> Range<usize> {
    if total_lines == 0 {
        return 0..0;
    }

    // Calculate first and last visible line with pixel-perfect precision
    let scroll_top = scroll_offset.y.abs();

    // Use floor for first visible to ensure we render from the first partially visible line
    // This ensures smooth rendering even at fractional scroll positions
    let first_visible = (scroll_top / line_height).floor() as usize;

    // Calculate exactly how many lines fit in the viewport
    // Use ceil to capture the last partially visible line
    let visible_line_count = (viewport_height / line_height).ceil() as usize;
    let last_visible = first_visible + visible_line_count;

    // Add minimal buffer zone - only what's needed for smooth high-speed scrolling
    // This is the key to performance: only render what you see + tiny buffer
    let start = first_visible.saturating_sub(buffer_lines);
    let end = (last_visible + buffer_lines).min(total_lines);

    // Ensure we have at least one line to render
    if start >= end {
        return start..(start + 1).min(total_lines);
    }

    // CRITICAL: Cap the maximum range to prevent rendering too many lines at once
    // This handles edge cases like very small line heights or zoom
    const MAX_VISIBLE_LINES: usize = 200; // Sensible maximum for any normal viewport
    if end - start > MAX_VISIBLE_LINES {
        return start..(start + MAX_VISIBLE_LINES);
    }

    start..end
}

/// Calculate the total content size for scrolling.
///
/// # Arguments
///
/// * `total_lines` - Total number of lines in the document
/// * `line_height` - Height of each line
/// * `max_line_width` - Maximum width of any line (for horizontal scrolling)
///
/// # Returns
///
/// Size of the scrollable content area
pub fn calculate_content_size(
    total_lines: usize,
    line_height: Pixels,
    max_line_width: Pixels,
) -> Size<Pixels> {
    let height = line_height * total_lines as f32;
    Size {
        width: max_line_width,
        height,
    }
}

/// Calculate optimal buffer size based on scroll velocity for high-speed scrolling.
///
/// When scrolling fast, we want to render a bit more to avoid blank frames.
/// When scrolling slowly or not at all, minimize buffer to save performance.
///
/// # Arguments
///
/// * `scroll_velocity` - Pixels per second scrolling speed (absolute value)
/// * `base_buffer` - Base buffer lines from config
///
/// # Returns
///
/// Optimal buffer line count
pub fn calculate_adaptive_buffer(scroll_velocity: f32, base_buffer: usize) -> usize {
    // If scrolling very fast (> 2000 px/s), double the buffer
    if scroll_velocity.abs() > 2000.0 {
        base_buffer * 2
    }
    // If scrolling moderately (> 500 px/s), add 50% more buffer
    else if scroll_velocity.abs() > 500.0 {
        base_buffer + (base_buffer / 2)
    }
    // Otherwise use base buffer
    else {
        base_buffer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px};

    #[test]
    fn test_visible_range_at_top() {
        let config = VirtualEditorConfig::default();
        let line_height = px(20.0);
        let viewport_height = px(100.0); // Can show 5 lines
        let scroll_offset = point(px(0.0), px(0.0));

        let range = calculate_visible_range(
            scroll_offset,
            viewport_height,
            line_height,
            100, // total lines
            config.buffer_lines,
        );

        // At top: show 0 + buffer down
        assert_eq!(range.start, 0);
        assert!(range.end >= 5); // At least 5 visible lines
        assert!(range.end <= 15); // 5 visible + 5 before (0) + 5 after = 10
    }

    #[test]
    fn test_visible_range_scrolled() {
        let line_height = px(20.0);
        let viewport_height = px(100.0);
        let scroll_offset = point(px(0.0), px(-200.0)); // Scrolled down 200px = 10 lines

        let range = calculate_visible_range(
            scroll_offset,
            viewport_height,
            line_height,
            100,
            5, // buffer
        );

        // Should be around line 10, with buffer zones
        assert!(range.start >= 5); // 10 - 5 buffer
        assert!(range.end >= 15); // 10 + 5 visible
        assert!(range.end <= 25); // 10 + 5 visible + 10 buffer (max)
    }

    #[test]
    fn test_visible_range_empty_document() {
        let range = calculate_visible_range(
            point(px(0.0), px(0.0)),
            px(100.0),
            px(20.0),
            0, // empty document
            5,
        );

        assert_eq!(range, 0..0);
    }

    #[test]
    fn test_visible_range_small_document() {
        let range = calculate_visible_range(
            point(px(0.0), px(0.0)),
            px(100.0),
            px(20.0),
            3, // only 3 lines
            5,
        );

        // Should not exceed document size
        assert!(range.end <= 3);
    }

    #[test]
    fn test_content_size_calculation() {
        let size = calculate_content_size(1000, px(20.0), px(800.0));

        assert_eq!(size.width, px(800.0));
        assert_eq!(size.height, px(20000.0)); // 1000 * 20
    }

    #[test]
    fn test_content_size_empty() {
        let size = calculate_content_size(0, px(20.0), px(800.0));

        assert_eq!(size.width, px(800.0));
        assert_eq!(size.height, px(0.0));
    }
}
