//! Per-function statistics: a sortable, filterable table that windows its rows
//! so any number of functions stays cheap.

use crate::analysis::{FunctionStats, aggregate, compute_self_times};
use crate::format::format_duration;
use crate::trace_data::{TraceData, TraceFrame};
use gpui::*;
use std::rc::Rc;
use std::sync::Arc;
use ui::{
    dock::{Panel, PanelEvent},
    h_flex,
    input::{InputEvent, InputState, TextInput},
    scroll::{Scrollbar, ScrollbarState},
    v_flex, ActiveTheme,
};

const ROW_HEIGHT: f32 = 24.0;
const NUMBER_WIDTH: f32 = 74.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortColumn {
    Name,
    Calls,
    Total,
    SelfTime,
    SelfPercent,
    Avg,
    Max,
}

pub struct StatisticsPanel {
    trace_data: Arc<TraceData>,
    /// The frame `all` was computed from.
    frame: Option<Arc<TraceFrame>>,
    all: Vec<FunctionStats>,
    /// Sum of all self time: the denominator of the "Self %" column.
    total_self_ns: u64,
    /// Indices into `all`: filtered, then sorted.
    rows: Rc<Vec<usize>>,
    sort_by: SortColumn,
    sort_ascending: bool,
    filter_input: Entity<InputState>,
    filter: String,
    selected: Option<String>,
    scroll: UniformListScrollHandle,
    scrollbar: ScrollbarState,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl StatisticsPanel {
    pub fn new(trace_data: Arc<TraceData>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter functions"));
        let subscription = cx.subscribe(
            &filter_input,
            |this, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.filter = input.read(cx).text().to_string();
                    this.rebuild_rows();
                    cx.notify();
                }
            },
        );
        Self {
            trace_data,
            frame: None,
            all: Vec::new(),
            total_self_ns: 0,
            rows: Rc::new(Vec::new()),
            sort_by: SortColumn::Total,
            sort_ascending: false,
            filter_input,
            filter: String::new(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
            scrollbar: ScrollbarState::default(),
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    /// Recompute the table when a different session has been loaded.
    fn refresh(&mut self) {
        let frame = self.trace_data.get_frame();
        if self
            .frame
            .as_ref()
            .is_some_and(|known| Arc::ptr_eq(known, &frame))
        {
            return;
        }
        let self_ns = compute_self_times(&frame);
        self.total_self_ns = self_ns.iter().sum();
        self.all = aggregate(&frame, &self_ns);
        self.frame = Some(frame);
        self.selected = None;
        self.rebuild_rows();
    }

    fn rebuild_rows(&mut self) {
        let words: Vec<String> = self
            .filter
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let mut rows: Vec<usize> = (0..self.all.len())
            .filter(|&index| {
                let name = self.all[index].name.to_lowercase();
                words.iter().all(|word| name.contains(word))
            })
            .collect();

        let all = &self.all;
        let key = |index: &usize| -> u64 {
            let stat = &all[*index];
            match self.sort_by {
                SortColumn::Name => 0,
                SortColumn::Calls => stat.calls as u64,
                SortColumn::Total => stat.total_ns,
                SortColumn::SelfTime | SortColumn::SelfPercent => stat.self_ns,
                SortColumn::Avg => stat.avg_ns(),
                SortColumn::Max => stat.max_ns,
            }
        };
        if self.sort_by == SortColumn::Name {
            rows.sort_by_key(|&index| all[index].name.to_lowercase());
        } else {
            rows.sort_by(|a, b| key(b).cmp(&key(a)).then_with(|| all[*a].name.cmp(&all[*b].name)));
        }
        if self.sort_ascending {
            rows.reverse();
        }
        self.rows = Rc::new(rows);
    }

    fn set_sort(&mut self, column: SortColumn, cx: &mut Context<Self>) {
        if self.sort_by == column {
            self.sort_ascending = !self.sort_ascending;
        } else {
            self.sort_by = column;
            // Names read best A to Z; every number starts with the biggest.
            self.sort_ascending = column == SortColumn::Name;
        }
        self.rebuild_rows();
        cx.notify();
    }

    fn header_cell(
        &self,
        label: &'static str,
        column: SortColumn,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let active = self.sort_by == column;
        let (accent, muted, foreground) = (theme.accent, theme.muted_foreground, theme.foreground);
        let arrow = match (active, self.sort_ascending) {
            (false, _) => "",
            (true, true) => " ▲",
            (true, false) => " ▼",
        };
        let cell = if column == SortColumn::Name {
            div().flex_1().min_w_0()
        } else {
            div().w(px(NUMBER_WIDTH)).flex_shrink_0().text_right()
        };
        cell.id(label)
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(if active { accent } else { muted })
            .cursor_pointer()
            .hover(move |style| style.text_color(foreground))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| this.set_sort(column, cx)),
            )
            .child(format!("{}{}", label, arrow))
    }

