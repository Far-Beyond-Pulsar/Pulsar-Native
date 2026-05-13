//! VS Code / Zed-style minimap for the code editor.
//!
//! Rendering is cached per source line. Only lines that were dirtied by an
//! edit (or that have never been seen) are recomputed; all others are painted
//! directly from the cache with no string allocation and no tree-sitter query.

use std::{cell::Cell, cell::RefCell, rc::Rc};

use gpui::*;
use ropey::{LineType, Rope};

use crate::{highlighter::SyntaxHighlighter, ActiveTheme};

pub const MINIMAP_WIDTH: Pixels = px(110.0);
const LINE_PX: f32 = 2.0;
const CHAR_PX: f32 = 1.0;
const PAD_X: Pixels = px(4.0);
/// Extra lines beyond the edit range to invalidate for multi-line tokens.
const SYNTAX_LOOKAHEAD: usize = 30;

// ── Cached span (one coloured run of non-whitespace columns) ─────────────────

/// A single paint run within one minimap line.
///
/// `col` and `len` are in character columns (not bytes), so painting is just
/// `x = origin_x + PAD_X + col * CHAR_PX`.
#[derive(Clone, Copy)]
pub struct MinimapSpan {
    pub col: u16,
    pub len: u16,
    pub color: Hsla,
}

// ── Line cache ────────────────────────────────────────────────────────────────

/// Persistent cache of pre-computed minimap spans, one entry per source line.
///
/// `None` means the line is dirty and must be recomputed.
pub struct MinimapLineCache {
    pub lines: Vec<Option<Vec<MinimapSpan>>>,
    cached_total_lines: usize,
}

impl MinimapLineCache {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            cached_total_lines: 0,
        }
    }

    /// Resize to match the current document. New slots start dirty (None).
    pub fn resize(&mut self, total_lines: usize) {
        if self.cached_total_lines != total_lines {
            self.lines.resize_with(total_lines, || None);
            self.cached_total_lines = total_lines;
        }
    }

    /// Mark lines in `start_line..end_line + SYNTAX_LOOKAHEAD` as dirty.
    pub fn mark_dirty_range(&mut self, start_line: usize, end_line: usize) {
        let end = (end_line + SYNTAX_LOOKAHEAD).min(self.lines.len());
        let start = start_line.min(self.lines.len());
        for slot in &mut self.lines[start..end] {
            *slot = None;
        }
    }

    /// Wipe the entire cache (e.g., on full text replacement or theme change).
    pub fn invalidate_all(&mut self) {
        for slot in &mut self.lines {
            *slot = None;
        }
    }
}

// ── Drag state ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Default)]
pub struct MinimapDrag {
    pub active: bool,
    pub start_y: f32,
    pub start_offset_y: f32,
}

// ── MinimapState (persists in InputState across frames) ───────────────────────

pub struct MinimapState {
    pub drag: Rc<Cell<MinimapDrag>>,
    pub cache: Rc<RefCell<MinimapLineCache>>,
}

impl Default for MinimapState {
    fn default() -> Self {
        Self {
            drag: Rc::new(Cell::new(MinimapDrag::default())),
            cache: Rc::new(RefCell::new(MinimapLineCache::new())),
        }
    }
}

impl Clone for MinimapState {
    fn clone(&self) -> Self {
        Self {
            drag: self.drag.clone(),
            cache: self.cache.clone(),
        }
    }
}

impl MinimapState {
    pub fn new() -> Self {
        Self::default()
    }
}

// ── Element ───────────────────────────────────────────────────────────────────

