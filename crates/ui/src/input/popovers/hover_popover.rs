use std::{ops::Range, rc::Rc, time::Duration};

use gpui::{
    canvas, deferred, div, px, App, AppContext, Bounds, Context, Entity, InteractiveElement,
    IntoElement, ParentElement, Pixels, Point, Render, StatefulInteractiveElement, Styled, Task,
    Window,
};

use crate::{
    input::{popovers::render_markdown, InputState},
    v_flex, ActiveTheme, StyledExt,
};

const MAX_HOVER_WIDTH: Pixels = px(500.);
const MAX_HOVER_HEIGHT: Pixels = px(400.);
const HOVER_OFFSET_X: Pixels = px(15.); // Offset to the right of cursor
const HOVER_SHOW_DELAY: Duration = Duration::from_millis(300); // Quick 300ms delay

pub struct HoverPopover {
    editor: Entity<InputState>,
    /// The symbol range byte of the hover trigger.
    pub(crate) symbol_range: Range<usize>,
    pub(crate) hover: Option<Rc<lsp_types::Hover>>,
    /// Mouse position where hover was triggered (in window coordinates)
    pub(crate) mouse_position: Point<Pixels>,
    /// Bounds of the popover (for hit testing)
    pub(crate) bounds: Bounds<Pixels>,
    /// Whether the popover is currently visible
    pub(crate) visible: bool,
    /// Task to show the popover after delay
    _show_task: Task<()>,
}

impl HoverPopover {
    /// Create immediately, fetch LSP data async, show after delay
    pub fn new(
        editor: Entity<InputState>,
        symbol_range: Range<usize>,
        mouse_position: Point<Pixels>,
        cx: &mut App,
    ) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            editor,
            symbol_range,
            hover: None, // Will be set when LSP responds
            mouse_position,
            bounds: Bounds::default(),
            visible: false,
            _show_task: Task::ready(()),
        });

        // Start the show delay task
        entity.update(cx, |popover, cx| {
            popover.start_show_delay(cx);
        });

        entity
    }

    pub(crate) fn is_same(&self, offset: usize) -> bool {
        self.symbol_range.contains(&offset)
    }

    /// Set hover data when LSP responds (before or after delay)
    pub fn set_hover(&mut self, hover: lsp_types::Hover, cx: &mut Context<Self>) {
        self.hover = Some(Rc::new(hover));
        cx.notify();
    }

    /// Start the delay before showing the popover
    fn start_show_delay(&mut self, cx: &mut Context<Self>) {
        let entity = cx.entity();
        self._show_task = cx.spawn(async move |_, cx| {
            cx.background_executor().timer(HOVER_SHOW_DELAY).await;
            _ = entity.update(cx, |popover, cx| {
                popover.visible = true;
                cx.notify();
            });
        });
    }

    /// Check if mouse is inside the popover bounds (with padding)
    pub fn contains_point(&self, mouse_position: Point<Pixels>) -> bool {
        if !self.visible {
            return false;
        }

        // Add padding to create a "sticky" zone
        let padding = px(30.);
        let bounds = Bounds {
            origin: self.bounds.origin.map(|v| v - padding),
            size: self.bounds.size.map(|v| v + padding * 2.),
        };

        bounds.contains(&mouse_position)
    }
}

impl Render for HoverPopover {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        // Don't show until both visible AND hover data is ready
        if !self.visible || self.hover.is_none() {
            return div().into_any_element();
        }

        let Some(hover) = &self.hover else {
            return div().into_any_element();
        };

        let contents = match hover.contents.clone() {
            lsp_types::HoverContents::Scalar(scalar) => match scalar {
                lsp_types::MarkedString::String(s) => s,
                lsp_types::MarkedString::LanguageString(ls) => {
                    // Format code blocks properly
                    format!("```{}\n{}\n```", ls.language, ls.value)
                }
            },
            lsp_types::HoverContents::Array(arr) => arr
                .into_iter()
                .map(|item| match item {
                    lsp_types::MarkedString::String(s) => s,
                    lsp_types::MarkedString::LanguageString(ls) => {
                        format!("```{}\n{}\n```", ls.language, ls.value)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
            lsp_types::HoverContents::Markup(markup) => markup.value,
        };

        // Get the editor's input_bounds to convert window coords to element coords
        let editor = self.editor.read(cx);
        let element_origin = editor.input_bounds.origin;

        // Convert window coordinates to element coordinates
        // Window coords - element origin = element-relative coords
        let element_mouse_x = self.mouse_position.x - element_origin.x;
        let element_mouse_y = self.mouse_position.y - element_origin.y;

        // Position popover to the right of the mouse cursor (in element coordinates)
        let mut pos_x = element_mouse_x + HOVER_OFFSET_X;
        let mut pos_y = element_mouse_y;

        // Get element size for boundary checking
        let element_width = editor.input_bounds.size.width;
        let element_height = editor.input_bounds.size.height;

        // If popover would go off the right edge of the element, show it to the left of cursor
        if pos_x + MAX_HOVER_WIDTH > element_width {
            pos_x = (element_mouse_x - MAX_HOVER_WIDTH - HOVER_OFFSET_X).max(px(10.));
        }

        // If popover would go off the bottom of the element, move it up
        if pos_y + MAX_HOVER_HEIGHT > element_height {
            pos_y = (element_height - MAX_HOVER_HEIGHT - px(10.)).max(px(10.));
        }

        let max_width = MAX_HOVER_WIDTH.min(element_width - pos_x - px(20.));

        let view = cx.entity();

        deferred(
            div()
                .id("hover-popover")
                .absolute()
                .left(pos_x)
                .top(pos_y)
                .on_scroll_wheel(|_, _, cx| {
                    // Stop scroll events from propagating
                    cx.stop_propagation();
                })
                .child(
                    v_flex()
                        .w(max_width)
                        .max_h(MAX_HOVER_HEIGHT)
                        .min_w(px(200.))
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
                                .id("hover-popover-content")
                                .w_full()
                                .h_full()
                                .overflow_y_scroll()
                                .overflow_x_hidden()
                                .child(render_markdown("hover-content", contents, window, cx)),
                        )
                        .child(
                            // Canvas to capture bounds for hit testing
                            canvas(
                                move |bounds, _, cx| {
                                    view.update(cx, |popover, _| popover.bounds = bounds)
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full(),
                        ),
                )
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    // Close hover on click outside
                    this.editor.update(cx, |editor, cx| {
                        editor.hover_popover = None;
                        cx.notify();
                    });
                })),
        )
        .into_any_element()
    }
}
