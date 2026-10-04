//! Render pipeline timing overlay.
//!
//! Like the performance overlay, it is a view with its own timer, mounted as an
//! isolated cached view: it polls the renderer at [`REFRESH`] and re-renders
//! itself in place only when a new profile arrived. Nothing above it renders,
//! lays out or prepaints. When the renderer is busy the poll simply finds
//! nothing new and the last profile stays on screen, at the same size.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::{ActiveTheme, StyledExt, h_flex, v_flex};

use engine_backend::subsystems::render::helio_renderer::{
    DiagnosticMetric, GpuProfilerAvailability, GpuProfilerData,
};

type GpuEngine = Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>;

/// How often the renderer is polled for a new profile.
const REFRESH: Duration = Duration::from_millis(100);

const PASS_COLORS: &[(f32, f32, f32)] = &[
    (0.4, 0.7, 1.0),
    (1.0, 0.6, 0.4),
    (0.6, 1.0, 0.6),
    (1.0, 0.8, 0.4),
    (0.8, 0.6, 1.0),
    (1.0, 0.6, 0.8),
    (0.6, 0.9, 1.0),
    (1.0, 0.9, 0.6),
];

fn time_label(time_ms: Option<f32>) -> String {
    time_ms
        .map(|time| format!("{time:.2}ms"))
        .unwrap_or_else(|| "—".to_owned())
}

fn timing_color(time_ms: Option<f32>, success: Hsla, warning: Hsla, danger: Hsla) -> Hsla {
    match time_ms {
        Some(time) if time < 8.0 => success,
        Some(time) if time < 16.0 => warning,
        Some(_) => danger,
        None => warning,
    }
}

/// Which profile this is, so an unchanged one does not cause a re-render.
fn profile_id(data: &GpuProfilerData) -> (u64, Option<u64>) {
    (data.frame_count, data.gpu_frame_count)
}

pub struct GpuPipelineOverlay {
    profile: Option<GpuProfilerData>,
    _tick: Task<()>,
}

impl GpuPipelineOverlay {
    pub fn new(gpu_engine: GpuEngine, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            profile: None,
            _tick: Task::ready(()),
        };
        this.poll(&gpu_engine);
        this._tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REFRESH).await;
                let updated = this.update(cx, |this, cx| {
                    if this.poll(&gpu_engine) {
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        });
        this
    }

    /// Take the renderer's latest profile if it can be had without waiting and
    /// is new. Returns whether the overlay changed. A busy renderer, or one with
    /// nothing new, leaves the last profile where it is.
    fn poll(&mut self, gpu_engine: &GpuEngine) -> bool {
        let Some(fresh) = gpu_engine
            .try_lock()
            .ok()
            .and_then(|engine| engine.get_gpu_profiler_data())
        else {
            return false;
        };
        if self.profile.as_ref().map(profile_id) == Some(profile_id(&fresh)) {
            return false;
        }
        self.profile = Some(fresh);
        true
    }
}

/// The overlay for the viewport, created on first use and kept in `slot` so its
/// timer exists only while the overlay is shown.
pub fn render_gpu_pipeline_overlay<V>(
    gpu_engine: &GpuEngine,
    slot: &std::cell::RefCell<Option<Entity<GpuPipelineOverlay>>>,
    cx: &mut Context<V>,
) -> AnyElement
where
    V: 'static,
{
    let overlay = slot
        .borrow_mut()
        .get_or_insert_with(|| {
            let gpu_engine = gpu_engine.clone();
            cx.new(|cx| GpuPipelineOverlay::new(gpu_engine, cx))
        })
        .clone();
    div()
        .w(px(410.0))
        .child(
            AnyView::from(overlay)
                .cached_auto_height(StyleRefinement::default().w_full().flex_shrink_0())
                .isolated(),
        )
        .into_any_element()
}

impl Render for GpuPipelineOverlay {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let profiler_data = self.profile.clone();

    let (background, border, foreground, muted, success, warning, danger) = {
        let theme = cx.theme();
        (
            theme.background,
            theme.border,
            theme.foreground,
            theme.muted_foreground,
            theme.success,
            theme.warning,
            theme.danger,
        )
    };

