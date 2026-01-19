use gpui::*;
use ui::v_flex;
use crate::trace_data::{TraceData, TraceFrame};
use std::ops::Range;
use std::collections::BTreeMap;
use std::sync::Arc;
use ui::ActiveTheme;

const ROW_HEIGHT: f32 = 20.0;
const MIN_SPAN_WIDTH: f32 = 2.0;
const PADDING: f32 = 2.0;
const GRAPH_HEIGHT: f32 = 100.0;
const THREAD_LABEL_WIDTH: f32 = 120.0;
const THREAD_ROW_PADDING: f32 = 30.0;
const TIMELINE_HEIGHT: f32 = 30.0;
const STATS_SIDEBAR_WIDTH: f32 = 250.0;
const TITLE_BAR_HEIGHT: f32 = 34.0;

fn get_palette() -> Vec<Hsla> {
    vec![
        // Professional color palette inspired by profiler tools
        hsla(210.0 / 360.0, 0.75, 0.55, 1.0), // Blue
        hsla(30.0 / 360.0, 0.80, 0.55, 1.0),  // Orange
        hsla(140.0 / 360.0, 0.70, 0.50, 1.0), // Green
        hsla(340.0 / 360.0, 0.75, 0.55, 1.0), // Pink
        hsla(270.0 / 360.0, 0.70, 0.55, 1.0), // Purple
        hsla(180.0 / 360.0, 0.65, 0.50, 1.0), // Cyan
        hsla(50.0 / 360.0, 0.75, 0.55, 1.0),  // Yellow
        hsla(10.0 / 360.0, 0.75, 0.55, 1.0),  // Red-Orange
        hsla(160.0 / 360.0, 0.70, 0.50, 1.0), // Teal
        hsla(290.0 / 360.0, 0.70, 0.55, 1.0), // Violet
        hsla(195.0 / 360.0, 0.70, 0.55, 1.0), // Sky Blue
        hsla(80.0 / 360.0, 0.65, 0.50, 1.0),  // Lime
        hsla(320.0 / 360.0, 0.75, 0.55, 1.0), // Magenta
        hsla(40.0 / 360.0, 0.75, 0.55, 1.0),  // Amber
        hsla(250.0 / 360.0, 0.70, 0.55, 1.0), // Indigo
        hsla(120.0 / 360.0, 0.70, 0.50, 1.0), // Emerald
    ]
}

#[derive(Clone)]
struct ViewState {
    zoom: f32,
    pan_x: f32,
    pan_y: f32,
    dragging: bool,
    drag_start_x: f32,
    drag_start_y: f32,
    drag_pan_start_x: f32,
    drag_pan_start_y: f32,
    hovered_span: Option<usize>,
    mouse_x: f32,
    mouse_y: f32,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            dragging: false,
            drag_start_x: 0.0,
            drag_start_y: 0.0,
            drag_pan_start_x: 0.0,
            drag_pan_start_y: 0.0,
            hovered_span: None,
            mouse_x: 0.0,
            mouse_y: 0.0,
        }
    }
}

struct SpanCache {
    thread_offsets: BTreeMap<u64, f32>,
}

impl SpanCache {
    fn build(frame: &TraceFrame) -> Self {
        let thread_offsets = FlamegraphView::calculate_thread_y_offsets(frame);
        Self { thread_offsets }
    }
}

pub struct FlamegraphView {
    trace_data: TraceData,
    view_state: ViewState,
    cache: Option<(Arc<TraceFrame>, SpanCache)>,
    viewport_width: Arc<std::sync::RwLock<f32>>,
    viewport_height: Arc<std::sync::RwLock<f32>>,
}

impl FlamegraphView {
    pub fn new(trace_data: TraceData) -> Self {
        Self {
            trace_data,
            view_state: ViewState::default(),
            cache: None,
            viewport_width: Arc::new(std::sync::RwLock::new(1920.0)),
            viewport_height: Arc::new(std::sync::RwLock::new(1080.0)),
        }
    }

    fn get_or_build_cache(&mut self) -> (&Arc<TraceFrame>, &SpanCache) {
        let frame = self.trace_data.get_frame();

        // Check if cache is valid (same Arc pointer)
        let needs_rebuild = match &self.cache {
            Some((cached_frame, _)) => !Arc::ptr_eq(cached_frame, &frame),
            None => true,
        };

        if needs_rebuild {
            let cache = SpanCache::build(&frame);
            self.cache = Some((Arc::clone(&frame), cache));
        }

        let (frame_ref, cache_ref) = self.cache.as_ref().unwrap();
        (frame_ref, cache_ref)
    }

