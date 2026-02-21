//! Memory panel — system memory stats (cache, pools, committed) + engine allocation breakdown.

use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{ActiveTheme, StyledExt, dock::{Panel, PanelEvent}, v_flex};
use crate::performance_metrics::SharedPerformanceMetrics;
use crate::memory_tracking::SharedMemoryTracker;

pub struct MemoryBreakdownPanel {
    focus_handle: FocusHandle,
    scroll_handle: ui::VirtualListScrollHandle,
    cached_entries: Vec<crate::AllocationEntry>,
    cached_total: usize,
    last_update: std::time::Instant,
    metrics: SharedPerformanceMetrics,
}

impl MemoryBreakdownPanel {
    pub fn new(_memory_tracker: SharedMemoryTracker, metrics: SharedPerformanceMetrics, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            scroll_handle: ui::VirtualListScrollHandle::new(),
            cached_entries: Vec::new(),
            cached_total: 0,
            last_update: std::time::Instant::now(),
            metrics,
        }
    }
}

impl EventEmitter<PanelEvent> for MemoryBreakdownPanel {}

impl Render for MemoryBreakdownPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{h_flex, v_virtual_list, chart::AreaChart, scroll::ScrollbarAxis};

        let theme = cx.theme().clone();

        // Refresh cached data at ~15 fps
        let now = std::time::Instant::now();
        if now.duration_since(self.last_update).as_millis() >= 67 {
            self.last_update = now;
            use crate::atomic_memory_tracking::ATOMIC_MEMORY_COUNTERS;
            self.cached_total   = ATOMIC_MEMORY_COUNTERS.total();
            self.cached_entries = ATOMIC_MEMORY_COUNTERS.get_all_entries();
        }

        let snap           = self.metrics.read().mem_snapshot.clone();
        let committed_hist: Vec<f64> = self.metrics.read().committed_history.iter().copied().collect();
        let cached_hist:    Vec<f64> = self.metrics.read().cached_history.iter().copied().collect();

        let entry_count  = self.cached_entries.len();
        let row_height   = px(50.0);
        let item_sizes   = std::rc::Rc::new(vec![size(px(0.0), row_height); entry_count]);
        let view         = cx.entity().clone();
        let cached_entries = self.cached_entries.clone();
        let cached_alloc   = self.cached_total;

        let in_use_pct = if snap.total_mb > 0 {
            (snap.in_use_mb as f64 / snap.total_mb as f64 * 100.0).min(100.0)
        } else { 0.0 };
        let use_color = if in_use_pct > 85.0 { theme.danger } else if in_use_pct > 65.0 { theme.warning } else { theme.success };

        let stat_card = |label: &str, value: String, color: gpui::Hsla| -> Div {
            v_flex()
                .p_2().gap_1().bg(theme.background)
                .border_1().border_color(theme.border).rounded(px(6.0))
                .child(div().text_size(px(10.0)).text_color(theme.muted_foreground).child(label.to_string()))
                .child(div().text_size(px(12.0)).font_weight(gpui::FontWeight::BOLD).text_color(color).child(value))
        };

        let mb_str = |v: u64| format!("{} MiB", v);
        let opt_mb = |v: Option<u64>| v.map(|n| format!("{} MiB", n)).unwrap_or_else(|| "N/A".to_string());

        #[derive(Clone)] struct Pt { i: usize, v: f64 }
        let committed_pts: Vec<Pt> = committed_hist.into_iter().enumerate().map(|(i,v)| Pt{i,v}).collect();
        let cached_pts:    Vec<Pt> = cached_hist.into_iter().enumerate().map(|(i,v)| Pt{i,v}).collect();

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            // ── Header + stats ────────────────────────────────────────────────
            .child(
                v_flex()
                    .w_full().p_3().gap_3()
                    .bg(theme.background).border_b_1().border_color(theme.border)
                    // Title + total
                    .child(
                        h_flex().w_full().justify_between().items_center()
                            .child(div().text_size(px(13.0)).font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.foreground).child("System Memory"))
                            .child(div().text_size(px(18.0)).font_weight(gpui::FontWeight::BOLD)
                                .text_color(use_color)
                                .child(format!("{} / {} MiB  ({:.0}%)", snap.in_use_mb, snap.total_mb, in_use_pct)))
                    )
                    // Usage bar
                    .child(
                        div().w_full().h(px(8.0)).bg(theme.border).rounded(px(4.0))
                            .child(div().h_full().w(relative(in_use_pct as f32 / 100.0))
                                .bg(use_color).rounded(px(4.0)))
                    )
                    // Stats grid
                    .child(
                        div().w_full().grid().grid_cols(4).gap_2()
                            .child(stat_card("In Use",         mb_str(snap.in_use_mb),    use_color))
                            .child(stat_card("Available",      mb_str(snap.available_mb), theme.success))
                            .child(stat_card("Cached",         opt_mb(snap.cached_mb),    theme.info))
                            .child(stat_card("Total RAM",      mb_str(snap.total_mb),     theme.muted_foreground))
                            .child(stat_card("Committed", {
                                if let (Some(c), Some(l)) = (snap.committed_mb, snap.committed_limit_mb) {
                                    format!("{} / {} MiB", c, l)
                                } else { opt_mb(snap.committed_mb) }
                            }, theme.warning))
                            .child(stat_card("Paged Pool",     opt_mb(snap.paged_pool_mb),     theme.accent))
                            .child(stat_card("Non-Paged Pool", opt_mb(snap.non_paged_pool_mb), theme.accent))
                            .child(stat_card("Page File", {
                                if snap.swap_total_mb > 0 {
                                    format!("{} / {} MiB", snap.swap_used_mb, snap.swap_total_mb)
                                } else { "N/A".to_string() }
                            }, theme.muted_foreground))
                    )
                    // Committed / Cached charts
                    .child(
                        div().w_full().grid().grid_cols(2).gap_2()
                            .child(
                                v_flex().p_2().gap_1().bg(theme.sidebar)
                                    .border_1().border_color(theme.border).rounded(px(6.0))
                                    .child(div().text_size(px(10.0)).text_color(theme.muted_foreground).child("Committed (MiB)"))
                                    .when(!committed_pts.is_empty(), |this| {
                                        this.child(div().h(px(48.0)).w_full().child(
                                            AreaChart::<_, SharedString, f64>::new(committed_pts)
                                                .x(|p: &Pt| format!("{}", p.i).into()).y(|p: &Pt| p.v)
                                                .stroke(theme.warning).fill(theme.warning.opacity(0.2))
                                                .linear().tick_margin(0)
                                        ))
                                    })
                            )
                            .child(
                                v_flex().p_2().gap_1().bg(theme.sidebar)
                                    .border_1().border_color(theme.border).rounded(px(6.0))
                                    .child(div().text_size(px(10.0)).text_color(theme.muted_foreground).child("Cached (MiB)"))
                                    .when(!cached_pts.is_empty(), |this| {
                                        this.child(div().h(px(48.0)).w_full().child(
                                            AreaChart::<_, SharedString, f64>::new(cached_pts)
                                                .x(|p: &Pt| format!("{}", p.i).into()).y(|p: &Pt| p.v)
                                                .stroke(theme.info).fill(theme.info.opacity(0.2))
                                                .linear().tick_margin(0)
                                        ))
                                    })
                            )
                    )
                    // Physical sticks note
                    .child(
                        div().text_size(px(10.0)).text_color(theme.muted_foreground)
                            .child("Physical slot layout requires WMI and is not available in this version.")
                    )
            )
            // ── Engine allocation list ────────────────────────────────────────
            .child(
                v_flex().w_full().p_2().gap_1()
                    .child(
                        h_flex().w_full().justify_between().px_2()
                            .child(div().text_size(px(11.0)).font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.foreground).child("Engine Allocations"))
                            .child(div().text_size(px(11.0)).text_color(theme.accent)
                                .child(format!("{:.2} MB", cached_alloc as f64 / 1024.0 / 1024.0)))
                    )
            )
            .child(
                v_virtual_list(
                    view,
                    "memory-breakdown-list",
                    item_sizes,
                    move |_this, range, _window, cx| {
                        let theme = cx.theme().clone();
                        let total   = cached_alloc;
                        let entries = &cached_entries;
                        let colors  = vec![
                            theme.chart_1, theme.chart_2, theme.chart_3,
                            theme.chart_4, theme.chart_5,
                            theme.info, theme.warning, theme.success,
                        ];
                        range.map(|ix| {
                            if let Some(entry) = entries.get(ix) {
                                let pct     = if total > 0 { entry.size as f64 / total as f64 * 100.0 } else { 0.0 };
                                let color   = colors[ix % colors.len()];
                                let size_mb = entry.size as f64 / 1024.0 / 1024.0;
                                use ui::h_flex;
                                v_flex().w_full().p_3().gap_1()
                                    .child(
                                        h_flex().w_full().justify_between().items_center()
                                            .child(div().text_size(px(12.0)).font_weight(gpui::FontWeight::MEDIUM)
                                                .text_color(theme.foreground).child(entry.name.clone()))
                                            .child(h_flex().gap_2().items_center()
                                                .child(div().text_size(px(11.0)).text_color(theme.muted_foreground)
                                                    .child(format!("{:.2} MB", size_mb)))
                                                .child(div().text_size(px(11.0)).font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .text_color(color).child(format!("{:.1}%", pct)))
                                            )
                                    )
                                    .child(
                                        div().w_full().h(px(6.0)).bg(theme.border).rounded(px(3.0))
                                            .child(div().h_full().w(relative(pct as f32 / 100.0))
                                                .bg(color).rounded(px(3.0)))
                                    )
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            }
                        }).collect()
                    },
                )
                .track_scroll(&self.scroll_handle)
            )
    }
}

impl Focusable for MemoryBreakdownPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle { self.focus_handle.clone() }
}

impl Panel for MemoryBreakdownPanel {
    fn panel_name(&self) -> &'static str { "memory_breakdown" }
    fn title(&self, _window: &Window, _cx: &App) -> AnyElement { "Memory".into_any_element() }
}
