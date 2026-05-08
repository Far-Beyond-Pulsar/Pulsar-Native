use gpui::{
    anchored, deferred, div, point, prelude::FluentBuilder, px, Action, AnyElement, AnyView,
    App, AppContext, Bounds, Context, Corner, Element, ElementId, GlobalElementId, Hitbox,
    HitboxBehavior, InspectorElementId, IntoElement, LayoutId, ParentElement, Pixels, Point,
    Render, SharedString, StyleRefinement, Styled, Window,
};
use std::{cell::RefCell, rc::Rc, time::Duration};

use crate::{h_flex, text::Text, ActiveTheme, Kbd, StyledExt};

const TOOLTIP_CURSOR_OFFSET: Pixels = px(5.0);
const TOOLTIP_STATIONARY_DELAY: Duration = Duration::from_secs(1);

enum TooltipContext {
    Text(Text),
    Element(Box<dyn Fn(&mut Window, &mut App) -> AnyElement>),
}

pub struct Tooltip {
    style: StyleRefinement,
    content: TooltipContext,
    key_binding: Option<Kbd>,
    action: Option<(Box<dyn Action>, Option<SharedString>)>,
}

impl Tooltip {
    /// Create a Tooltip with a text content.
    pub fn new(text: impl Into<Text>) -> Self {
        Self {
            style: StyleRefinement::default(),
            content: TooltipContext::Text(text.into()),
            key_binding: None,
            action: None,
        }
    }

    /// Create a Tooltip with a custom element.
    pub fn element<E, F>(builder: F) -> Self
    where
        E: IntoElement,
        F: Fn(&mut Window, &mut App) -> E + 'static,
    {
        Self {
            style: StyleRefinement::default(),
            key_binding: None,
            action: None,
            content: TooltipContext::Element(Box::new(move |window, cx| {
                builder(window, cx).into_any_element()
            })),
        }
    }

    /// Set Action to display key binding information for the tooltip if it exists.
    pub fn action(mut self, action: &dyn Action, context: Option<&str>) -> Self {
        self.action = Some((action.boxed_clone(), context.map(SharedString::new)));
        self
    }

    /// Set KeyBinding information for the tooltip.
    pub fn key_binding(mut self, key_binding: Option<Kbd>) -> Self {
        self.key_binding = key_binding;
        self
    }

    /// Build the tooltip and return it as an `AnyView`.
    pub fn build(self, _: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|_| self).into()
    }
}

pub fn smart_tooltip_anchor_and_position(window: &Window) -> (Corner, Point<Pixels>) {
    smart_tooltip_anchor_and_position_at(window.mouse_position(), window)
}

pub fn smart_tooltip_anchor_and_position_at(
    mouse: Point<Pixels>,
    window: &Window,
) -> (Corner, Point<Pixels>) {
    let bounds = window.bounds();
    let anchor = match (
        mouse.x > bounds.size.width / 2.0,
        mouse.y > bounds.size.height / 2.0,
    ) {
        (false, false) => Corner::TopLeft,
        (true, false) => Corner::TopRight,
        (false, true) => Corner::BottomLeft,
        (true, true) => Corner::BottomRight,
    };

    let x = match anchor {
        Corner::TopLeft | Corner::BottomLeft => mouse.x + TOOLTIP_CURSOR_OFFSET,
        Corner::TopRight | Corner::BottomRight => mouse.x - TOOLTIP_CURSOR_OFFSET,
    };
    let y = match anchor {
        Corner::TopLeft | Corner::TopRight => mouse.y + TOOLTIP_CURSOR_OFFSET,
        Corner::BottomLeft | Corner::BottomRight => mouse.y - TOOLTIP_CURSOR_OFFSET,
    };

    (anchor, point(x, y))
}

#[derive(Default)]
struct HoverTooltipSharedState {
    hovered: bool,
    visible: bool,
    mouse_position: Point<Pixels>,
    hover_generation: u64,
}

pub struct HoverTooltip {
    id: ElementId,
    trigger: Option<AnyElement>,
    tooltip_builder: Rc<dyn Fn(&mut Window, &mut App) -> AnyView>,
}

pub struct HoverTooltipElementState {
    trigger_layout_id: Option<LayoutId>,
    tooltip_layout_id: Option<LayoutId>,
    trigger_element: Option<AnyElement>,
    tooltip_element: Option<AnyElement>,
    hover_state: Rc<RefCell<HoverTooltipSharedState>>,
}

impl Default for HoverTooltipElementState {
    fn default() -> Self {
        Self {
            trigger_layout_id: None,
            tooltip_layout_id: None,
            trigger_element: None,
            tooltip_element: None,
            hover_state: Rc::new(RefCell::new(HoverTooltipSharedState::default())),
        }
    }
}

pub struct HoverTooltipPrepaintState {
    hitbox: Hitbox,
}

impl HoverTooltip {
    pub fn new(
        id: impl Into<ElementId>,
        trigger: impl IntoElement,
        tooltip_builder: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            trigger: Some(trigger.into_element().into_any_element()),
            tooltip_builder: Rc::new(tooltip_builder),
        }
    }

    fn with_element_state<R>(
        &mut self,
        id: &GlobalElementId,
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(&mut Self, &mut HoverTooltipElementState, &mut Window, &mut App) -> R,
    ) -> R {
        window.with_optional_element_state::<HoverTooltipElementState, _>(
            Some(id),
            |element_state, window| {
                let mut element_state = element_state.unwrap().unwrap_or_default();
                let result = f(self, &mut element_state, window, cx);
                (result, Some(element_state))
            },
        )
    }
}

