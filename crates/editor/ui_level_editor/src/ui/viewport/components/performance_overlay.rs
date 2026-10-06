//! Compact performance overlay for game development monitoring.
//!
//! The overlay is not rebuilt per frame. It is a handful of views that each own
//! their refresh rate:
//!
//! | Section                                 | Refresh |
//! |-----------------------------------------|---------|
//! | Headline (UI FPS, Render FPS, Frame Time) | 10 Hz |
//! | Rendering (draw calls, vertices, memory)  | 10 Hz |
//! | Input (latency)                           | 10 Hz |
//! | Charts (FPS, frame time, latency)         | 4 Hz  |
//!
//! Each is mounted as an isolated cached view (`AnyView::cached_auto_height` +
//! `isolated`). A section's own timer is the only thing that invalidates it, and
//! it refreshes in place: its ancestors do not render, lay out or prepaint, so
//! the rest of the window is untouched. A text section only invalidates when
//! the text it would show has changed. The overlay container itself never
//! changes after it is built.

use std::sync::Arc;
use std::time::Duration;

use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    chart::AreaChart,
    h_flex, v_flex, ActiveTheme, Icon, IconName, Theme,
};

use super::super::performance::*;
use crate::state::LevelEditorState;

// ── Shared pieces ───────────────────────────────────────────────────────────

/// Run `tick` on `view` every `period` until the view is dropped. The returned
/// task must be kept: dropping it stops the timer.
fn every<V: 'static>(
    period: Duration,
    cx: &mut Context<V>,
    mut tick: impl FnMut(&mut V, &mut Context<V>) + 'static,
) -> Task<()> {
    cx.spawn(async move |this, cx| loop {
        cx.background_executor().timer(period).await;
        if this.update(cx, |this, cx| tick(this, cx)).is_err() {
            break;
        }
    })
}

/// A section card.
fn card(theme: &Theme) -> Div {
    v_flex()
        .w_full()
        .gap_1()
        .p_1p5()
        .rounded(theme.radius)
        .bg(theme.sidebar.opacity(0.3))
        .border_1()
        .border_color(theme.border.opacity(0.3))
}

fn card_title(title: &'static str, theme: &Theme) -> impl IntoElement {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.foreground)
        .child(title)
}

/// Compact stat display: label and value in a single line.
fn stat_line(
    label: &'static str,
    value: impl Into<SharedString>,
    color: Hsla,
    theme: &Theme,
) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .child(div().text_xs().text_color(theme.foreground).child(label))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(value.into()),
        )
}

/// Mini area chart. The x axis is labelled every `step` samples (see
/// [`label_step`]), so no more than ten labels ever show.
fn mini_graph(data: Vec<ChartPoint>, step: usize, color: Hsla) -> impl IntoElement {
    div().h(px(40.0)).w_full().child(
        AreaChart::new(data)
            .x(|d: &ChartPoint| d.position.clone())
            .y(|d: &ChartPoint| d.value)
            .stroke(color)
            .fill(color.opacity(0.15))
            .linear()
            .tick_margin(step),
    )
}

fn ui_fps_color(fps: f64, theme: &Theme) -> Hsla {
    if fps >= 240.0 {
        theme.success
    } else if fps >= 120.0 {
        theme.foreground
    } else if fps >= 60.0 {
        theme.warning
    } else {
        theme.danger
    }
}

fn render_fps_color(fps: f64, theme: &Theme) -> Hsla {
    if fps >= 144.0 {
        theme.success
    } else if fps >= 60.0 {
        theme.foreground
    } else if fps >= 30.0 {
        theme.warning
    } else {
        theme.danger
    }
}

fn frame_time_color(ms: f64, theme: &Theme) -> Hsla {
    if ms <= 6.9 {
        theme.success
    } else if ms <= 16.6 {
        theme.foreground
    } else if ms <= 33.3 {
        theme.warning
    } else {
        theme.danger
    }
}

// ── Headline: UI FPS, Render FPS, Frame Time (10 Hz) ────────────────────────

struct HeadlineStats {
    stats: SharedStats,
    cursor: Cursor,
    ui_fps: f64,
    render_fps: f64,
    frame_ms: f64,
    /// What is on screen; a refresh that would show the same text does nothing.
    shown: [String; 3],
    _tick: Task<()>,
}

