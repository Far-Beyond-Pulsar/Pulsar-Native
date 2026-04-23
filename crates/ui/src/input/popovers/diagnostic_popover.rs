use std::rc::Rc;

use gpui::{
    deferred, div, px, App, AppContext as _, Bounds, Context, Empty, Entity, InteractiveElement,
    IntoElement, ParentElement, Pixels, Point, Render, StatefulInteractiveElement, Styled, Window,
};

use crate::{
    highlighter::DiagnosticEntry,
    input::{popovers::render_markdown, InputState},
    v_flex, ActiveTheme, StyledExt,
};

pub struct DiagnosticPopover {
    state: Entity<InputState>,
    pub(crate) diagnostic: Rc<DiagnosticEntry>,
    bounds: Bounds<Pixels>,
    open: bool,
}

impl DiagnosticPopover {
    pub fn new(
        diagnostic: &DiagnosticEntry,
        state: Entity<InputState>,
        cx: &mut App,
    ) -> Entity<Self> {
        let diagnostic = Rc::new(diagnostic.clone());

        cx.new(|_| Self {
            diagnostic,
            state,
            bounds: Bounds::default(),
            open: true,
        })
    }

    pub(crate) fn show(&mut self, cx: &mut Context<Self>) {
        self.open = true;
        cx.notify();
    }

    pub(crate) fn hide(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    pub(crate) fn check_to_hide(&mut self, mouse_position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }

        let padding = px(5.);
        let bounds = Bounds {
            origin: self.bounds.origin.map(|v| v - padding),
            size: self.bounds.size.map(|v| v + padding * 2.),
        };

        if !bounds.contains(&mouse_position) {
            self.hide(cx);
        }
    }
}

impl Render for DiagnosticPopover {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return Empty.into_any_element();
        }

        let message = self.diagnostic.message.clone();

        let (border, bg, fg) = (
            self.diagnostic.severity.border(cx),
            self.diagnostic.severity.bg(cx),
            self.diagnostic.severity.fg(cx),
        );

        // Get position for the popover
        let state = self.state.read(cx);
        let Some(last_layout) = state.last_layout.as_ref() else {
            return Empty.into_any_element();
        };

        let Some(_last_bounds) = state.last_bounds else {
            return Empty.into_any_element();
        };

        let (_, _, start_pos) = state.line_and_position_for_offset(self.diagnostic.range.start);
        let Some(start_pos) = start_pos else {
            return Empty.into_any_element();
        };

        let scroll_origin = state.scroll_handle.offset();
        let pos = scroll_origin + start_pos - state.input_bounds.origin
            + Point::new(px(0.), last_layout.line_height + px(4.));

        let max_width = px(400.).min(window.bounds().size.width - pos.x - px(20.));

        deferred(
            div()
                .id("diagnostic-popover")
                .absolute()
                .left(pos.x)
                .top(pos.y)
                .on_scroll_wheel(|_, _, cx| {
                    cx.stop_propagation();
                })
                .child(
                    v_flex()
                        .w(max_width)
                        .max_h(px(300.))
                        .min_w(px(200.))
                        .px_1()
                        .py_0p5()
                        .bg(bg)
                        .text_color(fg)
                        .border_1()
                        .border_color(border)
                        .rounded(cx.theme().radius)
                        .shadow_lg()
                        .overflow_hidden()
                        .child(
                            div()
                                .id("diagnostic-popover-content")
                                .w_full()
                                .h_full()
                                .overflow_y_scroll()
                                .overflow_x_hidden()
                                .child(render_markdown("diagnostic-message", message, window, cx)),
                        ),
                )
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.hide(cx);
                })),
        )
        .into_any_element()
    }
}