    fn calculate_thread_y_offsets(frame: &TraceFrame) -> BTreeMap<u64, f32> {
        let mut offsets = BTreeMap::new();
        let mut current_y = GRAPH_HEIGHT + TIMELINE_HEIGHT + THREAD_ROW_PADDING;
        
        // Sort threads: GPU (0) first, Main Thread (1) second, then workers
        let mut thread_ids: Vec<u64> = frame.threads.keys().copied().collect();
        thread_ids.sort_by_key(|id| match id {
            0 => (0, *id), // GPU first
            1 => (1, *id), // Main Thread second
            _ => (2, *id), // Workers after
        });
        
        for thread_id in thread_ids {
            // Calculate max depth for this thread
            let max_depth_for_thread = frame.spans
                .iter()
                .filter(|s| s.thread_id == thread_id)
                .map(|s| s.depth)
                .max()
                .unwrap_or(0);
            
            offsets.insert(thread_id, current_y);
            current_y += (max_depth_for_thread + 1) as f32 * ROW_HEIGHT + THREAD_ROW_PADDING;
        }
        
        offsets
    }

    fn time_to_x(time_ns: u64, frame: &TraceFrame, viewport_width: f32, view_state: &ViewState) -> f32 {
        if frame.duration_ns() == 0 {
            return 0.0;
        }
        let normalized = (time_ns - frame.min_time_ns) as f32 / frame.duration_ns() as f32;
        (normalized * (viewport_width - THREAD_LABEL_WIDTH) * view_state.zoom) + view_state.pan_x + THREAD_LABEL_WIDTH
    }

    fn visible_range(frame: &TraceFrame, viewport_width: f32, view_state: &ViewState) -> Range<u64> {
        if frame.duration_ns() == 0 {
            return 0..0;
        }

        let effective_width = viewport_width - THREAD_LABEL_WIDTH;
        
        // Calculate visible time range based on pan and zoom
        // Don't clamp to 0..1 here - allow wider range to prevent aggressive culling
        let normalized_start = (-(view_state.pan_x + effective_width * 0.5)) / (effective_width * view_state.zoom);
        let normalized_end = ((effective_width * 1.5 - view_state.pan_x) / (effective_width * view_state.zoom));

        let start_ns = (frame.min_time_ns as f64 + (normalized_start as f64 * frame.duration_ns() as f64)) as u64;
        let end_ns = (frame.min_time_ns as f64 + (normalized_end as f64 * frame.duration_ns() as f64)) as u64;

        // Allow rendering beyond visible bounds to prevent culling during pan/zoom
        let padding = frame.duration_ns() / 10; // 10% padding on each side
        start_ns.saturating_sub(padding)..end_ns.saturating_add(padding)
    }
    
    fn render_framerate_graph(&self, frame: &Arc<TraceFrame>, cx: &mut Context<Self>) -> impl IntoElement {
        let frame_times = frame.frame_times_ms.clone();
        let theme = cx.theme();

        div()
            .h(px(GRAPH_HEIGHT))
            .w_full()
            .bg(theme.list)
            .border_b_1()
            .border_color(theme.border)
            .child(
                canvas(
                    move |bounds, _window, _cx| {
                        (bounds, frame_times.clone())
                    },
                    move |bounds, state, window, _cx| {
                        let (bounds, frame_times) = state;
                        
                        if frame_times.is_empty() {
                            return;
                        }

                        let width: f32 = bounds.size.width.into();
                        let height: f32 = bounds.size.height.into();

                        // Draw background grid
                        window.paint_layer(bounds, |window| {
                            // 60 FPS line
                            let fps_60_y = height * (1.0 - 16.67 / 33.33);
                            let line_bounds = Bounds {
                                origin: point(bounds.origin.x, bounds.origin.y + px(fps_60_y)),
                                size: size(px(width), px(1.0)),
                            };
                            window.paint_quad(fill(line_bounds, hsla(120.0 / 360.0, 0.6, 0.5, 0.3)));

                            // 30 FPS line
                            let fps_30_y = height * (1.0 - 33.33 / 33.33);
                            let line_bounds = Bounds {
                                origin: point(bounds.origin.x, bounds.origin.y + px(fps_30_y)),
                                size: size(px(width), px(1.0)),
                            };
                            window.paint_quad(fill(line_bounds, hsla(0.0, 0.6, 0.5, 0.3)));

                            // Draw frame time graph
                            let point_width = width / frame_times.len() as f32;
                            for (i, &frame_time) in frame_times.iter().enumerate() {
                                let x = i as f32 * point_width;
                                let normalized_height = (frame_time / 33.33).min(1.0);
                                let bar_height = height * normalized_height;

                                let color = if frame_time <= 16.67 {
                                    hsla(120.0 / 360.0, 0.7, 0.5, 0.8) // Green for 60fps+
                                } else if frame_time <= 33.33 {
                                    hsla(60.0 / 360.0, 0.7, 0.5, 0.8) // Yellow for 30-60fps
                                } else {
                                    hsla(0.0, 0.7, 0.5, 0.8) // Red for <30fps
                                };

                                let bar_bounds = Bounds {
                                    origin: point(
                                        bounds.origin.x + px(x),
                                        bounds.origin.y + px(height - bar_height)
                                    ),
                                    size: size(px(point_width.max(1.0)), px(bar_height)),
                                };
                                window.paint_quad(fill(bar_bounds, color));
                            }
                        });
                    },
                )
                .size_full()
            )
    }

