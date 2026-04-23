//! SpeedGraph — a compact bar sparkline that visualises download speed over time.
//!
//! Usage:
//! ```rust
//! SpeedGraph::new(&speed_samples)   // &[f64] bytes/s values, oldest→newest
//!     .width(px(200.))
//!     .height(px(40.))
//! ```
//!
//! Renders as a row of proportional bars (histogram-style sparkline) — no
//! GPUI canvas paths required; pure-div layout so it works everywhere.

use gpui::{
    div, prelude::FluentBuilder as _, px, App, IntoElement, ParentElement, Pixels, RenderOnce,
    Styled, Window,
};

use crate::ActiveTheme;

// ── SpeedGraph ────────────────────────────────────────────────────────────────

/// A compact bar-sparkline for download speed history.
///
/// `samples` is a slice of bandwidth readings in **bytes/s**, oldest→newest.
/// By default the chart fills its container's full width (`w_full()`).  Pass
/// `.width(px(N))` to give it a fixed size instead.
#[derive(IntoElement)]
pub struct SpeedGraph {
    samples: Vec<f64>,
    /// `None` = fill available width (default).
    width: Option<Pixels>,
    height: Pixels,
}

impl SpeedGraph {
    pub fn new(samples: &[f64]) -> Self {
        Self {
            samples: samples.to_vec(),
            width: None, // full width by default
            height: px(36.),
        }
    }

    /// Give the chart a fixed pixel width instead of filling the container.
    pub fn width(mut self, w: Pixels) -> Self {
        self.width = Some(w);
        self
    }

    pub fn height(mut self, h: Pixels) -> Self {
        self.height = h;
        self
    }
}

impl RenderOnce for SpeedGraph {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let accent = cx.theme().accent;
        let accent_dim = cx.theme().accent.opacity(0.25);
        let h = self.height;

        let y_max = self
            .samples
            .iter()
            .cloned()
            .fold(0.0_f64, f64::max)
            .max(1.0);

        div()
            .map(|d| match self.width {
                Some(w) => d.w(w),
                None => d.w_full(),
            })
            .h(h)
            .rounded_sm()
            .overflow_hidden()
            .bg(accent_dim)
            .flex()
            .items_end() // bars grow upward from bottom
            .children(self.samples.iter().map(|&v| {
                let ratio = (v / y_max).clamp(0.0, 1.0) as f32;
                let bar_h = h * ratio;
                div()
                    .flex_1() // equal width, fills container
                    .h(bar_h)
                    .bg(accent)
            }))
    }
}

// ── Formatting helpers re-exported for convenience ────────────────────────────

/// Format bytes/sec as a human-readable string: "1.2 MB/s", "512 KB/s", etc.
pub fn fmt_speed(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1_000_000.0 {
        format!("{:.1} MB/s", bytes_per_sec / 1_000_000.0)
    } else if bytes_per_sec >= 1_000.0 {
        format!("{:.0} KB/s", bytes_per_sec / 1_000.0)
    } else {
        format!("{:.0} B/s", bytes_per_sec)
    }
}

/// Format byte count as human-readable: "12.3 MB", "4.5 GB", etc.
pub fn fmt_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.0} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{} B", bytes)
    }
}
