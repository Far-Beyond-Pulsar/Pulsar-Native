//! The canvas palette, taken from the active app theme so the graph blends with
//! whichever theme is selected.

use gpui::{App, Hsla};
use graphy_render::{Theme, ThemeColors};
use ui::ActiveTheme;

pub fn graph_theme(cx: &App) -> Theme {
    let t = cx.theme();
    let opaque = |c: Hsla| {
        let c = c.to_rgb();
        [c.r, c.g, c.b, 1.0]
    };
    Theme::from_colors(&ThemeColors {
        background: opaque(t.background),
        foreground: opaque(t.foreground),
        muted_foreground: opaque(t.muted_foreground),
        border: opaque(t.border),
        accent: opaque(t.primary),
    })
}