    fn render_timeline_ruler(&self, frame: &Arc<TraceFrame>, cx: &mut Context<Self>) -> impl IntoElement {
        let frame_for_canvas = Arc::clone(frame);
        let view_state = self.view_state.clone();
        let theme = cx.theme();

        div()
            .h(px(TIMELINE_HEIGHT))
            .w_full()
            .bg(theme.list_head)
            .border_b_1()
            .border_color(theme.border)
            .child(
                canvas(
                    move |bounds, _window, _cx| {
                        let viewport_width: f32 = bounds.size.width.into();
                        (bounds, Arc::clone(&frame_for_canvas), view_state.clone(), viewport_width)
                    },
                    move |bounds, state, window, _cx| {
                        let (bounds, frame, view_state, viewport_width) = state;

                        if frame.duration_ns() == 0 {
                            return;
                        }

                        let effective_width = viewport_width - THREAD_LABEL_WIDTH;

                        window.paint_layer(bounds, |window| {
                            // Calculate visible time range
                            let visible_range = Self::visible_range(&frame, viewport_width, &view_state);
                            let visible_duration = visible_range.end.saturating_sub(visible_range.start);

                            // Determine appropriate time interval for markers
                            let visible_ms = visible_duration as f64 / 1_000_000.0;
                            let marker_interval_ms = if visible_ms < 10.0 {
                                1.0
                            } else if visible_ms < 50.0 {
                                5.0
                            } else if visible_ms < 100.0 {
                                10.0
                            } else if visible_ms < 500.0 {
                                50.0
                            } else {
                                100.0
                            };

                            let marker_interval_ns = (marker_interval_ms * 1_000_000.0) as u64;

                            // Draw time markers
                            let first_marker = (visible_range.start / marker_interval_ns) * marker_interval_ns;
                            let mut current_time = first_marker;

                            while current_time <= visible_range.end {
                                if current_time >= frame.min_time_ns {
                                    let x = Self::time_to_x(current_time, &frame, viewport_width, &view_state);

                                    if x >= THREAD_LABEL_WIDTH && x <= viewport_width {
                                        // Draw tick mark
                                        let tick_bounds = Bounds {
                                            origin: point(bounds.origin.x + px(x), bounds.origin.y),
                                            size: size(px(1.0), px(6.0)),
                                        };
                                        window.paint_quad(fill(tick_bounds, hsla(0.0, 0.0, 0.6, 1.0)));

                                        // Draw time label
                                        let time_ms = (current_time - frame.min_time_ns) as f64 / 1_000_000.0;
                                        let label = format!("{:.1}ms", time_ms);

                                        // TODO: Add text rendering when GPUI text API is available
                                        // For now, just draw the tick marks
                                    }
                                }
                                current_time += marker_interval_ns;
                            }
                        });
                    },
                )
                .size_full()
            )
    }

