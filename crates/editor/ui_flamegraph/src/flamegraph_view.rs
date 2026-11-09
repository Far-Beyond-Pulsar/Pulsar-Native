use crate::components::*;
use crate::constants::*;
use crate::coordinates::time_to_x;
use crate::lod_tree::LODTree;
use crate::rendering::renderer::FlamegraphRenderer;
use crate::rendering::types::{FlamegraphUniforms, GpuSpan};
use crate::state::{SpanCache, ViewState};
use crate::trace_data::{TraceData, TraceFrame};
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use ui::v_flex;
use ui::ActiveTheme;
use ui::PixelsExt;

mod render_parts;

const SPAN_HOVER_HEIGHT_SCALE: f32 = 0.8;

#[inline]
fn atomic_f32(value: f32) -> Arc<AtomicU32> {
    Arc::new(AtomicU32::new(value.to_bits()))
}

#[inline]
fn load_f32(value: &AtomicU32) -> f32 {
    f32::from_bits(value.load(Ordering::Relaxed))
}

#[inline]
fn store_f32(value: &AtomicU32, next: f32) {
    value.store(next.to_bits(), Ordering::Relaxed);
}

pub struct FlamegraphView {
    trace_data: TraceData,
    view_state: ViewState,
    cache: Option<(Arc<TraceFrame>, Arc<SpanCache>)>,
    viewport_width: Arc<AtomicU32>,
    viewport_height: Arc<AtomicU32>,
    viewport_origin_x: Arc<AtomicU32>,
    viewport_origin_y: Arc<AtomicU32>,
    graph_width: Arc<AtomicU32>,
    graph_origin_x: Arc<AtomicU32>,
    surface: Option<WgpuSurfaceHandle>,
    renderer: FlamegraphRenderer,
    /// Current LOD level index (cached from last paint).
    lod_level: Option<usize>,
    /// Vertical window key `(level, vertical_tile)` used to decide when the
    /// cached `lod_spans` need rebuilding. Quantizes pan to one vertical tile
    /// so smooth scrolling does not re-upload spans every frame.
    lod_vertical_key: Option<(usize, i64)>,
    /// Cached GpuSpans for the current LOD level + vertical window — rebuilt
    /// only when either changes.
    lod_spans: Option<Arc<Vec<GpuSpan>>>,
}

/// A span as the inspector and status bar describe it. Times are relative to
/// the start of the session.
#[derive(Clone, Debug)]
pub struct SpanInfo {
    pub name: String,
    pub thread: String,
    pub start_ns: u64,
    pub duration_ns: u64,
    pub self_ns: u64,
    pub depth: u32,
}

/// Emitted when a click picks a span.
pub struct SpanSelected;

impl EventEmitter<SpanSelected> for FlamegraphView {}

/// How a frame of the call stack relates to the selected span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameRole {
    Ancestor,
    Selected,
    Child,
}

/// One entry of the call stack of the selected span.
#[derive(Clone, Debug)]
pub struct CallFrame {
    /// Index into `TraceFrame::spans`.
    pub index: usize,
    pub info: SpanInfo,
    pub role: FrameRole,
}

/// Where the view is looking, for the status bar and the toolbar.
#[derive(Clone, Debug, Default)]
pub struct ViewStatus {
    pub session_ns: u64,
    pub thread_count: usize,
    /// Time under the pointer, when it is over the timeline.
    pub cursor_ns: Option<u64>,
    pub visible_start_ns: u64,
    pub visible_end_ns: u64,
    pub px_per_ms: f32,
    /// Frame at the centre of the view, and how many frames the session has.
    pub frame_index: Option<usize>,
    pub frame_count: usize,
    pub hovered: Option<SpanInfo>,
    pub selected: Option<SpanInfo>,
}

impl FlamegraphView {
    /// Current zoom in pixels per nanosecond, resolving the "fit" sentinel.
    fn effective_zoom(&self, duration_ns: u64) -> f32 {
        if self.view_state.zoom == 0.0 {
            let width = (load_f32(&self.viewport_width) - THREAD_LABEL_WIDTH).max(1.0);
            width / duration_ns.max(1) as f32
        } else {
            self.view_state.zoom
        }
    }

    fn span_info(&self, frame: &TraceFrame, cache: &SpanCache, index: usize) -> Option<SpanInfo> {
        let span = frame.spans.get(index)?;
        Some(SpanInfo {
            name: span.name.clone(),
            thread: frame
                .threads
                .get(&span.thread_id)
                .map(|t| t.name.clone())
                .unwrap_or_else(|| format!("Thread {}", span.thread_id)),
            start_ns: span.start_ns.saturating_sub(frame.min_time_ns),
            duration_ns: span.duration_ns,
            self_ns: cache
                .self_ns
                .get(index)
                .copied()
                .unwrap_or(span.duration_ns),
            depth: span.depth,
        })
    }

