//! Bottom status line: where the pointer and the view are on the timeline.

use crate::flamegraph_view::FlamegraphView;
use crate::format::format_duration;
use gpui::*;
use ui::{h_flex, ActiveTheme};

/// Its own entity, so the pointer moving over the timeline repaints this line
/// and nothing else of the window.
pub struct StatusBar {
    view: Entity<FlamegraphView>,
    _observe: Subscription,
}

impl StatusBar {
    pub fn new(view: Entity<FlamegraphView>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        Self {
            view,
            _observe: observe,
        }
    }
}

impl Render for StatusBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.view.read(cx).status();
        let theme = cx.theme();
        let (muted, foreground, border, background) = (
            theme.muted_foreground,
            theme.foreground,
            theme.border,
            theme.tab_bar,
        );

        let item = |label: &'static str, value: String| {
            h_flex()
                .gap_1()
                .items_center()
                .child(div().text_size(px(10.)).text_color(muted).child(label))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_family("monospace")
                        .text_color(foreground)
                        .child(value),
                )
        };

        let cursor = status
            .cursor_ns
            .map(format_duration)
            .unwrap_or_else(|| "–".to_string());
        let visible = format!(
            "{} → {}  ({})",
            format_duration(status.visible_start_ns),
            format_duration(status.visible_end_ns),
            format_duration(
                status
                    .visible_end_ns
                    .saturating_sub(status.visible_start_ns)
            ),
        );
        let frame = match status.frame_index {
            Some(index) if status.frame_count > 0 => {
                format!("{} / {}", index + 1, status.frame_count)
            }
            _ => "–".to_string(),
        };

        let mut bar = h_flex()
            .w_full()
            .h(px(24.))
            .flex_shrink_0()
            .px_3()
            .gap_4()
            .items_center()
            .bg(background)
            .border_t_1()
            .border_color(border)
            .child(item("Cursor", cursor))
            .child(item("View", visible))
            .child(item("Zoom", format!("{:.2} px/ms", status.px_per_ms)))
            .child(item("Frame", frame))
            .child(item("Threads", status.thread_count.to_string()));

        if let Some(span) = status.selected.as_ref().or(status.hovered.as_ref()) {
            bar = bar.child(div().flex_1()).child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(11.))
                    .text_color(foreground)
                    .child(format!(
                        "{}  {}",
                        span.name,
                        format_duration(span.duration_ns)
                    )),
            );
        }
        bar
    }
}