impl HeadlineStats {
    fn new(stats: SharedStats, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            stats,
            cursor: Cursor::default(),
            ui_fps: 0.0,
            render_fps: 0.0,
            frame_ms: 0.0,
            shown: Default::default(),
            _tick: Task::ready(()),
        };
        this.sample();
        this._tick = every(TEXT_REFRESH, cx, |this, cx| {
            if this.sample() {
                cx.notify();
            }
        });
        this
    }

    /// Read the latest means; returns whether the text changed.
    fn sample(&mut self) -> bool {
        {
            let stats = self.stats.lock();
            self.ui_fps = stats.mean_since(&mut self.cursor, Metric::UiFps);
            self.render_fps = stats.mean_since(&mut self.cursor, Metric::RenderFps);
            self.frame_ms = stats.mean_since(&mut self.cursor, Metric::FrameTimeMs);
        }
        let text = [
            format!("{:.0}", self.ui_fps),
            format!("{:.0}", self.render_fps),
            format!("{:.2}ms", self.frame_ms),
        ];
        let changed = text != self.shown;
        self.shown = text;
        changed
    }
}

impl Render for HeadlineStats {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        card(theme)
            .child(stat_line(
                "UI FPS",
                self.shown[0].clone(),
                ui_fps_color(self.ui_fps, theme),
                theme,
            ))
            .child(stat_line(
                "Render FPS",
                self.shown[1].clone(),
                render_fps_color(self.render_fps, theme),
                theme,
            ))
            .child(stat_line(
                "Frame Time",
                self.shown[2].clone(),
                frame_time_color(self.frame_ms, theme),
                theme,
            ))
    }
}

// ── Rendering: draw calls, vertices, GPU memory (10 Hz) ─────────────────────

struct RenderingStats {
    stats: SharedStats,
    cursor: Cursor,
    shown: [String; 3],
    _tick: Task<()>,
}

impl RenderingStats {
    fn new(stats: SharedStats, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            stats,
            cursor: Cursor::default(),
            shown: Default::default(),
            _tick: Task::ready(()),
        };
        this.sample();
        this._tick = every(TEXT_REFRESH, cx, |this, cx| {
            if this.sample() {
                cx.notify();
            }
        });
        this
    }

    fn sample(&mut self) -> bool {
        let text = {
            let stats = self.stats.lock();
            [
                format!(
                    "{:.0}",
                    stats.mean_since(&mut self.cursor, Metric::DrawCalls)
                ),
                format!(
                    "{:.0}k",
                    stats.mean_since(&mut self.cursor, Metric::Vertices) / 1000.0
                ),
                format!(
                    "{:.1}MB",
                    stats.mean_since(&mut self.cursor, Metric::MemoryMb)
                ),
            ]
        };
        let changed = text != self.shown;
        self.shown = text;
        changed
    }
}

impl Render for RenderingStats {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        card(theme)
            .child(card_title("Rendering", theme))
            .child(stat_line(
                "Draw Calls",
                self.shown[0].clone(),
                theme.foreground,
                theme,
            ))
            .child(stat_line(
                "Vertices",
                self.shown[1].clone(),
                theme.foreground,
                theme,
            ))
            .child(stat_line(
                "GPU Memory",
                self.shown[2].clone(),
                theme.foreground,
                theme,
            ))
    }
}

// ── Input: latency (10 Hz) ──────────────────────────────────────────────────

struct InputStats {
    stats: SharedStats,
    cursor: Cursor,
    shown: String,
    _tick: Task<()>,
}

impl InputStats {
    fn new(stats: SharedStats, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            stats,
            cursor: Cursor::default(),
            shown: String::new(),
            _tick: Task::ready(()),
        };
        this.sample();
        this._tick = every(TEXT_REFRESH, cx, |this, cx| {
            if this.sample() {
                cx.notify();
            }
        });
        this
    }

    fn sample(&mut self) -> bool {
        let latency = self
            .stats
            .lock()
            .mean_since(&mut self.cursor, Metric::InputLatencyMs);
        let text = format!("{latency:.2}ms");
        let changed = text != self.shown;
        self.shown = text;
        changed
    }
}

impl Render for InputStats {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        card(theme)
            .child(card_title("Input", theme))
            .child(stat_line(
                "Latency",
                self.shown.clone(),
                theme.warning,
                theme,
            ))
    }
}

// ── Charts (4 Hz) ───────────────────────────────────────────────────────────

struct ChartSections {
    stats: SharedStats,
    cursor: Cursor,
    fps: History,
    frame_time: History,
    latency: History,
    _tick: Task<()>,
}