    /// A snapshot of the view for the status bar and inspector. Reads what the
    /// last paint built; it never builds the cache itself.
    pub fn status(&self) -> ViewStatus {
        let Some((frame, cache)) = &self.cache else {
            return ViewStatus::default();
        };
        let duration = frame.duration_ns();
        let zoom = self.effective_zoom(duration).max(1e-12);
        let width = (load_f32(&self.viewport_width) - THREAD_LABEL_WIDTH).max(1.0);
        let vs = &self.view_state;
        let to_ns = |px: f32| ((px / zoom).max(0.0) as u64).min(duration);

        let cursor_ns = (vs.mouse_x >= THREAD_LABEL_WIDTH)
            .then(|| to_ns(vs.mouse_x - THREAD_LABEL_WIDTH - vs.pan_x));
        let visible_start_ns = to_ns(-vs.pan_x);
        let visible_end_ns = to_ns(width - vs.pan_x);
        let centre = frame.min_time_ns + (visible_start_ns + visible_end_ns) / 2;
        let boundaries = &frame.frame_boundaries_ns;
        let frame_index = boundaries.partition_point(|b| *b <= centre).checked_sub(1);

        ViewStatus {
            session_ns: duration,
            thread_count: cache.thread_rows.len(),
            cursor_ns,
            visible_start_ns,
            visible_end_ns,
            px_per_ms: zoom * 1_000_000.0,
            frame_index,
            frame_count: boundaries.len(),
            hovered: vs
                .hovered_span
                .and_then(|i| self.span_info(frame, cache, i)),
            selected: vs
                .selected_span
                .and_then(|i| self.span_info(frame, cache, i)),
        }
    }

    /// Show the whole session.
    pub fn fit_all(&mut self, cx: &mut Context<Self>) {
        // Zoom 0 is "fit": the next paint derives it from the viewport.
        self.view_state.zoom = 0.0;
        self.view_state.pan_x = 0.0;
        self.view_state.pan_y = 0.0;
        cx.notify();
    }

