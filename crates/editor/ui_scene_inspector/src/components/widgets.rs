//! Small pieces the panes share.

use gpui::{prelude::*, *};
use ui::{scroll::Scrollbar, v_flex, ActiveTheme, Icon, IconName};

use crate::screen::{ListScroll, SceneInspectorView};

/// The face of addresses, bytes and values.
pub const MONO: &str = "JetBrains Mono";

/// One of a row of choices.
pub fn chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    cx: &mut Context<SceneInspectorView>,
    on_click: impl Fn(&mut SceneInspectorView, &mut Context<SceneInspectorView>) + 'static,
) -> Stateful<Div> {
    let theme = cx.theme().clone();
    div()
        .id(id.into())
        .flex_none()
        .px_2()
        .py(px(3.0))
        .rounded(px(4.0))
        .text_xs()
        .cursor_pointer()
        .bg(if active {
            theme.primary.opacity(0.22)
        } else {
            theme.muted.opacity(0.25)
        })
        .text_color(if active {
            theme.foreground
        } else {
            theme.muted_foreground
        })
        .hover(|chip| chip.bg(theme.primary.opacity(0.14)))
        .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
        .child(label.into())
}

/// A label naming a group of chips.
pub fn caption(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .flex_none()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// A virtualized list filling its parent, with a scrollbar: `render` builds
/// only the rows on screen.
pub fn virtual_list(
    id: &'static str,
    count: usize,
    scroll: &ListScroll,
    cx: &mut Context<SceneInspectorView>,
    render: impl Fn(
            &mut SceneInspectorView,
            std::ops::Range<usize>,
            &mut Context<SceneInspectorView>,
        ) -> Vec<AnyElement>
        + 'static,
) -> Div {
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .w_full()
        .overflow_hidden()
        .child(
            uniform_list(
                id,
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    render(this, range, cx)
                }),
            )
            .size_full()
            .track_scroll(&scroll.handle),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .child(Scrollbar::uniform_scroll(&scroll.bar, &scroll.handle)),
        )
}

pub fn empty_state(
    icon: IconName,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .px_4()
        .text_center()
        .child(
            Icon::new(icon)
                .size_8()
                .text_color(theme.muted_foreground.opacity(0.7)),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.foreground)
                .child(title.into()),
        )
        .child(
            div()
                .max_w(px(420.0))
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(detail.into()),
        )
        .into_any_element()
}

/// The `index`th field colour: hues a golden ratio apart, so neighbours
/// never look alike.
pub fn region_color(index: usize) -> Hsla {
    let hue = (0.58 + index as f32 * 0.618_034).fract();
    hsla(hue, 0.65, 0.55, 1.0)
}
