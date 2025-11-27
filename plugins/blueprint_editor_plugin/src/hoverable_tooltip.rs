use gpui::{
    canvas, deferred, div, px, App, AppContext as _, Bounds, Context, Empty, Entity,
    InteractiveElement, IntoElement, ParentElement, Pixels, Point, Render,
    StatefulInteractiveElement, Styled, Window,
};

use ui::{v_flex, ActiveTheme as _, text::TextView};

/// A hoverable tooltip that stays open when the mouse moves into it
pub struct HoverableTooltip {
    content: String,
    position: Point<Pixels>,
    bounds: Bounds<Pixels>,
    pub(crate) open: bool, // Made pub(crate) so panel can check it
}

impl HoverableTooltip {
    pub fn new(content: String, position: Point<Pixels>, cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self {
            content,
            position,
            bounds: Bounds::default(),
            open: true,
        })
    }

    pub fn show(&mut self, cx: &mut Context<Self>) {
        self.open = true;
        cx.notify();
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    pub fn set_position(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.position = position;
        cx.notify();
    }

    /// Check if mouse is outside the tooltip bounds and hide if so
    /// Uses padding to create a "sticky" zone
    pub fn check_to_hide(&mut self, mouse_position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }

        // Add generous padding to create a buffer zone around the tooltip
        // This makes it easy to move mouse from trigger element onto tooltip
        let padding = px(30.);
        let bounds = Bounds {
            origin: self.bounds.origin.map(|v| v - padding),
            size: self.bounds.size.map(|v| v + padding * 2.),
        };

        if !bounds.contains(&mouse_position) {
            self.hide(cx);
        }
    }

    /// Check if mouse is inside tooltip bounds
    pub fn contains_point(&self, mouse_position: Point<Pixels>) -> bool {
        self.bounds.contains(&mouse_position)
    }
}

impl Render for HoverableTooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return Empty.into_any_element();
        }

        let view = cx.entity();
        let content = self.content.clone();

        // Calculate position with smart placement
        let window_bounds = window.bounds();
        let tooltip_width = px(400.0);
        let tooltip_max_height = px(300.0);

        let mut x = self.position.x;
        // Use the position as provided (already offset by caller)
        let mut y = self.position.y;

        // Keep tooltip within window bounds
        if x + tooltip_width > window_bounds.size.width {
            x = window_bounds.size.width - tooltip_width - px(10.0);
        }
        if x < px(10.0) {
            x = px(10.0);
        }

        if y + tooltip_max_height > window_bounds.size.height {
            // Show above cursor if no room below
            y = self.position.y - tooltip_max_height - px(10.0);
        }

        deferred(
            div()
                .id("hoverable-tooltip")
                .absolute()
                .left(x)
                .top(y)
                .on_scroll_wheel(|_, _, cx| {
                    // Stop scroll events from propagating to the graph
                    cx.stop_propagation();
                })
                .child(
                    v_flex()
                        .w(tooltip_width)
                        .max_h(tooltip_max_height)
                        .p_2()
                        .bg(cx.theme().popover)
                        .text_color(cx.theme().popover_foreground)
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded(cx.theme().radius)
                        .shadow_lg()
                        .overflow_hidden()
                        .child(
                            div()
                                .id("hoverable-tooltip-content")
                                .w_full()
                                .h_full()
                                .overflow_y_scroll()
                                .overflow_x_hidden()
                                .child(TextView::markdown("tooltip-content", &content, window, cx).selectable())
                        )
                        .child(
                            // Canvas to capture bounds for hit testing
                            canvas(
                                move |bounds, _, cx| {
                                    view.update(cx, |r, _| r.bounds = bounds)
                                },
                                |_, _, _, _| {},
                            )
                            .top_0()
                            .left_0()
                            .absolute()
                            .size_full(),
                        )
                )
        )
        .into_any_element()
    }
}