pub struct Minimap {
    text: Rope,
    total_lines: usize,
    scroll_handle: ScrollHandle,
    content_height: Pixels,
    viewport_height: Pixels,
    editor_line_height: Pixels,
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
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Minimap {
    type RequestLayoutState = ();
    type PrepaintState = MinimapPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

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
        if total_lines == 0 || bounds.size.height < px(1.0) {
            return;
        }

        let container_h = bounds.size.height;

        // ── Scroll geometry ───────────────────────────────────────────────

        let editor_lh = self.editor_line_height.max(px(1.0));
        let content_h = self.content_height.max(self.viewport_height).max(px(1.0));
        let viewport_h = self.viewport_height.max(px(1.0));
        let max_scroll = (content_h - viewport_h).max(px(0.0));
        let scroll_abs = (-self.scroll_handle.offset().y).max(px(0.0));

        let editor_first_line = (scroll_abs / editor_lh) as usize;
        let editor_visible_lines = (viewport_h / editor_lh).ceil() as usize;
        let minimap_visible_lines = (container_h / px(LINE_PX)) as usize;

        let minimap_first_line = if total_lines <= minimap_visible_lines {
            0
        } else {
            let ideal = editor_first_line.saturating_sub(minimap_visible_lines / 3);
            ideal.min(total_lines.saturating_sub(minimap_visible_lines))
        };

        let last_minimap_line = (minimap_first_line + minimap_visible_lines).min(total_lines);

        // ── Background ────────────────────────────────────────────────────

        window.paint_quad(fill(bounds, cx.theme().secondary.opacity(0.35)));

        // ── Cached character rendering ────────────────────────────────────
        //
        // Acquire the cache for the duration of the render loop, then drop
        // it before registering mouse-event closures.

        let theme = &cx.theme().highlight_theme;
        let default_fg = cx.theme().foreground.opacity(0.55);
        let text = &self.text;
        let text_len = text.len();

        // Borrow the highlighter read-only (try_borrow so we never panic).
        // We hold it for the loop but drop before the closure section.
        let highlighter_guard = self
            .highlighter
            .as_ref()
            .and_then(|rc| rc.try_borrow().ok());
        let highlighter: Option<&SyntaxHighlighter> =
            highlighter_guard.as_ref().and_then(|g| g.as_ref());

        {
            let mut cache = self.drag_state.cache.borrow_mut();
            cache.resize(total_lines);

            use crate::input::RopeExt as _;

            for line_idx in minimap_first_line..last_minimap_line {
                if line_idx >= text.len_lines(LineType::LF) {
                    break;
                }

                let line_y = bounds.origin.y + px((line_idx - minimap_first_line) as f32 * LINE_PX);

                // Cache miss → compute and store.
                if cache.lines[line_idx].is_none() {
                    let byte_start = text.line_start_offset(line_idx);
                    let line_str = text.line(line_idx, LineType::LF).to_string();
                    let spans = compute_line_spans(
                        &line_str,
                        byte_start,
                        text_len,
                        highlighter,
                        theme,
                        default_fg,
                    );
                    cache.lines[line_idx] = Some(spans);
                }

                // Paint from cache — zero allocation on a cache hit.
                if let Some(spans) = &cache.lines[line_idx] {
                    paint_spans(window, spans, line_y, bounds.origin.x);
                }
            }
        } // cache borrow dropped here

        // ── Viewport indicator ────────────────────────────────────────────

        let indicator_top_line = editor_first_line.saturating_sub(minimap_first_line);
        let indicator_top = bounds.origin.y + px(indicator_top_line as f32 * LINE_PX);
        let indicator_h = px(editor_visible_lines as f32 * LINE_PX).max(px(8.0));
        let indicator_h =
            indicator_h.min(container_h - (indicator_top - bounds.origin.y).max(px(0.0)));

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

        let drag = self.drag_state.drag.clone();
        let scroll_handle = self.scroll_handle.clone();

        window.on_mouse_event({
            let drag = drag.clone();
            let scroll_handle = scroll_handle.clone();
            move |event: &MouseDownEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble || !bounds.contains(&event.position) {
                    return;
                }
                cx.stop_propagation();

                let click_frac =
                    f32::from(event.position.y - bounds.origin.y) / f32::from(container_h).max(1.0);
                let target_line =
                    minimap_first_line as f32 + click_frac * minimap_visible_lines as f32;
                let new_y = if max_scroll > px(0.0) {
                    -(max_scroll * (target_line / total_lines as f32).min(1.0))
                } else {
                    px(0.0)
                };
                let mut offset = scroll_handle.offset();
                offset.y = new_y;
                scroll_handle.set_offset(offset);

                drag.set(MinimapDrag {
                    active: true,
                    start_y: f32::from(event.position.y),
                    start_offset_y: f32::from(new_y),
                });
            }
        });