    fn render_statistics_sidebar(&self, frame: &Arc<TraceFrame>, cx: &mut Context<Self>) -> impl IntoElement {
        let duration_ms = frame.duration_ns() as f64 / 1_000_000.0;
        let num_frames = frame.frame_times_ms.len();
        let avg_frame_time = if !frame.frame_times_ms.is_empty() {
            frame.frame_times_ms.iter().sum::<f32>() / frame.frame_times_ms.len() as f32
        } else {
            0.0
        };

        let min_frame_time = frame.frame_times_ms.iter().copied().min_by(|a, b| a.partial_cmp(b).unwrap()).unwrap_or(0.0);
        let max_frame_time = frame.frame_times_ms.iter().copied().max_by(|a, b| a.partial_cmp(b).unwrap()).unwrap_or(0.0);
        let theme = cx.theme();

        div()
            .absolute()
            .right_0()
            .top_0()
            .w(px(STATS_SIDEBAR_WIDTH))
            .h_full()
            .bg(theme.popover)
            .border_l_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .p_4()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.foreground)
                    .child("Statistics")
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Total Spans:")
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.foreground)
                                    .child(format!("{}", frame.spans.len()))
                            )
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Duration:")
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.foreground)
                                    .child(format!("{:.2} ms", duration_ms))
                            )
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Frames:")
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.foreground)
                                    .child(format!("{}", num_frames))
                            )
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Avg Frame:")
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.foreground)
                                    .child(format!("{:.2} ms", avg_frame_time))
                            )
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Min Frame:")
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.foreground)
                                    .child(format!("{:.2} ms", min_frame_time))
                            )
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Max Frame:")
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.foreground)
                                    .child(format!("{:.2} ms", max_frame_time))
                            )
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Threads:")
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.foreground)
                                    .child(format!("{}", frame.threads.len()))
                            )
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Max Depth:")
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.foreground)
                                    .child(format!("{}", frame.max_depth))
                            )
                    )
            )
            .child(
                div()
                    .mt_4()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.foreground)
                    .child("Threads")
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(
                        frame.threads.values().take(10).map(|thread| {
                            let thread_color = match thread.id {
                                0 => rgb(0xff6b6b), // GPU
                                1 => rgb(0x51cf66), // Main
                                _ => rgb(0x74c0fc), // Others
                            };

                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .w(px(8.0))
                                        .h(px(8.0))
                                        .bg(thread_color)
                                        .rounded(px(2.0))
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.foreground)
                                        .child(thread.name.clone())
                                )
                        })
                    )
            )
    }

    fn render_hover_popup(&self, frame: &Arc<TraceFrame>, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let span_idx = self.view_state.hovered_span?;
        let span = frame.spans.get(span_idx)?;
        let theme = cx.theme();
        
        let duration_ms = span.duration_ns as f64 / 1_000_000.0;
        let start_ms = (span.start_ns - frame.min_time_ns) as f64 / 1_000_000.0;
        let end_ms = (span.end_ns() - frame.min_time_ns) as f64 / 1_000_000.0;
        let thread_name = frame.threads.get(&span.thread_id).map(|t| t.name.clone()).unwrap_or_else(|| "Unknown".to_string());
        
        let popup_width = 280.0;
        let mouse_x = self.view_state.mouse_x;
        let mouse_y = self.view_state.mouse_y;
        let viewport_width = *self.viewport_width.read().unwrap();
        
        // Position popup horizontally near the mouse cursor
        let popup_x = if mouse_x + popup_width + 20.0 > viewport_width - STATS_SIDEBAR_WIDTH {
            (mouse_x - popup_width - 10.0).max(0.0)
        } else {
            mouse_x + 15.0
        };
        
        // Mouse Y is already relative to the canvas div (where the popup is also rendered)
        // So no offset needed - just position slightly below the cursor
        let popup_y = mouse_y + 5.0 - 200.0;
        
        Some(
            div()
                .absolute()
                .left(px(popup_x))
                .top(px(popup_y))
                .w(px(popup_width))
                .bg(theme.popover)
                .border_1()
                .border_color(theme.border)
                .rounded(px(4.0))
                .shadow_lg()
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.foreground)
                        .child(span.name.clone())
                )
                .child(
                    div()
                        .w_full()
                        .h(px(1.0))
                        .bg(theme.border)
                )
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Duration:")
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(format!("{:.3} ms", duration_ms))
                        )
                )
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Start:")
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.foreground)
                                .child(format!("{:.3} ms", start_ms))
                        )
                )
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("End:")
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.foreground)
                                .child(format!("{:.3} ms", end_ms))
                        )
                )
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Thread:")
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.foreground)
                                .child(thread_name)
                        )
                )
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Depth:")
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.foreground)
                                .child(format!("{}", span.depth))
                        )
                )
        )
    }

    fn render_thread_labels(&self, frame: &Arc<TraceFrame>, thread_offsets: &BTreeMap<u64, f32>, cx: &mut Context<Self>) -> impl IntoElement {
        let thread_offsets = thread_offsets.clone();
        let view_state = self.view_state.clone();
        let theme = cx.theme();

        div()
            .absolute()
            .left_0()
            .top_0()
            .w(px(THREAD_LABEL_WIDTH))
            .h_full()
            .bg(theme.popover)
            .border_r_1()
            .border_color(theme.border)
            .overflow_hidden()
            .children(
                thread_offsets.iter().map(|(thread_id, y_offset)| {
                    let thread = frame.threads.get(thread_id).unwrap();
                    let y = y_offset + view_state.pan_y;
                    
                    div()
                        .absolute()
                        .top(px(y))
                        .left_0()
                        .w_full()
                        .h(px(ROW_HEIGHT))
                        .flex()
                        .items_center()
                        .px_2()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if *thread_id == 0 {
                                    rgb(0xff6b6b) // GPU in red
                                } else if *thread_id == 1 {
                                    rgb(0x51cf66) // Main Thread in green
                                } else {
                                    rgb(0x74c0fc) // Workers in blue
                                })
                                .child(thread.name.clone())
                        )
                })
            )
    }
}

