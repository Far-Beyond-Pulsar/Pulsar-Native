//! UI Utilities
//!
//! Helper functions for UI rendering

use gpui::Hsla;

/// Ensures minimum contrast between foreground and background colors
/// This is a simplified implementation - proper WCAG contrast calculation would be more complex
pub fn ensure_minimum_contrast(fg: Hsla, bg: Hsla, min_contrast: f32) -> Hsla {
    if min_contrast <= 0.0 {
        return fg;
    }

    // Simple luminance-based contrast adjustment
    // In a full implementation, this would use WCAG contrast ratio formulas
    let fg_lum = fg.l;
    let bg_lum = bg.l;

    let contrast = if fg_lum > bg_lum {
        (fg_lum + 0.05) / (bg_lum + 0.05)
    } else {
        (bg_lum + 0.05) / (fg_lum + 0.05)
    };

    // If contrast is sufficient, return original color
    if contrast * 100.0 >= min_contrast {
        return fg;
    }

    // Adjust lightness to meet minimum contrast
    // If bg is dark, make fg lighter; if bg is light, make fg darker
    let target_lum = if bg_lum < 0.5 {
        // Dark background - lighten foreground
        (min_contrast / 100.0) * (bg_lum + 0.05) - 0.05
    } else {
        // Light background - darken foreground
        (bg_lum + 0.05) / (min_contrast / 100.0) - 0.05
    };

    Hsla {
        h: fg.h,
        s: fg.s,
        l: target_lum.clamp(0.0, 1.0),
        a: fg.a,
    }
}
