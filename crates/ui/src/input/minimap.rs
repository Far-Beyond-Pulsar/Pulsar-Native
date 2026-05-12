//! VS Code / Zed-style minimap for the code editor.
//!
//! Renders a zoomed-out view of the document with actual character-level detail
//! (1 px wide × 2 px tall per character) coloured with the active syntax theme.
//!
//! **Scroll behaviour** (VSCode-style):
//! - When the document fits in the minimap it is shown in full.
//! - When it overflows, the minimap scrolls so the viewport-indicator stays
//!   visible, tracking the main editor proportionally.
//! - The user can click or drag the minimap to jump/scroll the main editor.

use std::{cell::Cell, rc::Rc, cell::RefCell};

use gpui::*;
use ropey::{LineType, Rope};

use crate::{
    highlighter::SyntaxHighlighter,
    ActiveTheme,
};

pub const MINIMAP_WIDTH: Pixels = px(110.0);
/// Pixel height of one source line in the minimap.
const LINE_PX: f32 = 2.0;
/// Pixel width of one character column in the minimap.
const CHAR_PX: f32 = 1.0;
/// Horizontal padding inside the minimap panel.
const PAD_X: Pixels = px(4.0);

// ── Drag state ───────────────────────────────────────────────────────────────

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
    /// Total content height (scroll_size.height from InputState).
    content_height: Pixels,
    /// Visible viewport height (input_bounds.size.height from InputState).
    viewport_height: Pixels,
    /// Editor line height — used to derive first-visible-line from scroll offset.
    editor_line_height: Pixels,
    /// Syntax highlighter from InputMode::CodeEditor, if initialised.
    highlighter: Option<Rc<RefCell<Option<SyntaxHighlighter>>>>,
    drag_state: MinimapState,
}