impl ChartSections {
    fn new(stats: SharedStats, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            stats,
            cursor: Cursor::default(),
            fps: History::default(),
            frame_time: History::default(),
            latency: History::default(),
            _tick: Task::ready(()),
        };
        this.sample();
        this._tick = every(CHART_REFRESH, cx, |this, cx| {
            this.sample();
            cx.notify();
        });
        this
    }

    /// Add one sample to each chart: the mean of the frames since the last one.
    fn sample(&mut self) {
        let stats = self.stats.lock();
        self.fps
            .push(stats.mean_since(&mut self.cursor, Metric::UiFps));
        self.frame_time
            .push(stats.mean_since(&mut self.cursor, Metric::FrameTimeMs));
        self.latency
            .push(stats.mean_since(&mut self.cursor, Metric::InputLatencyMs));
    }

    fn chart(
        &self,
        title: &'static str,
        history: &History,
        clamp: Option<f64>,
        color: Hsla,
        theme: &Theme,
    ) -> impl IntoElement {
        card(theme)
            .child(card_title(title, theme))
            .child(mini_graph(
                history.points(clamp),
                label_step(history.len()),
                color,
            ))
    }
}

impl Render for ChartSections {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let fps_color = ui_fps_color(self.fps.last().unwrap_or(0.0), theme);
        let frame_color = frame_time_color(self.frame_time.last().unwrap_or(0.0), theme);
        v_flex()
            .w_full()
            .gap_2()
            .child(self.chart("FPS History", &self.fps, None, fps_color, theme))
            // Spikes are capped so one hitch does not flatten the rest.
            .child(self.chart(
                "Frame Time (ms)",
                &self.frame_time,
                Some(50.0),
                frame_color,
                theme,
            ))
            .child(self.chart(
                "Input Latency (ms)",
                &self.latency,
                None,
                theme.warning,
                theme,
            ))
    }
}

// ── The overlay ─────────────────────────────────────────────────────────────

/// The overlay's static frame. It holds the sections and the close button and is
/// never invalidated by them.
pub struct PerformanceOverlay {
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    headline: Entity<HeadlineStats>,
    rendering: Entity<RenderingStats>,
    input: Entity<InputStats>,
    charts: Entity<ChartSections>,
    /// Feeds the statistics on this overlay's own timer. The viewport's render
    /// cannot: it no longer runs every frame, because nothing here dirties it.
    _sampler: Task<()>,
}

impl PerformanceOverlay {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        stats: SharedStats,
        sampler: Sampler,
        cx: &mut Context<Self>,
    ) -> Self {
        sampler(&mut stats.lock());
        let feed = stats.clone();
        let _sampler = every(SAMPLE_REFRESH, cx, move |_, _| sampler(&mut feed.lock()));
        Self {
            state,
            _sampler,
            headline: cx.new(|cx| HeadlineStats::new(stats.clone(), cx)),
            rendering: cx.new(|cx| RenderingStats::new(stats.clone(), cx)),
            input: cx.new(|cx| InputStats::new(stats.clone(), cx)),
            charts: cx.new(|cx| ChartSections::new(stats, cx)),
        }
    }

    /// A section as its own cached, *isolated* view: when its timer fires it
    /// re-renders in place and nothing above it (the overlay, the viewport panel,
    /// the dock) renders, lays out or prepaints. See `AnyView::isolated`.
    fn section<V: Render>(entity: &Entity<V>) -> AnyView {
        AnyView::from(entity.clone())
            .cached_auto_height(StyleRefinement::default().w_full().flex_shrink_0())
            .isolated()
    }
}

impl Render for PerformanceOverlay {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let state = self.state.clone();
        v_flex()
            .gap_2()
            .p_2()
            .w_full()
            .bg(theme.background.opacity(0.85))
            .rounded_lg()
            .border_1()
            .border_color(theme.border.opacity(0.5))
            .shadow_lg()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(Icon::new(IconName::Activity).size_3())
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.foreground)
                                    .child("Performance"),
                            ),
                    )
                    .child(
                        Button::new("collapse_performance")
                            .icon(IconName::Close)
                            .ghost()
                            .tooltip("Close")
                            .on_click(move |_, _, _| {
                                state
                                    .write()
                                    .overlays
                                    .set_performance_overlay_collapsed(true);
                            }),
                    ),
            )
            .child(Self::section(&self.headline))
            .child(Self::section(&self.rendering))
            .child(Self::section(&self.input))
            .child(Self::section(&self.charts))
    }
}

