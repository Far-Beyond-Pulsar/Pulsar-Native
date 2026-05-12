//! Custom thin scrollbar for the code editor — VS Code-style vertical bar.

use std::{cell::Cell, rc::Rc};

use gpui::*;

use crate::ActiveTheme;

pub const TRACK_WIDTH: Pixels = px(12.0);
const THUMB_INSET: Pixels = px(2.0);
const MIN_THUMB_HEIGHT: Pixels = px(24.0);

/// Persistent drag state (lives in InputState, shared via Rc<Cell>).
#[derive(Clone, Copy, Default)]
pub struct EditorScrollbarDrag {
    pub active: bool,
    pub start_y: f32,
    pub start_offset_y: f32,
}

#[derive(Clone, Default)]
pub struct EditorScrollbarState(pub Rc<Cell<EditorScrollbarDrag>>);

impl EditorScrollbarState {
    pub fn new() -> Self { Self::default() }
}

// ── Element ─────────────────────────────────────────────────────────────────

pub struct EditorScrollbar {
    scroll_handle: ScrollHandle,
    /// Total document height in pixels (scroll_size.height from InputState).
    content_height: Pixels,
    /// Visible editor viewport height (input_bounds.size.height from InputState).
    viewport_height: Pixels,
    drag_state: EditorScrollbarState,
    /// Right-side offset in px (MINIMAP_WIDTH when minimap is shown, else 0).
    right_offset: Pixels,
}

impl EditorScrollbar {
    pub fn new(
        scroll_handle: ScrollHandle,
        content_height: Pixels,
        viewport_height: Pixels,
        drag_state: EditorScrollbarState,
        right_offset: Pixels,
    ) -> Self {
        Self { scroll_handle, content_height, viewport_height, drag_state, right_offset }
    }

    fn thumb(&self, track_h: Pixels) -> (Pixels, Pixels) {
        let content_h = self.content_height.max(self.viewport_height).max(px(1.0));
        let viewport_h = self.viewport_height.max(px(1.0));
        let max_scroll = (content_h - viewport_h).max(px(0.0));

        let ratio = viewport_h / content_h; // f32
        let thumb_h = (track_h * ratio).max(MIN_THUMB_HEIGHT).min(track_h);

        let scroll_abs = (-self.scroll_handle.offset().y).max(px(0.0));
        let scroll_ratio = if max_scroll > px(0.0) {
            (scroll_abs / max_scroll).min(1.0) // f32
        } else {
            0.0_f32
        };

        let thumb_top = (track_h - thumb_h) * scroll_ratio;
        (thumb_top, thumb_h)
    }
}

pub struct EditorScrollbarPrepaint {
    bounds: Bounds<Pixels>,
    thumb_bounds: Bounds<Pixels>,
}

impl IntoElement for EditorScrollbar {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

impl Element for EditorScrollbar {
    type RequestLayoutState = ();
    type PrepaintState = EditorScrollbarPrepaint;

    fn id(&self) -> Option<ElementId> { None }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> { None }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = TRACK_WIDTH.into();
        style.size.height = relative(1.0).into();
        style.position = Position::Absolute;
        style.inset.right = self.right_offset.into();
        style.inset.top = px(0.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> EditorScrollbarPrepaint {
        let (thumb_top, thumb_h) = self.thumb(bounds.size.height);
        let thumb_bounds = Bounds::new(
            point(bounds.origin.x + THUMB_INSET, bounds.origin.y + thumb_top),
            size(TRACK_WIDTH - THUMB_INSET * 2.0, thumb_h),
        );
        window.insert_hitbox(bounds, HitboxBehavior::default());
        EditorScrollbarPrepaint { bounds, thumb_bounds }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut EditorScrollbarPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let bounds = prepaint.bounds;
        let thumb_bounds = prepaint.thumb_bounds;
        let track_h = bounds.size.height;
        let is_dragging = self.drag_state.0.get().active;

        // Track
        window.paint_quad(fill(bounds, cx.theme().secondary.opacity(0.3)));

        // Thumb
        let thumb_color = if is_dragging {
            cx.theme().muted_foreground.opacity(0.85)
        } else {
            cx.theme().muted_foreground.opacity(0.45)
        };
        window.paint_quad(PaintQuad {
            bounds: thumb_bounds,
            corner_radii: Corners::all(px(3.0)),
            background: thumb_color.into(),
            border_widths: Edges::all(px(0.0)),
            border_color: Hsla::default(),
            border_style: BorderStyle::Solid,
        });

        // ── Events ────────────────────────────────────────────────────────

        let drag_state = self.drag_state.clone();
        let scroll_handle = self.scroll_handle.clone();
        let content_height = self.content_height;
        let viewport_height = self.viewport_height;

        window.on_mouse_event({
            let drag_state = drag_state.clone();
            let scroll_handle = scroll_handle.clone();
            move |event: &MouseDownEvent, phase, _window, _cx| {
                if phase != DispatchPhase::Bubble || !bounds.contains(&event.position) { return; }

                let max_scroll = (content_height - viewport_height).max(px(0.0));

                if thumb_bounds.contains(&event.position) {
                    drag_state.0.set(EditorScrollbarDrag {
                        active: true,
                        start_y: f32::from(event.position.y),
                        start_offset_y: f32::from(scroll_handle.offset().y),
                    });
                } else {
                    // Click on track → jump, thumb centered on cursor
                    let click_y = event.position.y - bounds.origin.y;
                    let (_, thumb_h) = {
                        let ratio = (viewport_height / content_height.max(px(1.0))).min(1.0);
                        let th = (track_h * ratio).max(MIN_THUMB_HEIGHT).min(track_h);
                        (px(0.0), th)
                    };
                    let travel = (track_h - thumb_h).max(px(1.0));
                    let ratio = ((click_y - thumb_h / 2.0) / travel).clamp(0.0, 1.0);
                    let new_y = -(ratio * max_scroll);
                    let mut offset = scroll_handle.offset();
                    offset.y = new_y;
                    scroll_handle.set_offset(offset);
                }
            }
        });

        window.on_mouse_event({
            let drag_state = drag_state.clone();
            let scroll_handle = scroll_handle.clone();
            move |event: &MouseMoveEvent, phase, _window, _cx| {
                if phase != DispatchPhase::Bubble { return; }
                let state = drag_state.0.get();
                if !state.active { return; }

                let max_scroll = (content_height - viewport_height).max(px(0.0));
                let ratio = (viewport_height / content_height.max(px(1.0))).min(1.0);
                let thumb_h = (track_h * ratio).max(MIN_THUMB_HEIGHT).min(track_h);
                let travel = (track_h - thumb_h).max(px(1.0));

                let delta_y = f32::from(event.position.y) - state.start_y;
                let scroll_per_px = max_scroll / travel; // f32 (Pixels/Pixels)
                let new_y = px(state.start_offset_y) - px(delta_y) * scroll_per_px;
                let clamped = new_y.min(px(0.0)).max(-max_scroll);

                let mut offset = scroll_handle.offset();
                offset.y = clamped;
                scroll_handle.set_offset(offset);
            }
        });

        window.on_mouse_event({
            move |_: &MouseUpEvent, _phase, _window, _cx| {
                let mut s = drag_state.0.get();
                if s.active { s.active = false; drag_state.0.set(s); }
            }
        });
    }
}