    v_flex()
        .gap_2()
        .p_3()
        .w(px(410.0))
        .bg(background.opacity(0.85))
        .rounded_lg()
        .border_1()
        .border_color(border.opacity(0.5))
        .shadow_lg()
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(foreground)
                        .child("Render Pipeline"),
                )
                .when_some(profiler_data.as_ref(), |header, data| {
                    let (label, color) = match data.availability {
                        GpuProfilerAvailability::Disabled => ("GPU disabled", muted),
                        GpuProfilerAvailability::Unsupported => ("GPU unsupported", warning),
                        GpuProfilerAvailability::Pending => ("GPU pending", warning),
                        GpuProfilerAvailability::Available => ("GPU available", success),
                        GpuProfilerAvailability::Backpressured => ("GPU backpressured", danger),
                    };
                    header.child(div().text_xs().text_color(color).child(label))
                }),
        )
        .child(div().w_full().h(px(1.0)).bg(border))
        .map(|this| {
            if let Some(ref data) = profiler_data {
                let mut render_passes: Vec<&DiagnosticMetric> = data
                    .render_metrics
                    .iter()
                    .filter(|metric| metric.cpu_ms.is_some() || metric.gpu_ms.is_some())
                    .collect();
                render_passes.sort_by(|a, b| {
                    let a_time = a.gpu_ms.or(a.cpu_ms).unwrap_or_default();
                    let b_time = b.gpu_ms.or(b.cpu_ms).unwrap_or_default();
                    b_time.total_cmp(&a_time)
                });

                this.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .child(div().w(px(16.0)).flex_none())
                                .child(
                                    div()
                                        .w(px(210.0))
                                        .flex_none()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child("Pass"),
                                )
                                .child(
                                    div()
                                        .w(px(65.0))
                                        .flex_none()
                                        .text_right()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child("CPU"),
                                )
                                .child(
                                    div()
                                        .w(px(65.0))
                                        .flex_none()
                                        .text_right()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child("GPU"),
                                ),
                        )
                        .child(
                            div()
                                .id("gpu-pass-list")
                                .w_full()
                                .max_h(px(300.0))
                                .scrollable(gpui::Axis::Vertical)
                                .occlude()
                                .child(v_flex().gap_0p5().children(
                                    render_passes.iter().enumerate().map(|(index, metric)| {
                                        let (r, g, b) = PASS_COLORS[index % PASS_COLORS.len()];
                                        h_flex()
                                            .w_full()
                                            .items_center()
                                            .child(
                                                div().w(px(16.0)).flex_none().child(
                                                    div()
                                                        .w(px(8.0))
                                                        .h(px(8.0))
                                                        .rounded(px(2.0))
                                                        .bg(hsla(r, g, b, 1.0)),
                                                ),
                                            )
                                            .child(
                                                div()
                                                    .w(px(210.0))
                                                    .flex_none()
                                                    .overflow_hidden()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .line_height(relative(1.0))
                                                    .whitespace_nowrap()
                                                    .child(metric.name),
                                            )
                                            .child(
                                                div()
                                                    .w(px(65.0))
                                                    .flex_none()
                                                    .text_right()
                                                    .text_xs()
                                                    .text_color(foreground)
                                                    .child(time_label(metric.cpu_ms)),
                                            )
                                            .child(
                                                div()
                                                    .w(px(65.0))
                                                    .flex_none()
                                                    .text_right()
                                                    .text_xs()
                                                    .text_color(foreground)
                                                    .child(time_label(metric.gpu_ms)),
                                            )
                                    }),
                                )),
                        )
                        .child(div().w_full().h(px(1.0)).bg(border).mt_1())
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .child(div().w(px(16.0)).flex_none())
                                .child(
                                    div()
                                        .w(px(210.0))
                                        .flex_none()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(foreground)
                                        .child("Pass totals"),
                                )
                                .child(
                                    div()
                                        .w(px(65.0))
                                        .flex_none()
                                        .text_right()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(timing_color(
                                            data.total_cpu_ms,
                                            success,
                                            warning,
                                            danger,
                                        ))
                                        .child(time_label(data.total_cpu_ms)),
                                )
                                .child(
                                    div()
                                        .w(px(65.0))
                                        .flex_none()
                                        .text_right()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(timing_color(
                                            data.total_gpu_ms,
                                            success,
                                            warning,
                                            danger,
                                        ))
                                        .child(time_label(data.total_gpu_ms)),
                                ),
                        )
                        .child(div().text_xs().text_color(muted).child(format!(
                                "CPU frame {} · GPU frame {} · lag {} · drops {} · overflows {}",
                                data.frame_count,
                                data.gpu_frame_count
                                    .map(|frame| frame.to_string())
                                    .unwrap_or_else(|| "—".to_owned()),
                                data.gpu_lag_frames
                                    .map(|lag| lag.to_string())
                                    .unwrap_or_else(|| "—".to_owned()),
                                data.readback_drops,
                                data.query_overflows
                            ))),
                )
            } else {
                this.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("Waiting for the renderer"),
                )
            }
        })
        .into_any_element()
}
}