/// The overlay for the viewport: the expand button when collapsed, otherwise the
/// cached overlay view, created on first use.
///
/// The overlay entity is kept in the caller's `slot`, so it (and its refresh
/// timers) exists only while the overlay is actually shown.
pub fn render_performance_overlay<V>(
    state: &LevelEditorState,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    stats: &SharedStats,
    sampler: Sampler,
    slot: &std::cell::RefCell<Option<Entity<PerformanceOverlay>>>,
    cx: &mut Context<V>,
) -> AnyElement
where
    V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
{
    if state.overlays.state.performance_overlay_collapsed {
        // No timers while collapsed.
        slot.borrow_mut().take();
        return Button::new("expand_performance")
            .icon(IconName::Activity)
            .ghost()
            .tooltip("Show Performance Stats")
            .on_click(move |_, _, _| {
                state_arc
                    .write()
                    .overlays
                    .set_performance_overlay_collapsed(false);
            })
            .into_any_element();
    }

    let overlay = slot
        .borrow_mut()
        .get_or_insert_with(|| {
            let (state_arc, stats) = (state_arc.clone(), stats.clone());
            cx.new(|cx| PerformanceOverlay::new(state_arc, stats, sampler, cx))
        })
        .clone();

    div()
        .w(px(280.0))
        .child(
            AnyView::from(overlay)
                .cached_auto_height(StyleRefinement::default().w_full().flex_shrink_0()),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{render_performance_overlay, PerformanceOverlay};
    use crate::state::LevelEditorState;
    use crate::ui::viewport::performance::{LiveStats, Metric, Sampler, SharedStats};
    use gpui::{
        div, px, size, AppContext as _, Context, Entity, EventEmitter, IntoElement,
        ParentElement as _, Render, Styled as _, TestAppContext, Window,
    };
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Duration;

    struct FakeViewport {
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        stats: SharedStats,
        sampler: Sampler,
        slot: RefCell<Option<Entity<PerformanceOverlay>>>,
    }

    impl EventEmitter<ui::dock::PanelEvent> for FakeViewport {}

    impl Render for FakeViewport {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let state_arc = self.state.clone();
            let overlay = {
                let state = state_arc.read();
                render_performance_overlay(
                    &state,
                    state_arc.clone(),
                    &self.stats,
                    self.sampler.clone(),
                    &self.slot,
                    cx,
                )
            };
            div().size_full().relative().child(
                div()
                    .absolute()
                    .bottom_2()
                    .left_2()
                    .max_w(px(400.))
                    .child(overlay),
            )
        }
    }

    /// The real overlay, in a headless window, ticking: how is it layered, and
    /// does that change when its sections refresh in place?
    #[gpui::test]
    fn the_overlays_layers_survive_in_place_refreshes(cx: &mut TestAppContext) {
        cx.update(|cx| ui::init(cx));
        let n = Rc::new(Cell::new(0u32));
        let counter = n.clone();
        let sampler: Sampler = Rc::new(move |stats: &mut LiveStats| {
            let v = counter.get();
            counter.set(v + 1);
            stats.record(Metric::UiFps, 100.0 + (v % 90) as f64);
            stats.record(Metric::RenderFps, 60.0 + (v % 40) as f64);
            stats.record(Metric::FrameTimeMs, 1.0 + (v % 100) as f64 / 10.0);
            stats.record(Metric::DrawCalls, 1000.0 + (v % 500) as f64);
            stats.record(Metric::Vertices, 200_000.0 + (v % 900) as f64 * 100.0);
            stats.record(Metric::MemoryMb, 512.0 + (v % 100) as f64 / 3.0);
            stats.record(Metric::InputLatencyMs, 0.01);
        });
        let state = Arc::new(parking_lot::RwLock::new(LevelEditorState::new()));
        state.write().overlays.state.show_performance_overlay = true;
        let window = cx.open_window(size(px(700.), px(700.)), move |_, _| FakeViewport {
            state,
            stats: Default::default(),
            sampler,
            slot: RefCell::new(None),
        });
        cx.run_until_parked();
        let report = |cx: &mut TestAppContext, label: &str| {
            let text = window
                .update(cx, |_, window, _| window.debug_layer_report())
                .unwrap();
            eprintln!("== {label}\n{text}");
        };
        let quiet = |cx: &mut TestAppContext| {
            window.update(cx, |_, w, _| w.refresh_buffers()).unwrap();
            cx.run_until_parked();
        };
        for _ in 0..6 {
            quiet(cx);
        }
        report(cx, "settled");
        for tick in 1..=6 {
            cx.executor().advance_clock(Duration::from_millis(110));
            cx.run_until_parked();
            quiet(cx);
            report(cx, &format!("after tick {tick}"));
        }
    }
}
