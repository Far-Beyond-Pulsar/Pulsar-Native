//! The call stack of the clicked span: the spans that called it, itself, and
//! the spans it called. Clicking a frame selects that span in the timeline.

use crate::flamegraph_view::{CallFrame, FlamegraphView, FrameRole};
use crate::format::format_duration;
use gpui::*;
use ui::{
    dock::{Panel, PanelEvent},
    h_flex, v_flex, ActiveTheme,
};

const ROW_HEIGHT: f32 = 24.0;
const INDENT: f32 = 12.0;

pub struct CallstackPanel {
    view: Entity<FlamegraphView>,
    focus_handle: FocusHandle,
    _observe: Subscription,
}

impl CallstackPanel {
    pub fn new(view: Entity<FlamegraphView>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        Self {
            view,
            focus_handle: cx.focus_handle(),
            _observe: observe,
        }
    }

    fn frame_row(&self, position: usize, depth_base: usize, frame: &CallFrame, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (foreground, muted, accent, hover, border) = (
            theme.foreground,
            theme.muted_foreground,
            theme.accent,
            theme.list_hover,
            theme.border,
        );
        let selected = frame.role == FrameRole::Selected;
        let level = match frame.role {
            FrameRole::Ancestor => position,
            FrameRole::Selected => depth_base,
            FrameRole::Child => depth_base + 1,
        };
        let index = frame.index;
        h_flex()
            .id(SharedString::from(format!("frame-{}", index)))
            .w_full()
            .h(px(ROW_HEIGHT))
            .pl(px(8.0 + INDENT * level as f32))
            .pr_2()
            .gap_2()
            .items_center()
            .cursor_pointer()
            .border_b_1()
            .border_color(border.opacity(0.2))
            .bg(if selected { accent.opacity(0.18) } else { transparent_black() })
            .hover(move |s| s.bg(if selected { accent.opacity(0.18) } else { hover }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.view.update(cx, |view, cx| view.select_span(index, cx));
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_xs()
                    .font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                    .text_color(if frame.role == FrameRole::Child { muted } else { foreground })
                    .child(frame.info.name.clone()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .font_family("monospace")
                    .text_color(muted)
                    .child(format!(
                        "{}  self {}",
                        format_duration(frame.info.duration_ns),
                        format_duration(frame.info.self_ns)
                    )),
            )
            .into_any_element()
    }
}

impl EventEmitter<PanelEvent> for CallstackPanel {}

ui_common::panel_boilerplate!(CallstackPanel);

impl Panel for CallstackPanel {
    fn panel_name(&self) -> &'static str {
        "flamegraph_callstack"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        div().child("Call stack").into_any_element()
    }
}

impl Render for CallstackPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let stack = self.view.read(cx).callstack();
        let theme = cx.theme();
        let (background, muted, border) = (theme.background, theme.muted_foreground, theme.border);

        if stack.is_empty() {
            return div()
                .size_full()
                .p_3()
                .bg(background)
                .text_xs()
                .text_color(muted)
                .child("Click a span in the timeline to see its call stack.")
                .into_any_element();
        }

        let ancestors = stack.iter().filter(|f| f.role == FrameRole::Ancestor).count();
        let children = stack.iter().filter(|f| f.role == FrameRole::Child).count();
        let rows: Vec<AnyElement> = stack
            .iter()
            .enumerate()
            .map(|(position, frame)| self.frame_row(position, ancestors, frame, cx))
            .collect();

        v_flex()
            .size_full()
            .bg(background)
            .child(
                div()
                    .flex_shrink_0()
                    .px_3()
                    .py_1p5()
                    .border_b_1()
                    .border_color(border)
                    .text_size(px(10.))
                    .text_color(muted)
                    .child(format!(
                        "{} caller{} · {} callee{}",
                        ancestors,
                        if ancestors == 1 { "" } else { "s" },
                        children,
                        if children == 1 { "" } else { "s" }
                    )),
            )
            .child(
                div()
                    .id("flamegraph-callstack-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }
}