impl IntoElement for HoverTooltip {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for HoverTooltip {
    type RequestLayoutState = HoverTooltipElementState;
    type PrepaintState = HoverTooltipPrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.with_element_state(id.unwrap(), window, cx, |this, element_state, window, cx| {
            let hover_state = element_state.hover_state.clone();
            let mut tooltip_layout_id = None;
            let mut tooltip_element = None;

            if hover_state.borrow().visible {
                let mouse_position = hover_state.borrow().mouse_position;
                let (anchor, position) = smart_tooltip_anchor_and_position_at(mouse_position, window);
                let builder = this.tooltip_builder.clone();

                let mut element = deferred(
                    anchored()
                        .anchor(anchor)
                        .snap_to_window_with_margin(px(8.))
                        .position(position)
                        .child(div().child(builder(window, cx))),
                )
                .with_priority(1)
                .into_any();

                tooltip_layout_id = Some(element.request_layout(window, cx));
                tooltip_element = Some(element);
            }

            let mut trigger_element = this
                .trigger
                .take()
                .unwrap_or_else(|| div().into_any_element());
            let trigger_layout_id = trigger_element.request_layout(window, cx);

            let layout_id = window.request_layout(
                gpui::Style::default(),
                Some(trigger_layout_id).into_iter().chain(tooltip_layout_id),
                cx,
            );

            (
                layout_id,
                HoverTooltipElementState {
                    trigger_layout_id: Some(trigger_layout_id),
                    tooltip_layout_id,
                    trigger_element: Some(trigger_element),
                    tooltip_element,
                    hover_state,
                },
            )
        })
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        if let Some(element) = &mut request_layout.trigger_element {
            element.prepaint(window, cx);
        }
        if let Some(element) = &mut request_layout.tooltip_element {
            element.prepaint(window, cx);
        }

        let trigger_bounds = request_layout
            .trigger_layout_id
            .map(|layout_id| window.layout_bounds(layout_id));
        let hitbox = window.insert_hitbox(trigger_bounds.unwrap_or_default(), HitboxBehavior::Normal);

        let _ = trigger_bounds;

        HoverTooltipPrepaintState { hitbox }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(mut element) = request_layout.trigger_element.take() {
            element.paint(window, cx);
        }

        if let Some(mut element) = request_layout.tooltip_element.take() {
            element.paint(window, cx);
        }

        let hitbox = prepaint.hitbox.clone();
        let hover_state = request_layout.hover_state.clone();
        let window_handle = window.window_handle().into();
        window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, window, cx| {
            if !phase.bubble() {
                return;
            }

            let hovered = hitbox.is_hovered(window);
            let mut state = hover_state.borrow_mut();

            if hovered {
                let mouse_moved = !state.hovered || state.mouse_position != event.position;
                if mouse_moved {
                    state.hovered = true;
                    state.visible = false;
                    state.mouse_position = event.position;
                    state.hover_generation = state.hover_generation.saturating_add(1);

                    let hover_generation = state.hover_generation;
                    let hover_state = hover_state.clone();
                    let window_handle = window_handle;
                    cx.spawn(async move |cx| {
                        cx.background_executor().timer(TOOLTIP_STATIONARY_DELAY).await;
                        let _ = cx.update_window(window_handle, |_root, window, _cx| {
                            let mut hover_state = hover_state.borrow_mut();
                            if hover_state.hovered
                                && hover_state.hover_generation == hover_generation
                                && !hover_state.visible
                            {
                                hover_state.visible = true;
                                window.refresh();
                            }
                        });
                    })
                    .detach();

                    window.refresh();
                }
                return;
            }

            if state.hovered || state.visible {
                state.hovered = false;
                state.visible = false;
                state.hover_generation = state.hover_generation.saturating_add(1);
                window.refresh();
            }
        });
    }
}

impl FluentBuilder for Tooltip {}
impl Styled for Tooltip {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}
impl Render for Tooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let key_binding = if let Some(key_binding) = &self.key_binding {
            Some(key_binding.clone())
        } else {
            if let Some((action, context)) = &self.action {
                Kbd::binding_for_action(
                    action.as_ref(),
                    context.as_ref().map(|s| s.as_ref()),
                    window,
                )
            } else {
                None
            }
        };

        div().child(
            // Wrap in a child, to ensure the left margin is applied to the tooltip
            h_flex()
                .font_family(".SystemUIFont")
                .m_3()
                .bg(cx.theme().popover)
                .text_color(cx.theme().popover_foreground)
                .bg(cx.theme().popover)
                .border_1()
                .border_color(cx.theme().border)
                .shadow_md()
                .rounded(px(6.))
                .justify_between()
                .py_0p5()
                .px_2()
                .text_sm()
                .gap_3()
                .refine_style(&self.style)
                .map(|this| {
                    this.child(div().map(|this| match self.content {
                        TooltipContext::Text(ref text) => this.child(text.clone()),
                        TooltipContext::Element(ref builder) => this.child(builder(window, cx)),
                    }))
                })
                .when_some(key_binding, |this, kbd| {
                    this.child(
                        div()
                            .text_xs()
                            .flex_shrink_0()
                            .text_color(cx.theme().muted_foreground)
                            .child(kbd.appearance(false)),
                    )
                }),
        )
    }
}
