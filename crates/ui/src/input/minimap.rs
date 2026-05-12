//! VS Code-style minimap — a proper GPUI Element with correct bounds at paint time.
//!
//! Features: density-bar content scaled to actual container, a viewport indicator
//! that tracks the scroll position, and click/drag to scroll.

use std::{cell::Cell, rc::Rc};

use gpui::*;
use ropey::{LineType, Rope};

use crate::ActiveTheme;

pub const MINIMAP_WIDTH: Pixels = px(110.0);
const LINE_PX: f32 = 2.0; // minimap pixels per source line

/// Persistent drag state stored in InputState.
#[derive(Clone, Copy, Default)]
pub struct MinimapDrag {
    pub active: bool,
    pub start_y: f32,
    pub start_offset_y: f32,
}

#[derive(Clone, Default)]
pub struct MinimapState(pub Rc<Cell<MinimapDrag>>);

impl MinimapState {
    pub fn new() -> Self { Self::default() }
}

// ── Element ─────────────────────────────────────────────────────────────────

pub struct Minimap {
    text: Rope,
    total_lines: usize,
    scroll_handle: ScrollHandle,
    /// scroll_size.height from InputState — total content height.
    content_height: Pixels,
    /// input_bounds.size.height from InputState — visible viewport height.
    viewport_height: Pixels,
    drag_state: MinimapState,
}

impl Minimap {
    pub fn new(
        text: Rope,
        total_lines: usize,
        scroll_handle: ScrollHandle,
        content_height: Pixels,
        viewport_height: Pixels,
        drag_state: MinimapState,
    ) -> Self {
        Self { text, total_lines, scroll_handle, content_height, viewport_height, drag_state }
    }
}

pub struct MinimapPrepaint {
    bounds: Bounds<Pixels>,
}

impl IntoElement for Minimap {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

impl Element for Minimap {
    type RequestLayoutState = ();
    type PrepaintState = MinimapPrepaint;

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
        style.size.width = MINIMAP_WIDTH.into();
        style.size.height = relative(1.0).into();
        style.position = Position::Absolute;
        style.inset.right = px(0.0).into();
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
    ) -> MinimapPrepaint {
        window.insert_hitbox(bounds, HitboxBehavior::default());
        MinimapPrepaint { bounds }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut MinimapPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let bounds = prepaint.bounds;
        let total_lines = self.total_lines;
        if total_lines == 0 { return; }

        let container_h = bounds.size.height;

        // Total document height in minimap-pixel units.
        let doc_minimap_h = px(total_lines as f32 * LINE_PX);

        // Background
        window.paint_quad(fill(bounds, cx.theme().secondary.opacity(0.35)));

        // ── Scroll math ───────────────────────────────────────────────────

        let content_h = self.content_height.max(self.viewport_height).max(px(1.0));
        let viewport_h = self.viewport_height.max(px(1.0));
        let max_scroll = (content_h - viewport_h).max(px(0.0));
        let scroll_abs = (-self.scroll_handle.offset().y).max(px(0.0));

        // Fraction of document scrolled (0..1)
        let scroll_frac = if max_scroll > px(0.0) {
            (scroll_abs / max_scroll).min(1.0) // f32
        } else {
            0.0_f32
        };

        // When the minimap is shorter than the full doc, slide it to follow scroll.
        let minimap_scroll_offset = if doc_minimap_h > container_h {
            (doc_minimap_h - container_h) * scroll_frac
        } else {
            px(0.0)
        };

        // ── Density bars ──────────────────────────────────────────────────

        let sample_rate = match total_lines {
            0..=2000  => 1,
            2001..=20_000 => 5,
            _          => 20,
        };

        let code_color = cx.theme().foreground.opacity(0.22);

        for line_idx in (0..total_lines).step_by(sample_rate.max(1)) {
            if line_idx >= self.text.len_lines(LineType::LF) { break; }

            let line_text = self.text.line(line_idx, LineType::LF).to_string();
            let trimmed_len = line_text.trim().len() as f32;
            if trimmed_len < 1.0 { continue; }

            let density = (trimmed_len / 80.0_f32).min(1.0);

            // Y in minimap coordinate space, offset for scroll
            let raw_y = px(line_idx as f32 * LINE_PX) - minimap_scroll_offset;
            if raw_y < px(0.0) || raw_y > container_h { continue; }

            let bar_y = bounds.origin.y + raw_y;
            let bar_w = (MINIMAP_WIDTH - px(8.0)) * density;
            let bar_h = px(LINE_PX * sample_rate as f32).max(px(1.0));

            window.paint_quad(fill(
                Bounds::new(
                    point(bounds.origin.x + px(4.0), bar_y),
                    size(bar_w, bar_h),
                ),
                code_color,
            ));
        }

        // ── Viewport indicator ────────────────────────────────────────────

        // Fraction of the document that is visible.
        let viewport_frac = (viewport_h / content_h).min(1.0); // f32

        // Where the visible window starts, as a fraction of total document.
        let viewport_start_frac = if content_h > px(0.0) {
            (scroll_abs / content_h).min(1.0) // f32
        } else {
            0.0_f32
        };

        let indicator_top = container_h * viewport_start_frac;
        let indicator_h = (container_h * viewport_frac).max(px(16.0)).min(container_h - indicator_top);

        window.paint_quad(PaintQuad {
            bounds: Bounds::new(
                point(bounds.origin.x, bounds.origin.y + indicator_top),
                size(MINIMAP_WIDTH, indicator_h),
            ),
            corner_radii: Corners::all(px(0.0)),
            background: cx.theme().accent.opacity(0.12).into(),
            border_widths: Edges { top: px(1.0), bottom: px(1.0), left: px(0.0), right: px(0.0) },
            border_color: cx.theme().accent.opacity(0.55),
            border_style: BorderStyle::Solid,
        });

        // ── Mouse events ──────────────────────────────────────────────────

        let drag_state = self.drag_state.clone();
        let scroll_handle = self.scroll_handle.clone();

        window.on_mouse_event({
            let drag_state = drag_state.clone();
            let scroll_handle = scroll_handle.clone();
            move |event: &MouseDownEvent, phase, _window, _cx| {
                if phase != DispatchPhase::Bubble || !bounds.contains(&event.position) { return; }

                let click_frac = f32::from(event.position.y - bounds.origin.y)
                    / f32::from(container_h).max(1.0);
                let new_y = -(max_scroll * click_frac.clamp(0.0, 1.0));
                let mut offset = scroll_handle.offset();
                offset.y = new_y;
                scroll_handle.set_offset(offset);

                drag_state.0.set(MinimapDrag {
                    active: true,
                    start_y: f32::from(event.position.y),
                    start_offset_y: f32::from(new_y),
                });
            }
        });

        window.on_mouse_event({
            let drag_state = drag_state.clone();
            let scroll_handle = scroll_handle.clone();
            move |event: &MouseMoveEvent, phase, _window, _cx| {
                if phase != DispatchPhase::Bubble { return; }
                let state = drag_state.0.get();
                if !state.active { return; }

                let delta_frac = (f32::from(event.position.y) - state.start_y)
                    / f32::from(container_h).max(1.0);
                let new_y = px(state.start_offset_y) - max_scroll * delta_frac;
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
