use gpui::{prelude::*, *};
use std::{
    cell::RefCell,
    collections::VecDeque,
    ops::Range,
    rc::Rc,
    time::Duration,
};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    table::{Column, Table, TableDelegate},
    v_flex, ActiveTheme as _, IconName,
};

const MAX_BUFFERED_LINES: usize = 250_000;
const TRIM_CHUNK_LINES: usize = 10_000;
const LIVE_BATCH_MAX_LINES: usize = 2_048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
    Unknown,
}

impl LogLevel {
    fn from_line(line: &str) -> Self {
        let upper = line.to_ascii_uppercase();
        if upper.contains("ERROR") || upper.contains(" ERR ") || upper.starts_with("ERR") {
            LogLevel::Error
        } else if upper.contains("WARN") {
            LogLevel::Warn
        } else if upper.contains("INFO") {
            LogLevel::Info
        } else if upper.contains("DEBUG") {
            LogLevel::Debug
        } else if upper.contains("TRACE") {
            LogLevel::Trace
        } else {
            LogLevel::Unknown
        }
    }

    fn color(&self, theme: &ui::Theme) -> Hsla {
        match self {
            LogLevel::Error => theme.danger,
            LogLevel::Warn => theme.warning,
            LogLevel::Info => theme.info,
            LogLevel::Debug => theme.muted_foreground,
            LogLevel::Trace => theme.muted_foreground.opacity(0.7),
            LogLevel::Unknown => theme.foreground,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            LogLevel::Error => "ERROR",
            LogLevel::Warn => "WARN",
            LogLevel::Info => "INFO",
            LogLevel::Debug => "DEBUG",
            LogLevel::Trace => "TRACE",
            LogLevel::Unknown => "OTHER",
        }
    }
}

#[derive(Clone)]
struct LogRow {
    abs_line: usize,
    level: LogLevel,
    text: String,
}

struct LogStore {
    rows: VecDeque<LogRow>,
    filtered_indices: Vec<usize>,
    total_seen: usize,
    dropped_total: usize,
    level_filter: Option<LogLevel>,
    search_query: String,
}

impl LogStore {
    fn new() -> Self {
        Self {
            rows: VecDeque::new(),
            filtered_indices: Vec::new(),
            total_seen: 0,
            dropped_total: 0,
            level_filter: None,
            search_query: String::new(),
        }
    }

    fn clear(&mut self) {
        self.rows.clear();
        self.filtered_indices.clear();
        self.total_seen = 0;
        self.dropped_total = 0;
    }

    fn has_active_filter(&self) -> bool {
        self.level_filter.is_some() || !self.search_query.is_empty()
    }

    fn visible_count(&self) -> usize {
        if self.has_active_filter() {
            self.filtered_indices.len()
        } else {
            self.rows.len()
        }
    }

    fn matches_filters(&self, row: &LogRow) -> bool {
        if let Some(level) = self.level_filter {
            if row.level != level {
                return false;
            }
        }

        if self.search_query.is_empty() {
            return true;
        }

        row.text
            .to_ascii_lowercase()
            .contains(&self.search_query.to_ascii_lowercase())
    }

    fn refilter_all(&mut self) {
        self.filtered_indices.clear();
        if !self.has_active_filter() {
            return;
        }

        let query = self.search_query.to_ascii_lowercase();
        let has_query = !query.is_empty();
        for (ix, row) in self.rows.iter().enumerate() {
            if let Some(level) = self.level_filter {
                if row.level != level {
                    continue;
                }
            }

            if has_query && !row.text.to_ascii_lowercase().contains(&query) {
                continue;
            }

            self.filtered_indices.push(ix);
        }
    }

    fn append_batch(&mut self, lines: Vec<String>) {
        if lines.is_empty() {
            return;
        }

        let query = self.search_query.to_ascii_lowercase();
        let has_query = !query.is_empty();
        let level_filter = self.level_filter;

        for line in lines {
            self.total_seen += 1;
            let row = LogRow {
                abs_line: self.total_seen,
                level: LogLevel::from_line(&line),
                text: line,
            };

            let row_ix = self.rows.len();
            let matches = if let Some(level) = level_filter {
                if row.level != level {
                    false
                } else if has_query {
                    row.text.to_ascii_lowercase().contains(&query)
                } else {
                    true
                }
            } else if has_query {
                row.text.to_ascii_lowercase().contains(&query)
            } else {
                false
            };

            self.rows.push_back(row);

            if self.has_active_filter() && matches {
                self.filtered_indices.push(row_ix);
            }
        }

        self.trim_if_needed();
    }