        window.on_mouse_event({
            let drag = drag.clone();
            let scroll_handle = scroll_handle.clone();
            move |event: &MouseMoveEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let state = drag.get();
                if !state.active {
                    return;
                }
                cx.stop_propagation();

                let delta_px = f32::from(event.position.y) - state.start_y;
                let lines_per_px = total_lines as f32 / minimap_visible_lines.max(1) as f32;
                let delta_scroll = editor_lh * (delta_px * lines_per_px);

                let new_y = px(state.start_offset_y) - delta_scroll;
                let clamped = new_y.min(px(0.0)).max(-max_scroll);
                let mut offset = scroll_handle.offset();
                offset.y = clamped;
                scroll_handle.set_offset(offset);
            }
        });

        window.on_mouse_event({
            move |_: &MouseUpEvent, phase, _window, cx| {
                let mut s = drag.get();
                if s.active {
                    s.active = false;
                    drag.set(s);
                    if phase == DispatchPhase::Bubble {
                        cx.stop_propagation();
                    }
                }
            }
        });
    }
}

// ── Span computation (called only on cache miss) ──────────────────────────────

fn compute_line_spans(
    line_str: &str,
    byte_start: usize,
    text_len: usize,
    highlighter: Option<&SyntaxHighlighter>,
    theme: &crate::highlighter::HighlightTheme,
    default_fg: Hsla,
) -> Vec<MinimapSpan> {
    let line_len = line_str.len();
    if line_len == 0 {
        return Vec::new();
    }

    // Get syntax-coloured byte ranges for this line.
    let style_spans: Vec<(std::ops::Range<usize>, Hsla)> = if let Some(h) = highlighter {
        let abs_end = (byte_start + line_len).min(text_len);
        h.styles(&(byte_start..abs_end), theme)
            .into_iter()
            .map(|(r, style)| {
                let start = r.start.saturating_sub(byte_start);
                let end = r.end.saturating_sub(byte_start).min(line_len);
                (start..end, style.color.unwrap_or(default_fg))
            })
            .collect()
    } else {
        vec![(0..line_len, default_fg)]
    };

    build_spans(line_str, &style_spans)
}

/// Convert byte-range style spans into character-column `MinimapSpan` runs,
/// merging consecutive non-whitespace characters of the same colour.
fn build_spans(line: &str, style_spans: &[(std::ops::Range<usize>, Hsla)]) -> Vec<MinimapSpan> {
    let mut result: Vec<MinimapSpan> = Vec::new();

    // Active run state
    let mut run_col: u16 = 0;
    let mut run_len: u16 = 0;
    let mut run_color: Hsla = Hsla::default();
    let mut in_run = false;

    let flush = |result: &mut Vec<MinimapSpan>, col, len, color| {
        if len > 0 {
            result.push(MinimapSpan { col, len, color });
        }
    };

    let color_for_byte = |byte_off: usize| -> Hsla {
        for (range, color) in style_spans {
            if range.start <= byte_off && byte_off < range.end {
                return *color;
            }
        }
        // Default: use the last span's colour, or transparent.
        style_spans
            .last()
            .map(|(_, c)| *c)
            .unwrap_or(Hsla::default())
    };

    for (char_idx, (byte_off, ch)) in line.char_indices().enumerate() {
        if char_idx > 800 {
            break;
        } // hard cap per line for the minimap

        if ch.is_whitespace() {
            if in_run {
                flush(&mut result, run_col, run_len, run_color);
                in_run = false;
                run_len = 0;
            }
        } else {
            let color = color_for_byte(byte_off);
            let same = in_run
                && color.h == run_color.h
                && color.s == run_color.s
                && color.l == run_color.l
                && color.a == run_color.a;

            if same {
                run_len += 1;
            } else {
                if in_run {
                    flush(&mut result, run_col, run_len, run_color);
                }
                run_col = char_idx as u16;
                run_len = 1;
                run_color = color;
                in_run = true;
            }
        }
    }
    if in_run {
        flush(&mut result, run_col, run_len, run_color);
    }

    result
}

/// Paint pre-computed spans — zero allocation, just quads.
#[inline]
fn paint_spans(window: &mut Window, spans: &[MinimapSpan], y: Pixels, origin_x: Pixels) {
    let left = origin_x + PAD_X;
    let right = origin_x + MINIMAP_WIDTH - PAD_X;

    for span in spans {
        let x = left + px(f32::from(span.col) * CHAR_PX);
        if x >= right {
            break;
        }
        let w = (px(f32::from(span.len) * CHAR_PX)).min(right - x);
        if w <= px(0.0) {
            break;
        }

        window.paint_quad(fill(
            Bounds::new(point(x, y), size(w, px(LINE_PX))),
            span.color,
        ));
    }
}
