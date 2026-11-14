//! Render stats overlay: every named frame stat (`helio::stats`) the renderer
//! counted, by name: Helio's shared totals (draw calls, vertices, instances)
//! and whatever each plugin's passes publish.
//!
//! The list is whatever the frame holds, so it grows as passes contribute new
//! stats. Like the pipeline overlay it is a view with its own timer, mounted as
//! an isolated cached view: it polls the renderer's published stats (never the
//! renderer) at [`REFRESH`] and re-renders in place only when a newer frame's
//! totals arrived; its rows are a virtual list ([`super::overlay_table`]).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::*;
use ui::ActiveTheme;

use super::overlay_table::{footer_note, Column, OverlayTable, Row, TableScroll, CARD_WIDTH};
use crate::ui::viewport::performance::{compact_count, RendererStatsSource};

type GpuEngine = Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>;

/// How often the renderer's published stats are checked.
const REFRESH: Duration = Duration::from_millis(100);

/// The stat, its total in short form, and exactly.
const COLUMNS: &[Column] = &[
    Column::left("Stat", 230.0),
    Column::right("Total", 70.0),
    Column::right("Exact", 80.0),
];

/// Helio's shared totals, listed first; the rest follow by name.
const SHARED: &[&str] = &[
    helio::stats::DRAW_CALLS,
    helio::stats::VERTICES,
    helio::stats::INSTANCES,
];

pub struct RenderStatsOverlay {
    source: RendererStatsSource,
    stats: Option<Arc<helio::StatsSnapshot>>,
    /// The stat list's scroll, kept across updates.
    scroll: TableScroll,
    _tick: Task<()>,
}

impl RenderStatsOverlay {
    pub fn new(gpu_engine: GpuEngine, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            source: RendererStatsSource::new(gpu_engine),
            stats: None,
            scroll: TableScroll::default(),
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

    /// Take the newest frame's stats if they are newer. Returns whether the
    /// overlay changed.
    fn poll(&mut self) -> bool {
        let Some(fresh) = self
            .source
            .latest()
            .and_then(|stats| stats.details)
            .and_then(|details| details.frame_stats.clone())
        else {
            return false;
        };
        if self.stats.as_ref().is_some_and(|s| s.frame >= fresh.frame) {
            return false;
        }
        self.stats = Some(fresh);
        true
    }
}

/// The overlay for the viewport, created on first use and kept in `slot` so its
/// timer exists only while the overlay is shown.
pub fn render_render_stats_overlay<V>(
    gpu_engine: &GpuEngine,
    slot: &std::cell::RefCell<Option<Entity<RenderStatsOverlay>>>,
    cx: &mut Context<V>,
) -> AnyElement
where
    V: 'static,
{
    let overlay = slot
        .borrow_mut()
        .get_or_insert_with(|| {
            let gpu_engine = gpu_engine.clone();
            cx.new(|cx| RenderStatsOverlay::new(gpu_engine, cx))
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

impl Render for RenderStatsOverlay {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let table = OverlayTable::new("render-stats-list", &self.scroll, "Render Stats", COLUMNS);
        let Some(stats) = &self.stats else {
            return table.render(cx);
        };
        let mut entries: Vec<(&str, u64)> = stats.iter().collect();
        entries.sort_by(|(a, _), (b, _)| {
            let rank = |name: &str| SHARED.iter().position(|s| *s == name);
            match (rank(a), rank(b)) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.cmp(b),
            }
        });
        let rows: Vec<Row> = entries
            .iter()
            .map(|&(name, value)| Row {
                swatch: None,
                cells: vec![
                    name.to_owned().into(),
                    compact_count(value as f64).into(),
                    value.to_string().into(),
                ],
            })
            .collect();
        let count = rows.len();
        table
            .badge(
                format!("frame {}", stats.frame),
                cx.theme().muted_foreground,
            )
            .rows(rows, false)
            .empty("No stats counted yet")
            .footer(footer_note(
                format!("{count} stats · any pass may add to one by name"),
                cx,
            ))
            .render(cx)
    }
}