    fn trim_if_needed(&mut self) {
        if self.rows.len() <= MAX_BUFFERED_LINES {
            return;
        }

        let drop_count = TRIM_CHUNK_LINES.min(self.rows.len());
        for _ in 0..drop_count {
            let _ = self.rows.pop_front();
        }
        self.dropped_total += drop_count;

        if self.has_active_filter() {
            self.filtered_indices.retain(|ix| *ix >= drop_count);
            for ix in &mut self.filtered_indices {
                *ix -= drop_count;
            }
        }
    }

    fn set_level_filter(&mut self, level: Option<LogLevel>) {
        self.level_filter = level;
        self.refilter_all();
    }

    fn row_for_visible(&self, visible_row: usize) -> Option<&LogRow> {
        if self.has_active_filter() {
            let base_ix = *self.filtered_indices.get(visible_row)?;
            self.rows.get(base_ix)
        } else {
            self.rows.get(visible_row)
        }
    }
}

#[derive(Clone)]
struct LogTableDelegate {
    store: Rc<RefCell<LogStore>>,
    columns: Vec<Column>,
}

impl LogTableDelegate {
    fn new(store: Rc<RefCell<LogStore>>) -> Self {
        Self {
            store,
            columns: vec![
                Column::new("line", "Line").width(px(90.0)).resizable(false),
                Column::new("level", "Level").width(px(88.0)).resizable(false),
                Column::new("message", "Message")
                    .width(px(1600.0))
                    .resizable(false),
            ],
        }
    }
}

impl TableDelegate for LogTableDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.store.borrow().visible_count()
    }

    fn column(&self, col_ix: usize, _cx: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn render_td(
        &self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<Table<Self>>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let borrowed = self.store.borrow();
        let Some(row) = borrowed.row_for_visible(row_ix) else {
            return div().into_any_element();
        };

        match col_ix {
            0 => div()
                .w_full()
                .px_2()
                .text_color(theme.muted_foreground)
                .child(format!("{}", row.abs_line))
                .into_any_element(),
            1 => div()
                .w_full()
                .px_2()
                .text_color(row.level.color(&theme))
                .child(row.level.label())
                .into_any_element(),
            _ => div()
                .w_full()
                .px_2()
                .text_color(row.level.color(&theme))
                .child(row.text.clone())
                .into_any_element(),
        }
    }

    fn load_more(&mut self, _window: &mut Window, _cx: &mut Context<Table<Self>>) {}

    fn is_eof(&self, _cx: &App) -> bool {
        true
    }

    fn visible_rows_changed(
        &mut self,
        _visible_range: Range<usize>,
        _window: &mut Window,
        _cx: &mut Context<Table<Self>>,
    ) {
    }
}

pub struct LogDrawer {
    store: Rc<RefCell<LogStore>>,
    table: Option<Entity<Table<LogTableDelegate>>>,
    locked_to_bottom: bool,
    error_message: Option<String>,
    _background_task: Option<Task<()>>,
}

impl LogDrawer {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            store: Rc::new(RefCell::new(LogStore::new())),
            table: None,
            locked_to_bottom: true,
            error_message: None,
            _background_task: None,
        }
    }

    fn ensure_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.table.is_some() {
            return;
        }

        let delegate = LogTableDelegate::new(self.store.clone());
        let table = cx.new(|cx| {
            Table::new(delegate, window, cx)
                .sortable(false)
                .col_movable(false)
                .col_resizable(false)
                .row_selectable(false)
                .col_selectable(false)
                .loop_selection(false)
                .stripe(true)
        });

        self.table = Some(table);
    }

    pub fn start_monitoring(&mut self, cx: &mut Context<Self>) {
        if self._background_task.is_some() {
            return;
        }

        let rx = crate::subscribe_live_logs();

        let task = cx.spawn(async move |this, cx| {
            while let Ok(first_line) = rx.recv().await {
                let mut batch = Vec::with_capacity(LIVE_BATCH_MAX_LINES);
                batch.push(first_line);

                while batch.len() < LIVE_BATCH_MAX_LINES {
                    match rx.try_recv() {
                        Ok(line) => batch.push(line),
                        Err(_) => break,
                    }
                }

                let _ = cx.update(|cx| {
                    if let Some(this) = this.upgrade() {
                        this.update(cx, |drawer, cx| {
                            drawer.ingest_lines(batch, cx);
                        });
                    }
                });
            }
        });

        self._background_task = Some(task);
        self.error_message = None;
        cx.notify();
    }

    pub fn stop_monitoring(&mut self) {
        self._background_task = None;
    }

    fn ingest_lines(&mut self, lines: Vec<String>, cx: &mut Context<Self>) {
        if lines.is_empty() {
            return;
        }

        self.store.borrow_mut().append_batch(lines);

        self.refresh_table(cx);
        if self.locked_to_bottom {
            self.scroll_to_bottom(cx);
        }

        cx.notify();
    }

    fn refresh_table(&mut self, cx: &mut Context<Self>) {
        if let Some(table) = self.table.clone() {
            table.update(cx, |_, cx| {
                cx.notify();
            });
        }
    }

    fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        let visible_count = self.store.borrow().visible_count();
        if visible_count == 0 {
            return;
        }

        if let Some(table) = self.table.clone() {
            table.update(cx, |table, cx| {
                table.scroll_to_row(visible_count - 1, cx);
            });
        }
    }

    fn clear_logs(&mut self, cx: &mut Context<Self>) {
        self.store.borrow_mut().clear();
        self.refresh_table(cx);
        cx.notify();
    }

    fn set_level_filter(&mut self, level: Option<LogLevel>, cx: &mut Context<Self>) {
        self.store.borrow_mut().set_level_filter(level);
        self.refresh_table(cx);

        if self.locked_to_bottom {
            self.scroll_to_bottom(cx);
        }

        cx.notify();
    }

    fn jump_to_latest(
        &mut self,
        _event: &gpui::ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.locked_to_bottom = true;
        self.scroll_to_bottom(cx);
        cx.notify();
    }

    fn handle_scroll(
        &mut self,
        event: &gpui::ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scrolling_up = match event.delta {
            ScrollDelta::Pixels(delta) => delta.y > px(0.0),
            ScrollDelta::Lines(delta) => delta.y > 0.0,
        };

        if scrolling_up && self.locked_to_bottom {
            self.locked_to_bottom = false;
            cx.notify();
        }
    }
}

