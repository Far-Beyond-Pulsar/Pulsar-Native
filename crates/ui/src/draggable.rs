use gpui::{prelude::*, *};
use std::rc::Rc;

use crate::Icon;
use crate::IconName;

/// A reusable drag-source wrapper with a visual drag handle that preserves
/// interactivity of child elements.
///
/// `T` must implement [`Render`] because GPUI requires the drag type to be
/// able to render its own ghost element during dragging.
///
/// # Visual behavior
///
/// - Shows a drag handle icon (default: left side)
/// - Hover state: subtle background highlight
/// - Content remains fully interactive (buttons, inputs, etc. work normally)
/// - Only the drag handle triggers dragging
///
/// # Example — basic use
///
/// ```rust,ignore
/// Draggable::new("my-item", my_payload)
///     .on_drag_start(|payload, _window, _cx| {
///         println!("started dragging: {:?}", payload);
///     })
///     .child(my_content)
/// ```
///
/// # Example — inside a GPUI entity
///
/// ```rust,ignore
/// Draggable::new(format!("file-{}", id), asset_payload)
///     .on_drag_start(cx.listener(|this, payload, window, cx| {
///         this.handle_drag_start(payload, window, cx);
///     }))
///     .child(
///         h_flex()
///             .child(icon)
///             .child(file_name_input)  // Fully interactive
///     )
/// ```
pub struct Draggable<T: Clone + Render + 'static> {
    id: ElementId,
    payload: T,
    on_drag_start: Option<Rc<dyn Fn(&T, &mut Window, &mut App)>>,
    drag_handle_position: DragHandlePosition,
    show_hover_state: bool,
    /// Inner content and styles. Children added via `ParentElement` land here;
    /// styles applied via `Styled` also land here.
    base: Div,
}

/// Position of the drag handle icon relative to the content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragHandlePosition {
    /// Drag handle on the left side (default)
    Left,
    /// Drag handle on the right side
    Right,
    /// No automatic drag handle (user provides their own)
    Custom,
}

impl<T: Clone + Render + 'static> Draggable<T> {
    /// Create a new draggable item with the given element ID and payload.
    ///
    /// The ID must be unique within the window for GPUI's event handling to work
    /// correctly. Use a descriptive string, e.g. `"file-item-42"`.
    ///
    /// The payload will be emitted when dragging starts and passed to drop targets.
    pub fn new(id: impl Into<ElementId>, payload: T) -> Self {
        Self {
            id: id.into(),
            payload,
            on_drag_start: None,
            drag_handle_position: DragHandlePosition::Left,
            show_hover_state: true,
            base: div(),
        }
    }

    /// Register a callback called when the user starts dragging via the drag handle.
    ///
    /// This is called once at drag start, making it ideal for triggering side effects
    /// like closing drawers.
    ///
    /// Inside a GPUI entity's `render` method, wrap the closure with
    /// `cx.listener(...)` to get access to `&mut Self`.
    pub fn on_drag_start(mut self, f: impl Fn(&T, &mut Window, &mut App) + 'static) -> Self {
        self.on_drag_start = Some(Rc::new(f));
        self
    }

    /// Set the position of the drag handle icon.
    ///
    /// Defaults to `DragHandlePosition::Left`.
    pub fn drag_handle(mut self, position: DragHandlePosition) -> Self {
        self.drag_handle_position = position;
        self
    }

    /// Enable or disable the hover state visual feedback.
    ///
    /// When enabled (default), hovering shows a subtle background highlight.
    pub fn show_hover_state(mut self, show: bool) -> Self {
        self.show_hover_state = show;
        self
    }
}

// ─── Trait impls ─────────────────────────────────────────────────────────────

impl<T: Clone + Render + 'static> ParentElement for Draggable<T> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.base.extend(elements);
    }
}

impl<T: Clone + Render + 'static> Styled for Draggable<T> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl<T: Clone + Render + 'static> RenderOnce for Draggable<T> {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let payload = self.payload.clone();
        let on_drag_start = self.on_drag_start;

        let drag_handle = match self.drag_handle_position {
            DragHandlePosition::Left | DragHandlePosition::Right => {
                let handle_payload = payload.clone();
                let handle_on_drag_start = on_drag_start.clone();

                Some(
                    div()
                        .flex_shrink_0()
                        .cursor_grab()
                        .p_1()
                        .text_color(gpui::rgb(0x80_80_80))
                        .hover(|style| style.text_color(gpui::rgb(0x40_40_40)))
                        .child(Icon::new(IconName::Drag).size_4())
                        .on_drag(handle_payload, move |drag, _, window, cx| {
                            // Call the on_drag_start handler if registered
                            if let Some(ref handler) = handle_on_drag_start {
                                handler(&drag, window, cx);
                            }
                            cx.new(|_| drag.clone())
                        }),
                )
            }
            DragHandlePosition::Custom => None,
        };

        let mut container = div()
            .id(self.id)
            .flex()
            .items_center()
            .gap_2();

        // Add hover state if enabled
        if self.show_hover_state {
            container = container.hover(|style| style.bg(gpui::rgb(0x00_00_00).opacity(0.05)));
        }

        // Add drag handle on the left
        if self.drag_handle_position == DragHandlePosition::Left {
            if let Some(handle) = drag_handle.clone() {
                container = container.child(handle);
            }
        }

        // Add the user's content
        container = container.child(self.base);

        // Add drag handle on the right
        if self.drag_handle_position == DragHandlePosition::Right {
            if let Some(handle) = drag_handle {
                container = container.child(handle);
            }
        }

        container
    }
}
