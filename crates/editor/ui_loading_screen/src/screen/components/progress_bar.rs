use gpui::*;

pub(crate) fn progress_bar_widget(progress: f32, anim_tick: u32) -> impl IntoElement {
    let p = progress.clamp(0.0, 1.0);
    let bar_w = relative(p);

    let cycle = 90u32;
    let shine_t = ((anim_tick % cycle) as f32) / cycle as f32;
    let shine_left = relative((shine_t * p).clamp(0.0, (p - 0.06).max(0.0)));

    div()
        .w_full()
        .h(px(4.0))
        .bg(gpui::white().opacity(0.12))
        .relative()
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .h_full()
                .w(bar_w)
                .bg(gpui::white().opacity(0.85))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left(shine_left)
                        .h_full()
                        .w(relative(0.06))
                        .bg(gpui::white().opacity(0.5)),
                ),
        )
}