    /// Zoom about the centre of the timeline.
    pub fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        let Some((frame, _)) = &self.cache else {
            return;
        };
        let duration = frame.duration_ns();
        if duration == 0 {
            return;
        }
        let width = (load_f32(&self.viewport_width) - THREAD_LABEL_WIDTH).max(1.0);
        let old_zoom = self.effective_zoom(duration);
        let new_zoom = (old_zoom * factor).max(1e-12);
        let centre = width * 0.5;
        let world = (centre - self.view_state.pan_x) / old_zoom;
        self.view_state.zoom = new_zoom;
        self.view_state.pan_x = centre - world * new_zoom;
        cx.notify();
    }

    /// Fit the previous (`-1`) or next (`1`) frame.
    pub fn step_frame(&mut self, direction: i32, cx: &mut Context<Self>) {
        let Some((frame, _)) = self.cache.clone() else {
            return;
        };
        let boundaries = &frame.frame_boundaries_ns;
        if boundaries.is_empty() {
            return;
        }
        let status = self.status();
        let current = status.frame_index;
        let next = match (current, direction.signum()) {
            (None, _) => 0,
            (Some(i), d) => (i as i64 + d as i64).clamp(0, boundaries.len() as i64 - 1) as usize,
        };
        let start = boundaries[next];
        let end = boundaries
            .get(next + 1)
            .copied()
            .unwrap_or(frame.min_time_ns + frame.duration_ns());
        self.fit_bottom_view_to_segment(&frame, start, end);
        cx.notify();
    }

    /// The selected span with the spans that called it (outermost first) and
    /// the spans it called directly. Empty while nothing is selected.
    pub fn callstack(&self) -> Vec<CallFrame> {
        const SCAN_LIMIT: usize = 8192;
        const MAX_CHILDREN: usize = 200;
        let (Some((frame, cache)), Some(selected)) = (&self.cache, self.view_state.selected_span)
        else {
            return Vec::new();
        };
        let Some(span) = frame.spans.get(selected) else {
            return Vec::new();
        };
        let Some(&row_y) = cache.thread_offsets.get(&span.thread_id) else {
            return Vec::new();
        };
        let row = cache.thread_rows.partition_point(|l| l.row.y < row_y);
        let Some(layout) = cache.thread_rows.get(row) else {
            return Vec::new();
        };
        let indices = &layout.span_indices;

        // Ancestors: one enclosing span per shallower depth, found walking back
        // from the selected span's start.
        let at_or_before =
            indices.partition_point(|i| frame.spans[*i as usize].start_ns <= span.start_ns);
        let mut ancestors: Vec<Option<usize>> = vec![None; span.depth as usize];
        let mut missing = span.depth as usize;
        for k in (at_or_before.saturating_sub(SCAN_LIMIT)..at_or_before).rev() {
            if missing == 0 {
                break;
            }
            let index = indices[k] as usize;
            let candidate = &frame.spans[index];
            let depth = candidate.depth as usize;
            if index != selected
                && depth < ancestors.len()
                && ancestors[depth].is_none()
                && candidate.start_ns <= span.start_ns
                && candidate.end_ns() >= span.end_ns()
            {
                ancestors[depth] = Some(index);
                missing -= 1;
            }
        }

        let make = |index: usize, role: FrameRole| {
            self.span_info(frame, cache, index)
                .map(|info| CallFrame { index, info, role })
        };
        let mut stack: Vec<CallFrame> = ancestors
            .into_iter()
            .flatten()
            .filter_map(|index| make(index, FrameRole::Ancestor))
            .collect();
        stack.extend(make(selected, FrameRole::Selected));

        // Children: spans one level deeper that start inside the selected one.
        let from = indices.partition_point(|i| frame.spans[*i as usize].start_ns < span.start_ns);
        let mut children = 0;
        for k in from..indices.len().min(from + SCAN_LIMIT) {
            let index = indices[k] as usize;
            let candidate = &frame.spans[index];
            if candidate.start_ns >= span.end_ns() {
                break;
            }
            if index != selected
                && candidate.depth == span.depth + 1
                && candidate.end_ns() <= span.end_ns()
            {
                stack.extend(make(index, FrameRole::Child));
                children += 1;
                if children == MAX_CHILDREN {
                    break;
                }
            }
        }
        stack
    }

    /// Select a span by index (clicking a frame of the call stack).
    pub fn select_span(&mut self, index: usize, cx: &mut Context<Self>) {
        self.view_state.selected_span = Some(index);
        cx.emit(SpanSelected);
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.view_state.selected_span = None;
        cx.notify();
    }

    /// Compute the LOD level index from view state and viewport width.
    fn lod_level_for(&self, viewport_w: f32, frame: &TraceFrame) -> usize {
        let zoom = if self.view_state.zoom == 0.0 && frame.duration_ns() > 0 {
            viewport_w / frame.duration_ns() as f32
        } else {
            self.view_state.zoom
        };
        let pixels_per_ns = zoom.max(1e-10) as f64;
        // Can't call LODTree::select_level without the tree — we compute inline.
        // Target: bucket >= 4.0 / pixels_per_ns
        let min_bucket = (4.0 / pixels_per_ns) as u64;
        let sizes: &[u64] = &[
            50_000,
            100_000,
            500_000,
            1_000_000,
            5_000_000,
            10_000_000,
            50_000_000,
            100_000_000,
            200_000_000,
            500_000_000,
            1_000_000_000,
        ];
        if min_bucket >= sizes[sizes.len() - 1] {
            return sizes.len() - 1;
        }
        let mut best = sizes.len() - 1;
        let mut best_diff = u64::MAX;
        for (i, &s) in sizes.iter().enumerate() {
            if s >= min_bucket {
                let diff = s - min_bucket;
                if diff < best_diff {
                    best_diff = diff;
                    best = i;
                }
            }
        }
        best
    }

    /// Rebuild cached spans from the LOD tree at the given level, limited to
    /// `[y_min, y_max]` (a padded vertical window around the viewport).
    fn rebuild_lod(
        &mut self,
        level: usize,
        vertical_tile: i64,
        frame: &TraceFrame,
        lod_tree: &LODTree,
        y_min: f32,
        y_max: f32,
    ) {
        let spans = Arc::new(lod_tree.collect_level_gpu_spans_ybounded(
            level,
            frame.min_time_ns,
            y_min,
            y_max,
        ));
        self.lod_level = Some(level);
        self.lod_vertical_key = Some((level, vertical_tile));
        self.lod_spans = Some(spans);
    }
    pub fn new(trace_data: TraceData) -> Self {
        Self {
            trace_data,
            view_state: ViewState::default(),
            cache: None,
            viewport_width: atomic_f32(1920.0),
            viewport_height: atomic_f32(1080.0),
            viewport_origin_x: atomic_f32(0.0),
            viewport_origin_y: atomic_f32(0.0),
            graph_width: atomic_f32(1920.0),
            graph_origin_x: atomic_f32(0.0),
            surface: None,
            renderer: FlamegraphRenderer::new(),
            lod_level: None,
            lod_vertical_key: None,
            lod_spans: None,
        }
    }

    fn graph_x_to_time_ns(&self, frame: &TraceFrame, local_x: f32) -> Option<u64> {
        let duration_ns = frame.duration_ns();
        if duration_ns == 0 {
            return None;
        }

        let width = load_f32(&self.graph_width).max(1.0);
        let clamped_x = local_x.clamp(0.0, width);
        let ratio = clamped_x / width;
        Some(frame.min_time_ns + (ratio * duration_ns as f32) as u64)
    }

    fn center_bottom_view_on_time(&mut self, frame: &TraceFrame, time_ns: u64) {
        let duration_ns = frame.duration_ns();
        if duration_ns == 0 {
            return;
        }

        let viewport_width = load_f32(&self.viewport_width);
        let effective_width = (viewport_width - THREAD_LABEL_WIDTH).max(1.0);
        let zoom = if self.view_state.zoom == 0.0 {
            effective_width / duration_ns as f32
        } else {
            self.view_state.zoom
        };

        let offset_ns = time_ns.saturating_sub(frame.min_time_ns) as f32;
        self.view_state.pan_x = (effective_width * 0.5) - (offset_ns * zoom);
    }

    fn fit_bottom_view_to_segment(&mut self, frame: &TraceFrame, start_ns: u64, end_ns: u64) {
        let duration_ns = frame.duration_ns();
        if duration_ns == 0 {
            return;
        }

        let start = start_ns.min(end_ns).max(frame.min_time_ns);
        let end = end_ns.max(start_ns).min(frame.min_time_ns + duration_ns);
        let selected_duration = end.saturating_sub(start);
        if selected_duration == 0 {
            return;
        }

        let viewport_width = load_f32(&self.viewport_width);
        let effective_width = (viewport_width - THREAD_LABEL_WIDTH).max(1.0);
        let new_zoom = effective_width / selected_duration as f32;

        self.view_state.zoom = new_zoom;
        self.view_state.pan_x = -((start - frame.min_time_ns) as f32 * new_zoom);
    }

    fn get_or_build_cache(&mut self) -> (Arc<TraceFrame>, Arc<SpanCache>) {
        let started = std::time::Instant::now();
        let frame = self.trace_data.get_frame();

        let needs_rebuild = match &self.cache {
            Some((cached_frame, _)) => !Arc::ptr_eq(cached_frame, &frame),
            None => true,
        };

        if needs_rebuild {
            let cache_started = std::time::Instant::now();
            let cache = Arc::new(SpanCache::build(&frame));

            if self.view_state.zoom == 0.0 && frame.duration_ns() > 0 {
                let effective_width = self.view_state.viewport_width - THREAD_LABEL_WIDTH;
                self.view_state.zoom = effective_width / frame.duration_ns() as f32;
                self.view_state.pan_x = 0.0;
            }

            self.cache = Some((Arc::clone(&frame), cache));
            tracing::warn!(
                target: "flamegraph.workload",
                spans = frame.spans.len(),
                threads = frame.threads.len(),
                cache_ms = cache_started.elapsed().as_secs_f64() * 1000.0,
                "flamegraph cache rebuild"
            );
        }

        if started.elapsed() >= std::time::Duration::from_millis(5) {
            tracing::warn!(
                target: "flamegraph.workload",
                total_ms = started.elapsed().as_secs_f64() * 1000.0,
                spans = frame.spans.len(),
                "slow flamegraph frame acquisition"
            );
        }

        let (frame_ref, cache_ref) = self
            .cache
            .as_ref()
            .expect("Cache should be populated by get_or_build_cache");
        (Arc::clone(frame_ref), Arc::clone(cache_ref))
    }

    /// Virtualized span pick: returns the span under the given canvas-local
    /// coordinates, or None. Mirrors the hover logic — binary-search the thread
    /// row by world Y, then scan that row's start-sorted spans backward from
    /// the cursor's time window — instead of scanning every span.
    fn pick_span_at(
        &self,
        frame: &TraceFrame,
        cache: &SpanCache,
        local_x: f32,
        local_y: f32,
    ) -> Option<usize> {
        let viewport_width = load_f32(&self.viewport_width);
        let viewport_height = load_f32(&self.viewport_height);
        if local_x < THREAD_LABEL_WIDTH || local_x > viewport_width {
            return None;
        }
        if local_y < 0.0 || local_y > viewport_height {
            return None;
        }

        let vs = &self.view_state;
        let rows = &cache.thread_rows;
        // Rows are ordered by ascending world Y, so the row under the cursor is
        // `rows[last]`, found by binary search on its screen top.
        let last = rows.partition_point(|layout| layout.row.y - GRAPH_HEIGHT + vs.pan_y < local_y);
        let Some(layout) = last.checked_sub(1).and_then(|l| rows.get(l)) else {
            return None;
        };
        let row_bottom = layout.row.y - GRAPH_HEIGHT + vs.pan_y + layout.row.height;
        if local_y > row_bottom {
            return None;
        }

        let mouse_time = {
            let duration_ns = frame.duration_ns().max(1);
            let zoom = if vs.zoom == 0.0 {
                let ew = (viewport_width - THREAD_LABEL_WIDTH).max(1.0);
                ew / duration_ns as f32
            } else {
                vs.zoom
            };
            let world_x = (local_x - THREAD_LABEL_WIDTH) - vs.pan_x;
            frame.min_time_ns + ((world_x / zoom.max(1e-10)).max(0.0) as u64)
        };
        // Lower bound on candidate window: spans that started near the cursor time.
        const HOVER_SCAN_LIMIT: usize = 1024;
        let hi = layout
            .span_indices
            .partition_point(|i| frame.spans[*i as usize].start_ns <= mouse_time);
        let lo = hi.saturating_sub(HOVER_SCAN_LIMIT);
        for k in (lo..hi).rev() {
            let idx = layout.span_indices[k] as usize;
            let span = &frame.spans[idx];
            let y = layout.row.y - GRAPH_HEIGHT + (span.depth as f32 * ROW_HEIGHT) + vs.pan_y;
            if local_y >= y && local_y <= y + ((ROW_HEIGHT - PADDING) * SPAN_HOVER_HEIGHT_SCALE) {
                let x1 = time_to_x(span.start_ns, frame, viewport_width, vs);
                let x2 = time_to_x(span.end_ns(), frame, viewport_width, vs);
                if local_x >= x1 && local_x <= x2 {
                    return Some(idx);
                }
            }
        }
        None
    }
}