    fn row(&self, row: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let stat = self.all.get(*self.rows.get(row)?)?;
        let theme = cx.theme();
        let (foreground, muted, border, selected_bg, hover_bg) = (
            theme.foreground,
            theme.muted_foreground,
            theme.border,
            theme.list_active,
            theme.list_hover,
        );
        let is_selected = self.selected.as_deref() == Some(stat.name.as_str());
        let percent = if self.total_self_ns == 0 {
            0.0
        } else {
            stat.self_ns as f64 * 100.0 / self.total_self_ns as f64
        };
        let name = stat.name.clone();
        let number = move |text: String| {
            div()
                .w(px(NUMBER_WIDTH))
                .flex_shrink_0()
                .text_right()
                .text_xs()
                .font_family("monospace")
                .text_color(muted)
                .child(text)
        };

        Some(
            h_flex()
                .id(SharedString::from(format!("stat-{}", stat.name)))
                .w_full()
                .h(px(ROW_HEIGHT))
                .px_3()
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(border.opacity(0.25))
                .cursor_pointer()
                .bg(if is_selected { selected_bg } else { transparent_black() })
                .hover(move |style| style.bg(if is_selected { selected_bg } else { hover_bg }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.selected = Some(name.clone());
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_xs()
                        .text_color(foreground)
                        .child(stat.name.clone()),
                )
                .child(number(stat.calls.to_string()))
                .child(number(format_duration(stat.total_ns)))
                .child(number(format_duration(stat.self_ns)))
                .child(number(format!("{:.1}%", percent)))
                .child(number(format_duration(stat.avg_ns())))
                .child(number(format_duration(stat.max_ns)))
                .into_any_element(),
        )
    }
}

impl EventEmitter<PanelEvent> for StatisticsPanel {}

ui_common::panel_boilerplate!(StatisticsPanel);

impl Panel for StatisticsPanel {
    fn panel_name(&self) -> &'static str {
        "flamegraph_statistics"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        div().child("Statistics").into_any_element()
    }
}

impl Render for StatisticsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh();
        let theme = cx.theme();
        let (background, sidebar, border, muted) = (
            theme.background,
            theme.sidebar,
            theme.border,
            theme.muted_foreground,
        );
        let shown = self.rows.len();
        let total = self.all.len();

        let header = h_flex()
            .w_full()
            .h(px(28.0))
            .flex_shrink_0()
            .px_3()
            .gap_2()
            .items_center()
            .bg(sidebar)
            .border_b_1()
            .border_color(border)
            .child(self.header_cell("Function", SortColumn::Name, cx))
            .child(self.header_cell("Calls", SortColumn::Calls, cx))
            .child(self.header_cell("Total", SortColumn::Total, cx))
            .child(self.header_cell("Self", SortColumn::SelfTime, cx))
            .child(self.header_cell("Self %", SortColumn::SelfPercent, cx))
            .child(self.header_cell("Avg", SortColumn::Avg, cx))
            .child(self.header_cell("Max", SortColumn::Max, cx));

        let body: AnyElement = if shown == 0 {
            div()
                .flex_1()
                .p_3()
                .text_xs()
                .text_color(muted)
                .child(if total == 0 {
                    "No spans in this session."
                } else {
                    "No functions match the filter."
                })
                .into_any_element()
        } else {
            let list = uniform_list(
                "flamegraph-statistics-rows",
                shown,
                cx.processor(|panel, range: std::ops::Range<usize>, _window, cx| {
                    range
                        .filter_map(|row| panel.row(row, cx))
                        .collect::<Vec<AnyElement>>()
                }),
            )
            .track_scroll(&self.scroll)
            .size_full();
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(list)
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(Scrollbar::vertical(&self.scrollbar, &self.scroll)),
                )
                .into_any_element()
        };

        v_flex()
            .size_full()
            .bg(background)
            .child(
                h_flex()
                    .flex_shrink_0()
                    .p_2()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(div().flex_1().child(TextInput::new(&self.filter_input)))
                    .child(div().text_xs().text_color(muted).child(format!(
                        "{} of {} functions",
                        shown, total
                    ))),
            )
            .child(header)
            .child(body)
    }
}
