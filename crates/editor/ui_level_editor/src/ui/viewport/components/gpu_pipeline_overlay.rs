//! Render pipeline timing overlay.
//!
//! Like the performance overlay, it is a view with its own timer, mounted as an
//! isolated cached view: it polls the renderer at [`REFRESH`] and re-renders
//! itself in place only when a new profile arrived. Nothing above it renders,
//! lays out or prepaints. It reads the profile the renderer publishes after a
//! frame (`RendererStatsFeed`), not the renderer itself, whose mutex the render
//! scheduler holds while it draws. With nothing new, the last profile stays on
//! screen, at the same size.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::*;
use ui::ActiveTheme;

use super::overlay_table::{
    footer_note, footer_row, Cell, Column, OverlayTable, Row, TableScroll, CARD_WIDTH,
};

use engine_backend::subsystems::render::helio_renderer::{
    DiagnosticMetric, GpuProfilerAvailability, GpuProfilerData,
};

use crate::ui::viewport::performance::RendererStatsSource;

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
    /// The pass list's scroll, kept across updates.
    scroll: TableScroll,
    source: RendererStatsSource,
    _tick: Task<()>,
}

impl GpuPipelineOverlay {
    pub fn new(gpu_engine: GpuEngine, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            profile: None,
            scroll: TableScroll::default(),
            source: RendererStatsSource::new(gpu_engine),
            _tick: Task::ready(()),
        };
        this.poll();
        this._tick = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(REFRESH).await;
            let updated = this.update(cx, |this, cx| {
                if this.poll() {
                    cx.notify();
                }
            });
            if updated.is_err() {
                break;
            }
        });
        this
    }

    /// Take the profile the renderer last published, if it is new. Returns
    /// whether the overlay changed. Reads the renderer's stats feed, never its
    /// mutex (which the render scheduler holds for every frame it draws).
    fn poll(&mut self) -> bool {
        let Some(fresh) = self
            .source
            .latest()
            .and_then(|stats| stats.details)
            .and_then(|details| details.profiler.clone())
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
        .w(px(CARD_WIDTH))
        .child(
            AnyView::from(overlay)
                .cached_auto_height(StyleRefinement::default().w_full().flex_shrink_0())
                .isolated(),
        )
        .into_any_element()
}

/// Pass, its CPU time, its GPU time.
const COLUMNS: &[Column] = &[
    Column::left("Pass", 210.0),
    Column::right("CPU", 65.0),
    Column::right("GPU", 65.0),
];

impl Render for GpuPipelineOverlay {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (foreground, muted, success, warning, danger) = {
            let theme = cx.theme();
            (
                theme.foreground,
                theme.muted_foreground,
                theme.success,
                theme.warning,
                theme.danger,
            )
        };
        let table = OverlayTable::new("gpu-pass-list", &self.scroll, "Render Pipeline", COLUMNS);
        let Some(data) = &self.profile else {
            return table.render(cx);
        };
        let (label, color) = match data.availability {
            GpuProfilerAvailability::Disabled => ("GPU disabled", muted),
            GpuProfilerAvailability::Unsupported => ("GPU unsupported", warning),
            GpuProfilerAvailability::Pending => ("GPU pending", warning),
            GpuProfilerAvailability::Available => ("GPU available", success),
            GpuProfilerAvailability::Backpressured => ("GPU backpressured", danger),
        };
        // The passes that recorded anything, slowest first.
        let mut passes: Vec<&DiagnosticMetric> = data
            .render_metrics
            .iter()
            .filter(|metric| metric.cpu_ms.is_some() || metric.gpu_ms.is_some())
            .collect();
        passes.sort_by(|a, b| {
            let a_time = a.gpu_ms.or(a.cpu_ms).unwrap_or_default();
            let b_time = b.gpu_ms.or(b.cpu_ms).unwrap_or_default();
            b_time.total_cmp(&a_time)
        });
        let rows: Vec<Row> = passes
            .iter()
            .enumerate()
            .map(|(index, metric)| {
                let (r, g, b) = PASS_COLORS[index % PASS_COLORS.len()];
                Row {
                    swatch: Some(hsla(r, g, b, 1.0)),
                    cells: vec![
                        metric.name.into(),
                        time_label(metric.cpu_ms).into(),
                        time_label(metric.gpu_ms).into(),
                    ],
                }
            })
            .collect();
        let total = |ms: Option<f32>| Cell {
            text: time_label(ms).into(),
            color: Some(timing_color(ms, success, warning, danger)),
        };
        table
            .badge(label, color)
            .rows(rows, true)
            .footer(footer_row(
                COLUMNS,
                true,
                vec![
                    "Pass totals".into(),
                    total(data.total_cpu_ms),
                    total(data.total_gpu_ms),
                ],
                foreground,
            ))
            .footer(footer_note(
                format!(
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
                ),
                cx,
            ))
            .render(cx)
    }
}
