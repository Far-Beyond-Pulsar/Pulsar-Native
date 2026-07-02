use gpui::prelude::*;
use gpui::*;
use ui::ActiveTheme as _;

use crate::screen::EntryScreen;

pub fn render_progress_bar(
    progress: f32,
    cx: &mut Context<EntryScreen>,
) -> impl IntoElement {
    let theme = cx.theme();
    let clamped = progress.clamp(0.0, 1.0);

    div()
        .w_full()
        .h(px(6.0))
        .rounded_full()
        .bg(theme.border)
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(theme.accent)
                .w(px(clamped * 100.0)),
        )
        .into_any_element()
}
