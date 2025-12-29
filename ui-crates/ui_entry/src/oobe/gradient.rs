//! Animated Gradient Background
//!
//! GPU-accelerated animated gradient background with smooth color transitions

use gpui::*;
use std::time::Instant;

/// Animated gradient state with multiple color stops that smoothly transition
#[derive(Clone)]
pub struct AnimatedGradient {
    start_time: Instant,
    /// Primary color hue (0-360)
    primary_hue: f32,
    /// Secondary color hue offset
    secondary_hue_offset: f32,
    /// Animation speed multiplier
    speed: f32,
}

impl Default for AnimatedGradient {
    fn default() -> Self {
        Self::new()
    }
}

impl AnimatedGradient {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            primary_hue: 220.0,      // Start with blue
            secondary_hue_offset: 60.0, // Purple/pink offset
            speed: 0.3,
        }
    }

    /// Create a vibrant blue-purple gradient (like Arc)
    pub fn arc_style() -> Self {
        Self {
            start_time: Instant::now(),
            primary_hue: 220.0,         // Blue
            secondary_hue_offset: 80.0, // Shifts to purple/magenta
            speed: 0.2,
        }
    }

    /// Create a sunset gradient
    pub fn sunset() -> Self {
        Self {
            start_time: Instant::now(),
            primary_hue: 20.0,          // Orange
            secondary_hue_offset: 40.0, // Pink/magenta
            speed: 0.15,
        }
    }

    /// Create an aurora gradient
    pub fn aurora() -> Self {
        Self {
            start_time: Instant::now(),
            primary_hue: 160.0,         // Teal
            secondary_hue_offset: 100.0, // Purple
            speed: 0.25,
        }
    }

    /// Get elapsed time in seconds
    fn elapsed_seconds(&self) -> f32 {
        self.start_time.elapsed().as_secs_f32()
    }

    /// Calculate current animated hue based on time
    fn animated_hue(&self) -> f32 {
        let t = self.elapsed_seconds() * self.speed;
        // Smooth oscillation using sine wave
        let offset = t.sin() * 30.0; // ±30 degree hue shift
        (self.primary_hue + offset).rem_euclid(360.0)
    }

    /// Get the current primary color
    pub fn primary_color(&self) -> Hsla {
        let hue = self.animated_hue() / 360.0;
        hsla(hue, 0.85, 0.55, 1.0)
    }

    /// Get the current secondary color
    pub fn secondary_color(&self) -> Hsla {
        let hue = (self.animated_hue() + self.secondary_hue_offset) / 360.0;
        hsla(hue.rem_euclid(1.0), 0.8, 0.45, 1.0)
    }

    /// Get an accent color for highlights
    pub fn accent_color(&self) -> Hsla {
        let hue = (self.animated_hue() + self.secondary_hue_offset / 2.0) / 360.0;
        hsla(hue.rem_euclid(1.0), 0.9, 0.7, 1.0)
    }

    /// Create a gradient fill string for CSS-like backgrounds
    /// Returns colors for a 3-stop gradient
    pub fn gradient_colors(&self) -> (Hsla, Hsla, Hsla) {
        let t = self.elapsed_seconds() * self.speed;
        
        // Three colors that shift over time
        let hue1 = (self.primary_hue + (t.sin() * 20.0)) / 360.0;
        let hue2 = (self.primary_hue + self.secondary_hue_offset + (t.cos() * 25.0)) / 360.0;
        let hue3 = (self.primary_hue + self.secondary_hue_offset * 1.5 + ((t * 0.7).sin() * 15.0)) / 360.0;

        let color1 = hsla(hue1.rem_euclid(1.0), 0.9, 0.5, 1.0);
        let color2 = hsla(hue2.rem_euclid(1.0), 0.85, 0.4, 1.0);
        let color3 = hsla(hue3.rem_euclid(1.0), 0.8, 0.35, 1.0);

        (color1, color2, color3)
    }

    /// Get background color for overlays (semi-transparent dark)
    pub fn overlay_bg(&self) -> Hsla {
        let hue = self.animated_hue() / 360.0;
        hsla(hue, 0.3, 0.1, 0.85)
    }
}
