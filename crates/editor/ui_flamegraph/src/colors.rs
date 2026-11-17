//! Color palette for flamegraph spans

use gpui::*;

/// Get thread-specific colors
pub fn get_thread_color(thread_id: u64) -> Rgba {
    match thread_id {
        0 => rgb(0xff6b6b), // GPU - Red
        1 => rgb(0x51cf66), // Main Thread - Green
        _ => rgb(0x74c0fc), // Workers - Blue
    }
}