impl Minimap {
    pub fn new(
        text: Rope,
        total_lines: usize,
        scroll_handle: ScrollHandle,
        content_height: Pixels,
        viewport_height: Pixels,
        editor_line_height: Pixels,
        highlighter: Option<Rc<RefCell<Option<SyntaxHighlighter>>>>,
        drag_state: MinimapState,
    ) -> Self {
        Self {
            text,
            total_lines,
            scroll_handle,
            content_height,
            viewport_height,
            editor_line_height,
            highlighter,
            drag_state,
        }
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
        if total_lines == 0 || bounds.size.height < px(1.0) { return; }

        let container_h = bounds.size.height;

        // ── Scroll geometry ───────────────────────────────────────────────

        let editor_lh = self.editor_line_height.max(px(1.0));
        let content_h = self.content_height.max(self.viewport_height).max(px(1.0));
        let viewport_h = self.viewport_height.max(px(1.0));
        let max_scroll = (content_h - viewport_h).max(px(0.0));
        let scroll_abs = (-self.scroll_handle.offset().y).max(px(0.0));

        // First line visible in main editor
        let editor_first_line = (scroll_abs / editor_lh) as usize;
        let editor_visible_lines =
            (viewport_h / editor_lh).ceil() as usize;

        // How many lines fit in the minimap panel?
        let minimap_visible_lines = (container_h / px(LINE_PX)) as usize;

        // Compute the first line the minimap should show (VSCode algorithm):
        // Keep the viewport indicator roughly centred in the minimap.
        let minimap_first_line = if total_lines <= minimap_visible_lines {
            0
        } else {
            let ideal = editor_first_line.saturating_sub(minimap_visible_lines / 3);
            ideal.min(total_lines.saturating_sub(minimap_visible_lines))
        };

        // ── Background ────────────────────────────────────────────────────

        window.paint_quad(fill(bounds, cx.theme().secondary.opacity(0.35)));

        // ── Character-level text rendering ────────────────────────────────

        let theme = &cx.theme().highlight_theme;
        let default_fg = cx.theme().foreground.opacity(0.55);
        let highlighter_ref = self.highlighter.as_ref()
            .and_then(|rc| rc.try_borrow().ok()
                .and_then(|b| if b.is_some() { Some(rc.clone()) } else { None }));

        let text = &self.text;
        let last_minimap_line = (minimap_first_line + minimap_visible_lines).min(total_lines);

        // Byte offset of the first rendered line — O(log n) via rope index.
        use crate::input::RopeExt as _;
        let mut line_byte_offset: usize = text
            .line_start_offset(minimap_first_line.min(text.len_lines(LineType::LF)));

        for line_idx in minimap_first_line..last_minimap_line {
            if line_idx >= text.len_lines(LineType::LF) { break; }

            let line_y = bounds.origin.y
                + px((line_idx - minimap_first_line) as f32 * LINE_PX);

            let line_slice = text.line(line_idx, LineType::LF);
            let line_str = line_slice.to_string();
            let line_len = line_str.len(); // byte length

            // Get syntax colour spans for this line
            let style_spans: Vec<(std::ops::Range<usize>, Hsla)> = {
                if let Some(ref rc) = highlighter_ref {
                    if let Ok(guard) = rc.try_borrow() {
                        if let Some(ref h) = *guard {
                            let abs_range = line_byte_offset
                                ..(line_byte_offset + line_len).min(text.len());
                            h.styles(&abs_range, theme)
                                .into_iter()
                                .map(|(r, style)| {
                                    let start = r.start.saturating_sub(line_byte_offset);
                                    let end = r.end.saturating_sub(line_byte_offset).min(line_len);
                                    let color = style.color.unwrap_or(default_fg);
                                    (start..end, color)
                                })
                                .collect()
                        } else {
                            vec![(0..line_len, default_fg)]
                        }
                    } else {
                        vec![(0..line_len, default_fg)]
                    }
                } else {
                    vec![(0..line_len, default_fg)]
                }
            };

            paint_minimap_line(
                window,
                &line_str,
                &style_spans,
                line_y,
                bounds.origin.x,
            );

            line_byte_offset += line_len + 1; // +1 for '\n'
        }

        // ── Viewport indicator ────────────────────────────────────────────

        let indicator_top_line = editor_first_line.saturating_sub(minimap_first_line);
        let indicator_top = bounds.origin.y + px(indicator_top_line as f32 * LINE_PX);
        let indicator_h = px(editor_visible_lines as f32 * LINE_PX).max(px(8.0));
        // Clamp so it doesn't overflow the minimap
        let indicator_h = indicator_h.min(container_h - (indicator_top - bounds.origin.y).max(px(0.0)));

        if indicator_h > px(0.0) {
            window.paint_quad(PaintQuad {
                bounds: Bounds::new(
                    point(bounds.origin.x, indicator_top),
                    size(MINIMAP_WIDTH, indicator_h),
                ),
                corner_radii: Corners::all(px(0.0)),
                background: cx.theme().accent.opacity(0.10).into(),
                border_widths: Edges {
                    top: px(1.0),
                    bottom: px(1.0),
                    left: px(0.0),
                    right: px(0.0),
                },
                border_color: cx.theme().accent.opacity(0.5),
                border_style: BorderStyle::Solid,
            });
        }

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
                // Map minimap click to document position
                let target_line = minimap_first_line as f32
                    + click_frac * minimap_visible_lines as f32;
                let new_y = if max_scroll > px(0.0) {
                    -(max_scroll * (target_line / total_lines as f32).min(1.0))
                } else {
                    px(0.0)
                };
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

                let delta_px = f32::from(event.position.y) - state.start_y;
                // Dragging 1px in the minimap = (total_lines / minimap_visible_lines) editor lines
                let lines_per_minimap_px = total_lines as f32 / minimap_visible_lines.max(1) as f32;
                let delta_lines = delta_px * lines_per_minimap_px;
                let delta_scroll = editor_lh * delta_lines;

                let new_y = px(state.start_offset_y) - delta_scroll;
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

// ── Character rendering helper ───────────────────────────────────────────────

/// Paint one source line as tiny 1×LINE_PX character blocks.
///
/// Consecutive non-whitespace characters with the same colour are batched
/// into a single wider quad for performance.
fn paint_minimap_line(
    window: &mut Window,
    line: &str,
    spans: &[(std::ops::Range<usize>, Hsla)],
    y: Pixels,
    origin_x: Pixels,
) {
    let left = origin_x + PAD_X;
    let right = origin_x + MINIMAP_WIDTH - PAD_X;

    // We track an open "run" of consecutive non-whitespace chars that share a colour.
    let mut run_start_x: Option<f32> = None;
    let mut run_width: f32 = 0.0;
    let mut run_color = Hsla::default();

    let flush = |window: &mut Window, start: Option<f32>, w: f32, color: Hsla| {
        if let Some(sx) = start {
            if w > 0.0 {
                window.paint_quad(fill(
                    Bounds::new(point(px(sx), y), size(px(w), px(LINE_PX))),
                    color,
                ));
            }
        }
    };

    // Span iterator: for every byte position in the line find the colour.
    // We build a flat colour array per byte for simplicity with overlapping spans.
    // For performance on the minimap this is fine (lines are ≤ a few hundred bytes).
    let line_bytes = line.len();
    if line_bytes == 0 { return; }

    // Build a per-char colour map (index = char position, value = colour).
    // We iterate chars so multi-byte chars still take one pixel column.
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    if chars.is_empty() { return; }

    // Map each char's byte offset to a colour using span list.
    let color_for_byte = |byte_off: usize| -> Hsla {
        for (range, color) in spans {
            if range.start <= byte_off && byte_off < range.end {
                return *color;
            }
        }
        Hsla::default() // transparent — shouldn't happen
    };

    for (char_idx, (byte_off, ch)) in chars.iter().enumerate() {
        let x = f32::from(left) + char_idx as f32 * CHAR_PX;
        if px(x) >= right { break; }

        if ch.is_whitespace() {
            flush(window, run_start_x, run_width, run_color);
            run_start_x = None;
            run_width = 0.0;
        } else {
            let color = color_for_byte(*byte_off);
            // Check if we can extend the current run
            let same_color = run_start_x.is_some()
                && color.h == run_color.h
                && color.s == run_color.s
                && color.l == run_color.l
                && color.a == run_color.a;

            if same_color {
                run_width += CHAR_PX;
            } else {
                flush(window, run_start_x, run_width, run_color);
                run_start_x = Some(x);
                run_width = CHAR_PX;
                run_color = color;
            }
        }
    }
    flush(window, run_start_x, run_width, run_color);
}