impl Render for LogDrawer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_table(window, cx);
        let theme = cx.theme().clone();

        let store = self.store.borrow();
        let visible_count = store.visible_count();
        let buffered_count = store.rows.len();
        let total_seen = store.total_seen;
        let dropped_total = store.dropped_total;
        drop(store);

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                h_flex()
                    .w_full()
                    .h(px(44.0))
                    .px_4()
                    .items_center()
                    .justify_between()
                    .bg(theme.background)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "{} shown | {} buffered | {} seen | {} dropped",
                                visible_count, buffered_count, total_seen, dropped_total
                            )),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                Button::new("clear-logs")
                                    .label("Clear")
                                    .icon(IconName::Trash)
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.clear_logs(cx);
                                    })),
                            )
                            .when(!self.locked_to_bottom, |this| {
                                this.child(
                                    Button::new("jump-to-latest")
                                        .label("Jump to Latest")
                                        .icon(IconName::ChevronDown)
                                        .on_click(cx.listener(Self::jump_to_latest)),
                                )
                            }),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .h(px(44.0))
                    .px_4()
                    .items_center()
                    .gap_2()
                    .bg(theme.background)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("filter-all")
                            .label("All")
                            .when(self.store.borrow().level_filter.is_none(), |btn| btn.primary())
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.set_level_filter(None, cx);
                            })),
                    )
                    .child(
                        Button::new("filter-error")
                            .label("Errors")
                            .when(
                                self.store.borrow().level_filter == Some(LogLevel::Error),
                                |btn| btn.primary(),
                            )
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.set_level_filter(Some(LogLevel::Error), cx);
                            })),
                    )
                    .child(
                        Button::new("filter-warn")
                            .label("Warnings")
                            .when(
                                self.store.borrow().level_filter == Some(LogLevel::Warn),
                                |btn| btn.primary(),
                            )
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.set_level_filter(Some(LogLevel::Warn), cx);
                            })),
                    )
                    .child(
                        Button::new("filter-info")
                            .label("Info")
                            .when(
                                self.store.borrow().level_filter == Some(LogLevel::Info),
                                |btn| btn.primary(),
                            )
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.set_level_filter(Some(LogLevel::Info), cx);
                            })),
                    )
                    .child(
                        Button::new("filter-debug")
                            .label("Debug")
                            .when(
                                self.store.borrow().level_filter == Some(LogLevel::Debug),
                                |btn| btn.primary(),
                            )
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.set_level_filter(Some(LogLevel::Debug), cx);
                            })),
                    )
                    .child(
                        Button::new("filter-trace")
                            .label("Trace")
                            .when(
                                self.store.borrow().level_filter == Some(LogLevel::Trace),
                                |btn| btn.primary(),
                            )
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.set_level_filter(Some(LogLevel::Trace), cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .on_scroll_wheel(cx.listener(Self::handle_scroll))
                    .map(|this| {
                        if let Some(ref error) = self.error_message {
                            this.child(
                                v_flex().size_full().items_center().justify_center().child(
                                    div().text_color(theme.muted_foreground).child(error.clone()),
                                ),
                            )
                        } else if let Some(table) = self.table.clone() {
                            this.child(table)
                        } else {
                            this.child(
                                v_flex().size_full().items_center().justify_center().child(
                                    div().text_color(theme.muted_foreground).child("Loading table..."),
                                ),
                            )
                        }
                    }),
            )
    }
}
