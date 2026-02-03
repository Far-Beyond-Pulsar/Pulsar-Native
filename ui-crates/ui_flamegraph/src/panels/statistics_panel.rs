use gpui::*;
use ui::{
    v_flex, h_flex,
    ActiveTheme,
    dock::{Panel, PanelEvent},
    StyledExt,
};
use gpui::prelude::FluentBuilder;
use crate::trace_data::TraceData;
use std::sync::Arc;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct FunctionStats {
    pub name: String,
    pub call_count: usize,
    pub total_duration_ns: u64,
    pub avg_duration_ns: u64,
    pub min_duration_ns: u64,
    pub max_duration_ns: u64,
}

pub struct StatisticsPanel {
    trace_data: Arc<TraceData>,
    stats: Vec<FunctionStats>,
    sort_by: SortColumn,
    sort_ascending: bool,
    focus_handle: FocusHandle,
    last_span_count: usize,
    stats_dirty: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum SortColumn {
    Name,
    Calls,
    TotalTime,
    AvgTime,
}

impl StatisticsPanel {
    pub fn new(trace_data: Arc<TraceData>, cx: &mut Context<Self>) -> Self {
        Self {
            trace_data,
            stats: Vec::new(),
            sort_by: SortColumn::TotalTime,
            sort_ascending: false,
            focus_handle: cx.focus_handle(),
            last_span_count: 0,
            stats_dirty: true,
        }
    }

    fn compute_statistics(&mut self) {
        let frame = self.trace_data.get_frame();
        
        // Only recompute if span count changed
        if frame.spans.len() == self.last_span_count && !self.stats_dirty {
            return;
        }
        
        self.last_span_count = frame.spans.len();
        self.stats_dirty = false;
        let mut function_map: HashMap<String, (usize, u64, u64, u64)> = HashMap::new();

        // Aggregate statistics by function name
        for span in &frame.spans {
            let entry = function_map.entry(span.name.clone())
                .or_insert((0, 0, u64::MAX, 0));
            
            entry.0 += 1; // call count
            entry.1 += span.duration_ns; // total duration
            entry.2 = entry.2.min(span.duration_ns); // min duration
            entry.3 = entry.3.max(span.duration_ns); // max duration
        }

        // Convert to FunctionStats vec
        self.stats = function_map.into_iter().map(|(name, (count, total, min, max))| {
            FunctionStats {
                name,
                call_count: count,
                total_duration_ns: total,
                avg_duration_ns: total / count as u64,
                min_duration_ns: min,
                max_duration_ns: max,
            }
        }).collect();

        // Sort by current column
        self.sort_statistics();
    }

    fn sort_statistics(&mut self) {
        self.stats.sort_by(|a, b| {
            let cmp = match self.sort_by {
                SortColumn::Name => a.name.cmp(&b.name),
                SortColumn::Calls => a.call_count.cmp(&b.call_count),
                SortColumn::TotalTime => a.total_duration_ns.cmp(&b.total_duration_ns),
                SortColumn::AvgTime => a.avg_duration_ns.cmp(&b.avg_duration_ns),
            };

            if self.sort_ascending {
                cmp
            } else {
                cmp.reverse()
            }
        });
    }

    fn set_sort(&mut self, column: SortColumn, cx: &mut Context<Self>) {
        if self.sort_by == column {
            self.sort_ascending = !self.sort_ascending;
        } else {
            self.sort_by = column;
            self.sort_ascending = false;
        }
        self.stats_dirty = true; // Mark for re-sort
        self.sort_statistics();
        cx.notify();
    }

    fn format_duration(ns: u64) -> String {
        if ns < 1_000 {
            format!("{}ns", ns)
        } else if ns < 1_000_000 {
            format!("{:.2}µs", ns as f64 / 1_000.0)
        } else if ns < 1_000_000_000 {
            format!("{:.2}ms", ns as f64 / 1_000_000.0)
        } else {
            format!("{:.2}s", ns as f64 / 1_000_000_000.0)
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        
        h_flex()
            .w_full()
            .h(px(32.0))
            .px_3()
            .items_center()
            .gap_2()
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.border)
            .child(
                self.render_header_cell("Function", SortColumn::Name, true, cx)
            )
            .child(
                self.render_header_cell("Calls", SortColumn::Calls, false, cx)
            )
            .child(
                self.render_header_cell("Total", SortColumn::TotalTime, false, cx)
            )
            .child(
                self.render_header_cell("Avg", SortColumn::AvgTime, false, cx)
            )
    }

    fn render_header_cell(&self, label: &str, column: SortColumn, flex_grow: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let is_active = self.sort_by == column;
        let label_owned = label.to_string();
        
        let mut el = div();
        if flex_grow {
            el = el.flex_grow().flex_basis(relative(0.0));
        } else {
            el = el.w(px(80.0));
        }
        
        el.text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(if is_active { theme.accent } else { theme.muted_foreground })
            .cursor_pointer()
            .hover(|style| style.text_color(theme.accent))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _event, _window, cx| {
                this.set_sort(column, cx);
            }))
            .child(label_owned)
    }

    fn render_row(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let stats = &self.stats[index];
        
        h_flex()
            .w_full()
            .h(px(28.0))
            .px_3()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(theme.border.opacity(0.3))
            .hover(|style| style.bg(theme.muted.opacity(0.2)))
            .child(
                div()
                    .flex_grow()
                    .flex_basis(relative(0.0))
                    .text_sm()
                    .text_color(theme.foreground)
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(stats.name.clone())
            )
            .child(
                div()
                    .w(px(80.0))
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .font_family("monospace")
                    .child(format!("{}", stats.call_count))
            )
            .child(
                div()
                    .w(px(80.0))
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .font_family("monospace")
                    .child(Self::format_duration(stats.total_duration_ns))
            )
            .child(
                div()
                    .w(px(80.0))
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .font_family("monospace")
                    .child(Self::format_duration(stats.avg_duration_ns))
            )
    }
}

impl EventEmitter<PanelEvent> for StatisticsPanel {}

impl Focusable for StatisticsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for StatisticsPanel {
    fn panel_name(&self) -> &'static str {
        "flamegraph_statistics"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        div()
            .child("Statistics")
            .into_any_element()
    }
}

impl Render for StatisticsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Only recompute if needed
        self.compute_statistics();

        // Limit to top 100 items for performance
        let item_count = self.stats.len().min(100);
        let total_count = self.stats.len();
        
        let theme = cx.theme();
        
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(self.render_header(cx))
            .child(
                div()
                    .id("stats-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(
                        (0..item_count).map(|index| {
                            self.render_row(index, cx)
                        })
                    )
            )
            .when(total_count > 100, |this| {
                let theme = cx.theme();
                this.child(
                    div()
                        .w_full()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("Showing top 100 of {} functions", total_count))
                )
            })
    }
}