impl Render for FlamegraphView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (frame, cache) = self.get_or_build_cache();
        let frame = Arc::clone(frame);
        let thread_offsets = cache.thread_offsets.clone();
        let view_state = self.view_state.clone();
        let palette = get_palette();
        let theme = cx.theme().clone();

        let view_state_for_canvas = view_state.clone();
        let palette_for_canvas = palette.clone();

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                // Framerate graph at top
                self.render_framerate_graph(&frame, cx)
            )
            .child(
                // Timeline ruler
                self.render_timeline_ruler(&frame, cx)
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .relative()
                    .on_children_prepainted({
                        let viewport_width = self.viewport_width.clone();
                        let viewport_height = self.viewport_height.clone();
                        move |bounds: Vec<Bounds<Pixels>>, _window: &mut Window, _cx: &mut App| {
                            // Store the canvas viewport dimensions
                            if let Some(canvas_bounds) = bounds.first() {
                                *viewport_width.write().unwrap() = canvas_bounds.size.width.into();
                                *viewport_height.write().unwrap() = canvas_bounds.size.height.into();
                            }
                        }
                    })
                    .child(
                        // Main flamegraph canvas
                        canvas(
                            {
                                let frame = Arc::clone(&frame);
                                let thread_offsets = thread_offsets.clone();
                                move |bounds, _window, _cx| {
                                    let viewport_width: f32 = bounds.size.width.into();
                                    let viewport_height: f32 = bounds.size.height.into();
                                    (bounds, Arc::clone(&frame), thread_offsets.clone(), view_state_for_canvas.clone(), viewport_width, viewport_height, palette_for_canvas.clone())
                                }
                            },
                            move |bounds, state, window, _cx| {
                                let (bounds_prep, frame, thread_offsets, view_state, viewport_width, viewport_height, palette) = state;

                                if frame.spans.is_empty() {
                                    return;
                                }

                                let visible_time = Self::visible_range(&frame, viewport_width, &view_state);

                                window.paint_layer(bounds, |window| {
                                    // Draw vertical grid lines aligned with timeline
                                    let visible_range_for_grid = Self::visible_range(&frame, viewport_width, &view_state);
                                    let visible_duration = visible_range_for_grid.end.saturating_sub(visible_range_for_grid.start);
                                    let visible_ms = visible_duration as f64 / 1_000_000.0;

                                    let marker_interval_ms = if visible_ms < 10.0 {
                                        1.0
                                    } else if visible_ms < 50.0 {
                                        5.0
                                    } else if visible_ms < 100.0 {
                                        10.0
                                    } else if visible_ms < 500.0 {
                                        50.0
                                    } else {
                                        100.0
                                    };

                                    let marker_interval_ns = (marker_interval_ms * 1_000_000.0) as u64;
                                    let first_marker = (visible_range_for_grid.start / marker_interval_ns) * marker_interval_ns;
                                    let mut current_time = first_marker;

                                    while current_time <= visible_range_for_grid.end {
                                        if current_time >= frame.min_time_ns {
                                            let x = Self::time_to_x(current_time, &frame, viewport_width, &view_state);

                                            if x >= THREAD_LABEL_WIDTH && x <= viewport_width {
                                                let grid_bounds = Bounds {
                                                    origin: point(bounds.origin.x + px(x), bounds.origin.y),
                                                    size: size(px(1.0), px(viewport_height)),
                                                };
                                                window.paint_quad(fill(grid_bounds, hsla(0.0, 0.0, 0.25, 0.15)));
                                            }
                                        }
                                        current_time += marker_interval_ns;
                                    }

                                    // Draw thread separators
                                    for (idx, (thread_id, y_offset)) in thread_offsets.iter().enumerate() {
                                        if idx > 0 {
                                            let separator_y = y_offset - THREAD_ROW_PADDING / 2.0 + view_state.pan_y;
                                            if separator_y >= 0.0 && separator_y < viewport_height {
                                                let separator_bounds = Bounds {
                                                    origin: point(
                                                        bounds.origin.x + px(THREAD_LABEL_WIDTH),
                                                        bounds.origin.y + px(separator_y)
                                                    ),
                                                    size: size(
                                                        px(viewport_width - THREAD_LABEL_WIDTH),
                                                        px(1.0)
                                                    ),
                                                };
                                                window.paint_quad(fill(separator_bounds, hsla(0.0, 0.0, 0.3, 0.3)));
                                            }
                                        }
                                    }

                                    // ========================================================================
                                    // OFFSCREEN CULLING with careful coordinate handling
                                    // Cull spans outside viewport to improve performance
                                    // Uses padding to prevent edge popping during pan/zoom
                                    // ========================================================================

                                    // Group visible spans by (thread_id, depth) for continuous rendering
                                    let mut span_groups: std::collections::HashMap<(u64, u32), Vec<(f32, f32, usize, f32)>> = std::collections::HashMap::new();

                                    // Culling padding to prevent edge artifacts
                                    const CULL_PADDING: f32 = 100.0;

                                    for (idx, span) in frame.spans.iter().enumerate() {
                                        // Calculate Y position first for vertical culling
                                        // Note: y is relative to canvas top (0 = top of canvas)
                                        let thread_y_offset = thread_offsets.get(&span.thread_id).copied().unwrap_or(0.0);
                                        let y = thread_y_offset + (span.depth as f32 * ROW_HEIGHT) + view_state.pan_y;

                                        // Vertical culling: skip if completely outside viewport (with padding)
                                        // y is in canvas-relative coordinates, viewport_height is the canvas height
                                        if y + ROW_HEIGHT < -CULL_PADDING || y > viewport_height + CULL_PADDING {
                                            continue;
                                        }

                                        // Time-based culling: check if span is in visible time range
                                        if span.end_ns() < visible_time.start || span.start_ns > visible_time.end {
                                            continue;
                                        }

                                        // Calculate X positions (canvas-relative coordinates)
                                        let x1 = Self::time_to_x(span.start_ns, &frame, viewport_width, &view_state);
                                        let x2 = Self::time_to_x(span.end_ns(), &frame, viewport_width, &view_state);

                                        // Horizontal culling: skip if completely outside viewport (with padding)
                                        // x1, x2 are canvas-relative, viewport_width is the canvas width
                                        if x2 < -CULL_PADDING || x1 > viewport_width + CULL_PADDING {
                                            continue;
                                        }

                                        // Group visible spans for rendering
                                        let key = (span.thread_id, span.depth);
                                        span_groups.entry(key).or_insert_with(Vec::new).push((x1, x2, idx, y));
                                    }
                                    
                                    // Process each group - merge adjacent spans into continuous blocks
                                    for ((thread_id, depth), mut spans) in span_groups {
                                        // Sort by x1 position
                                        spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                                        
                                        let mut i = 0;
                                        while i < spans.len() {
                                            let (merge_start, mut merge_end, first_idx, y) = spans[i];
                                            let mut j = i + 1;
                                            let mut span_count = 1;
                                            
                                            // Merge adjacent spans only when nearly touching
                                            // Very minimal merging to preserve detail
                                            while j < spans.len() {
                                                let (next_start, next_end, _, _) = spans[j];
                                                let gap = next_start - merge_end;

                                                // Only merge if gap is less than 1 pixel
                                                // No relative threshold - just absolute minimum
                                                let should_merge = gap < 1.0;
                                                
                                                if should_merge {
                                                    merge_end = next_end;
                                                    span_count += 1;
                                                    j += 1;
                                                } else {
                                                    // Gap too large, stop merging
                                                    break;
                                                }
                                            }
                                            
                                            // Calculate width and ensure minimum after padding
                                            let total_width = merge_end - merge_start;
                                            // CRITICAL: Ensure rendered width is at least MIN_SPAN_WIDTH (2px) AFTER padding
                                            let rendered_width = (total_width - PADDING * 2.0).max(MIN_SPAN_WIDTH);

                                            // Choose color based on first span
                                            let first_span = &frame.spans[first_idx];
                                            let base_color = palette[(first_span.color_index as usize) % palette.len()];

                                            // Darken slightly if multiple spans merged
                                            let color = if span_count > 1 {
                                                hsla(
                                                    base_color.h,
                                                    base_color.s * 0.9,
                                                    base_color.l * 0.85,
                                                    1.0
                                                )
                                            } else {
                                                base_color
                                            };

                                            // Render the span/block with subtle border
                                            let span_bounds = Bounds {
                                                origin: point(
                                                    bounds.origin.x + px(merge_start + PADDING),
                                                    bounds.origin.y + px(y + PADDING)
                                                ),
                                                size: size(
                                                    px(rendered_width),
                                                    px(ROW_HEIGHT - PADDING * 2.0)
                                                ),
                                            };

                                            // Fill with main color
                                            window.paint_quad(fill(span_bounds, color));

                                            // Add subtle top border for depth
                                            if rendered_width > 4.0 {
                                                let highlight_color = hsla(
                                                    color.h,
                                                    color.s * 0.7,
                                                    (color.l * 1.15).min(0.95),
                                                    0.4
                                                );
                                                let top_border = Bounds {
                                                    origin: span_bounds.origin,
                                                    size: size(px(rendered_width), px(1.0)),
                                                };
                                                window.paint_quad(fill(top_border, highlight_color));

                                                // Add subtle bottom shadow
                                                let shadow_color = hsla(0.0, 0.0, 0.0, 0.3);
                                                let bottom_border = Bounds {
                                                    origin: point(
                                                        span_bounds.origin.x,
                                                        span_bounds.origin.y + span_bounds.size.height - px(1.0)
                                                    ),
                                                    size: size(px(rendered_width), px(1.0)),
                                                };
                                                window.paint_quad(fill(bottom_border, shadow_color));
                                            }

                                            // Visual indicator if many spans merged
                                            if span_count > 5 && total_width > 20.0 {
                                                let badge_bounds = Bounds {
                                                    origin: point(
                                                        bounds.origin.x + px(merge_start + total_width - 8.0),
                                                        bounds.origin.y + px(y + PADDING)
                                                    ),
                                                    size: size(px(6.0), px(6.0)),
                                                };
                                                window.paint_quad(fill(badge_bounds, hsla(0.0, 0.0, 1.0, 0.3)));
                                            }
                                            
                                            i = j;
                                        }
                                    }
                                });
                            },
                        )
                        .size_full()
                    )
                    .on_mouse_down(MouseButton::Right, cx.listener(|view, event: &MouseDownEvent, _window, cx| {
                        view.view_state.dragging = true;
                        let pos: Point<Pixels> = event.position;
                        view.view_state.drag_start_x = pos.x.into();
                        view.view_state.drag_start_y = pos.y.into();
                        view.view_state.drag_pan_start_x = view.view_state.pan_x;
                        view.view_state.drag_pan_start_y = view.view_state.pan_y;
                        cx.notify();
                    }))
                    .on_mouse_up(MouseButton::Right, cx.listener(|view, _event: &MouseUpEvent, _window, cx| {
                        view.view_state.dragging = false;
                        cx.notify();
                    }))
                    .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _window, cx| {
                        let pos: Point<Pixels> = event.position;
                        let current_x: f32 = pos.x.into();
                        let current_y: f32 = pos.y.into();
                        
                        view.view_state.mouse_x = current_x;
                        view.view_state.mouse_y = current_y;
                        
                        if view.view_state.dragging {
                            let delta_x = current_x - view.view_state.drag_start_x;
                            let delta_y = current_y - view.view_state.drag_start_y;
                            
                            view.view_state.pan_x = view.view_state.drag_pan_start_x + delta_x;
                            view.view_state.pan_y = view.view_state.drag_pan_start_y + delta_y;
                        } else {
                            // Detect hovered span
                            // Account for the offset from titlebar, framerate graph and timeline at the top
                            let canvas_offset_y = TITLE_BAR_HEIGHT + GRAPH_HEIGHT + TIMELINE_HEIGHT;
                            let canvas_y = current_y - canvas_offset_y;
                            
                            // Copy view_state values before borrowing
                            let view_state_copy = view.view_state.clone();
                            let viewport_width = *view.viewport_width.read().unwrap();
                            let (frame, cache) = view.get_or_build_cache();
                            
                            let mut new_hovered_span = None;
                            
                            // Only check if mouse is within the canvas area
                            if canvas_y >= 0.0 && current_x >= THREAD_LABEL_WIDTH {
                                for (idx, span) in frame.spans.iter().enumerate() {
                                    let thread_y_offset = cache.thread_offsets.get(&span.thread_id).copied().unwrap_or(0.0);
                                    let y = thread_y_offset + (span.depth as f32 * ROW_HEIGHT) + view_state_copy.pan_y;
                                    
                                    if canvas_y >= y && canvas_y <= y + ROW_HEIGHT {
                                        let x1 = Self::time_to_x(span.start_ns, &frame, viewport_width, &view_state_copy);
                                        let x2 = Self::time_to_x(span.end_ns(), &frame, viewport_width, &view_state_copy);
                                        
                                        if current_x >= x1 && current_x <= x2 {
                                            new_hovered_span = Some(idx);
                                            break;
                                        }
                                    }
                                }
                            }
                            
                            view.view_state.hovered_span = new_hovered_span;
                        }
                        
                        cx.notify();
                    }))
                    .on_scroll_wheel(cx.listener(|view, event: &ScrollWheelEvent, _window, cx| {
                        let delta = event.delta.pixel_delta(px(1.0));
                        let delta_y: f32 = delta.y.into();
                        let delta_x: f32 = delta.x.into();

                        if event.modifiers.control || event.modifiers.platform {
                            // Zoom around cursor position (horizontal only)
                            let cursor_pos: Point<Pixels> = event.position;
                            let cursor_x: f32 = cursor_pos.x.into();

                            let old_zoom = view.view_state.zoom;
                            let zoom_factor = 1.0 - (delta_y * 0.01);
                            let new_zoom = old_zoom * zoom_factor;

                            // Calculate world position under cursor before zoom (X only)
                            let world_x = (cursor_x - view.view_state.pan_x) / old_zoom;

                            // Update zoom
                            view.view_state.zoom = new_zoom;

                            // Adjust pan_x so the same world position stays under cursor
                            // Keep pan_y unchanged
                            view.view_state.pan_x = cursor_x - (world_x * new_zoom);
                        } else if event.modifiers.shift {
                            // Vertical panning with shift
                            view.view_state.pan_y -= delta_y * 10.0;
                        } else {
                            // Horizontal panning (default)
                            view.view_state.pan_x -= delta_y * 10.0;
                        }

                        cx.notify();
                    }))
                    .child(
                        // Thread labels on the left (rendered after canvas for proper layering)
                        self.render_thread_labels(&frame, &thread_offsets, cx)
                    )
                    .child(
                        // Statistics sidebar on the right (rendered after canvas for proper layering)
                        self.render_statistics_sidebar(&frame, cx)
                    )
                    .children(
                        // Hover popup (rendered last to be on top)
                        self.render_hover_popup(&frame, cx)
                    )
            )
            .child(
                // Status bar at bottom
                div()
                    .h(px(40.0))
                    .w_full()
                    .bg(theme.list_head)
                    .border_t_1()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .px_4()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.foreground)
                            .child(format!("Spans: {}", frame.spans.len()))
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(format!("Threads: {}", frame.threads.len()))
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(format!("Duration: {:.2}ms", frame.duration_ns() as f64 / 1_000_000.0))
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(format!("Zoom: {:.1}x", view_state.zoom))
                    )
            )
    }
}
