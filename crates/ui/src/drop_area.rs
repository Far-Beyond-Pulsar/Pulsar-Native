use gpui::{prelude::*, *};
use std::rc::Rc;
use crate::theme::ActiveTheme;

/// A reusable drop-target wrapper that shows accept/reject visual feedback
/// while a drag of type `T` is in flight over it.
///
/// `T` must implement [`Render`] because GPUI requires the drag type to be
/// able to render its own ghost element.
///
/// # Visual behaviour
///
/// - **Accepted drag** (predicate returns `true`): a 2px accent-coloured border
///   is drawn around the area.
/// - **Rejected drag** (predicate returns `false`): the area is dimmed to 40%
///   opacity to signal it won't accept the payload.
/// - **No drag in flight**: no visual change.
///
/// # Example — generic use
///
/// ```rust,ignore
/// DropArea::new("my-zone")
///     .can_accept(|payload: &MyDragType| payload.is_compatible())
///     .on_drop(|payload, _window, _cx| {
///         println!("dropped: {:?}", payload);
///     })
///     .child(my_content)
/// ```
///
/// # Example — inside a GPUI entity, using `cx.listener`
///
/// ```rust,ignore
/// DropArea::new("asset-zone")
///     .can_accept(|p: &AssetPayload| p.kind.is_mesh())
///     .on_drop(cx.listener(|this, payload, window, cx| {
///         this.handle_mesh_drop(payload, window, cx);
///     }))
///     .child(self.render_viewport(window, cx))
/// ```
pub struct DropArea<T: Clone + Render + 'static> {
    id: ElementId,
    can_accept: Rc<dyn Fn(&T) -> bool>,
    on_drop_handler: Option<Rc<dyn Fn(&T, &mut Window, &mut App)>>,
    /// Inner content and styles. Children added via `ParentElement` land here;
    /// styles applied via `Styled` also land here.
    base: Div,
}

impl<T: Clone + Render + 'static> DropArea<T> {
    /// Create a new drop area with the given element ID.
    ///
    /// The ID must be unique within the window for GPUI's hit-testing to work
    /// correctly. Use a descriptive string, e.g. `"level-editor-viewport"`.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            can_accept: Rc::new(|_| true),
            on_drop_handler: None,
            base: div().size_full(),
        }
    }

    /// Set the predicate that determines whether an incoming drag payload is
    /// acceptable.
    ///
    /// This is called on every frame while a drag of type `T` is over this
    /// area, so keep it allocation-free.
    ///
    /// Defaults to `|_| true` (accept everything).
    pub fn can_accept(mut self, f: impl Fn(&T) -> bool + 'static) -> Self {
        self.can_accept = Rc::new(f);
        self
    }

    /// Register a handler called when the user releases the drag over this
    /// area and `can_accept` returns `true`.
    ///
    /// Inside a GPUI entity's `render` method, wrap the closure with
    /// `cx.listener(...)` to get access to `&mut Self`.
    pub fn on_drop(mut self, f: impl Fn(&T, &mut Window, &mut App) + 'static) -> Self {
        self.on_drop_handler = Some(Rc::new(f));
        self
    }
}

// ─── Trait impls ─────────────────────────────────────────────────────────────

impl<T: Clone + Render + 'static> ParentElement for DropArea<T> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.base.extend(elements);
    }
}

impl<T: Clone + Render + 'static> Styled for DropArea<T> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl<T: Clone + Render + 'static> RenderOnce for DropArea<T> {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let can_accept_hover = self.can_accept.clone();
        let can_accept_drop = self.can_accept;
        let on_drop_handler = self.on_drop_handler;

        // `.id()` converts Div → Stateful<Div>, which is required for GPUI to
        // correctly track whether the drag is over this element.
        self.base
            .id(self.id)
            .drag_over::<T>(move |style, payload, _window, cx| {
                if can_accept_hover(payload) {
                    style
                        .border_2()
                        .border_color(cx.theme().accent)
                        .rounded(px(4.0))
                } else {
                    style.opacity(0.4)
                }
            })
            .on_drop::<T>(move |payload, window, cx| {
                if can_accept_drop(payload) {
                    if let Some(ref handler) = on_drop_handler {
                        handler(payload, window, cx);
                    }
                }
            })
    }
}