impl Render for FlamegraphView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (frame, cache) = self.get_or_build_cache();

        let thread_offsets = Arc::clone(&cache.thread_offsets);
        let view_state = self.view_state.clone();
        let theme = cx.theme().clone();

        let framerate_graph = render_framerate_graph(&frame, &view_state, cx);
        let driver = self.render_driver_canvas(cx);
        let gpu_display = self.render_gpu_display(theme.background);

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(self.render_graph_strip(
                &frame,
                framerate_graph,
                theme.accent,
                theme.background,
                cx,
            ))
            .child(self.render_timeline_area(
                gpu_display,
                driver,
                &frame,
                &thread_offsets,
                &view_state,
                cx,
            ))
    }
}

#[cfg(test)]
mod device_loss_tests {
    use super::{FlamegraphView, TraceData};
    use gpui::{TestAppContext, VisualTestContext};
    use std::sync::Arc;

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    /// On a test window with a real GPU device (skipped where there is
    /// none): after the device is lost the view keeps no surface, and once
    /// the window has a new device it draws on a surface there.
    #[test]
    fn the_view_moves_onto_the_new_device_after_a_loss() {
        let mut cx = TestAppContext::single();
        cx.update(ui::init);
        let (view, cx) = cx.add_window_view(|_, _| FlamegraphView::new(TraceData::new()));
        if !cx.update(|window, _| window.use_test_gpu_device()) {
            return;
        }
        draw(cx);
        let first = view.read_with(cx, |view, _| view.surface.clone());
        let first = first.expect("a surface on the test device");

        assert!(cx.update(|window, _| window.lose_test_gpu_device()));
        draw(cx);
        assert!(
            view.read_with(cx, |view, _| view.surface.is_none()),
            "no surface is kept while the device is lost"
        );

        assert!(cx.update(|window, cx| window.replace_test_gpu_device(cx)));
        draw(cx);
        let second = view.read_with(cx, |view, _| view.surface.clone());
        let second = second.expect("a surface on the new device");
        assert!(!second.is_device_lost());
        assert!(!Arc::ptr_eq(
            &first.device_lost_flag(),
            &second.device_lost_flag()
        ));
    }
}
