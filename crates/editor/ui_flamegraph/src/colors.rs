//! Color palette for flamegraph spans

use gpui::*;

/// Get the professional color palette for flamegraph spans
///
/// Returns a vector of colors inspired by professional profiler tools
/// that provide good visual distinction between different spans.
pub fn get_palette() -> Vec<Hsla> {
    vec![
        // Professional color palette inspired by profiler tools
        hsla(210.0 / 360.0, 0.75, 0.55, 1.0), // Blue
        hsla(30.0 / 360.0, 0.80, 0.55, 1.0),  // Orange
        hsla(140.0 / 360.0, 0.70, 0.50, 1.0), // Green
        hsla(340.0 / 360.0, 0.75, 0.55, 1.0), // Pink
        hsla(270.0 / 360.0, 0.70, 0.55, 1.0), // Purple
        hsla(180.0 / 360.0, 0.65, 0.50, 1.0), // Cyan
        hsla(50.0 / 360.0, 0.75, 0.55, 1.0),  // Yellow
        hsla(10.0 / 360.0, 0.75, 0.55, 1.0),  // Red-Orange
        hsla(160.0 / 360.0, 0.70, 0.50, 1.0), // Teal
        hsla(290.0 / 360.0, 0.70, 0.55, 1.0), // Violet
        hsla(195.0 / 360.0, 0.70, 0.55, 1.0), // Sky Blue
        hsla(80.0 / 360.0, 0.65, 0.50, 1.0),  // Lime
        hsla(320.0 / 360.0, 0.75, 0.55, 1.0), // Magenta
        hsla(40.0 / 360.0, 0.75, 0.55, 1.0),  // Amber
        hsla(250.0 / 360.0, 0.70, 0.55, 1.0), // Indigo
        hsla(120.0 / 360.0, 0.70, 0.50, 1.0), // Emerald
    ]
}

/// Get thread-specific colors
pub fn get_thread_color(thread_id: u64) -> Rgba {
    match thread_id {
        0 => rgb(0xff6b6b), // GPU - Red
        1 => rgb(0x51cf66), // Main Thread - Green
        _ => rgb(0x74c0fc), // Workers - Blue
    }
}
