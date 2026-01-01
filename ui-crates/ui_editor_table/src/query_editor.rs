use gpui::{prelude::*, *};
use ui::{
    h_flex, v_flex, button::{Button, ButtonVariants}, label::Label,
    input::{TextInput, InputState, TabSize, InputEvent},
    divider::Divider, IconName,
    table::{Table, TableDelegate, Column, ColumnSort, TableEvent},
    ActiveTheme, Sizable, Size, StyleSized, StyledExt, Disableable,
};
use crate::database::{DatabaseManager, CellValue};
use std::time::Instant;
use std::ops::Range;

pub struct QueryEditor {
    db: DatabaseManager,
    query_input: Entity<InputState>,
    results: Option<QueryResult>,
    results_table: Option<Entity<Table<QueryResultsTableView>>>,
    error: Option<String>,
    is_executing: bool,
    query_history: Vec<SavedQuery>,
    focus_handle: FocusHandle,
    show_schema_sidebar: bool,
    available_tables: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SavedQuery {
    pub name: String,
    pub sql: String,
    pub timestamp: Instant,
}

#[derive(Clone)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<CellValue>>,
    pub row_count: usize,
    pub execution_time_ms: u64,
}

pub struct QueryResultsTableView {
    result: QueryResult,
    columns: Vec<Column>,
    size: Size,
    visible_range: Range<usize>,
}

impl QueryResultsTableView {
    pub fn new(result: QueryResult) -> Self {
        let mut columns = Vec::new();

        // Add columns based on result columns
        for (idx, col_name) in result.columns.iter().enumerate() {
            let id = format!("col_{}", idx);
            let mut column = Column::new(&id, col_name)
                .width(150.0)
                .resizable(true)
                .sortable();

            // Pin first 2 columns (usually ID and key columns)
            if idx < 2 {
                column = column.fixed(ui::table::ColumnFixed::Left);
            }

            columns.push(column);
        }

        Self {
            result,
            columns,
            size: Size::default(),
            visible_range: 0..0,
        }
    }
}

impl TableDelegate for QueryResultsTableView {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.result.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn render_th(
        &self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<Table<Self>>,
    ) -> impl IntoElement {
        if let Some(col) = self.result.columns.get(col_ix) {
            div()
                .px_3()
                .py_2()
                .text_sm()
                .font_semibold()
                .child(col.clone())
        } else {
            div()
        }
    }

    fn render_tr(
        &self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<Table<Self>>,
    ) -> Stateful<Div> {
        div()
            .id(("result-row", row_ix))
            .when(row_ix % 2 == 1, |this| {
                this.bg(cx.theme().muted.opacity(0.3))
            })
    }

    fn render_td(
        &self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<Table<Self>>,
    ) -> impl IntoElement {
        if let Some(row) = self.result.rows.get(row_ix) {
            if let Some(cell) = row.get(col_ix) {
                let display = cell.display.clone();

                return div()
                    .id(("result-cell", row_ix * 10000 + col_ix))
                    .px_3()
                    .py_2()
                    .text_sm()
                    .when(display.is_empty() || display == "NULL", |this| {
                        this.text_color(cx.theme().muted_foreground)
                            .italic()
                            .child(if display.is_empty() { "empty" } else { "NULL" })
                    })
                    .when(!display.is_empty() && display != "NULL", |this| {
                        this.child(display)
                    })
                    .into_any_element();
            }
        }

        div()
            .px_3()
            .py_2()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child("—")
            .into_any_element()
    }

    fn visible_rows_changed(
        &mut self,
        visible_range: Range<usize>,
        _: &mut Window,
        _: &mut Context<Table<Self>>,
    ) {
        self.visible_range = visible_range;
    }
}

impl QueryEditor {
    pub fn new(db: DatabaseManager, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Get available tables for schema browser
        let available_tables = db.list_tables().unwrap_or_default();

        // Create query input with SQL syntax highlighting
        let query_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .code_editor("sql")
                .line_number(true)
                .minimap(false)
                .tab_size(TabSize {
                    tab_size: 2,
                    hard_tabs: false,
                })
                .soft_wrap(false);

            state.set_value("SELECT * FROM ", window, cx);
            state
        });

        Self {
            db,
            query_input,
            results: None,
            results_table: None,
            error: None,
            is_executing: false,
            query_history: Vec::new(),
            focus_handle: cx.focus_handle(),
            show_schema_sidebar: true,
            available_tables,
        }
    }

    pub fn set_query(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        self.query_input.update(cx, |state, cx| {
            state.set_value(&query, window, cx);
        });
    }

    pub fn get_query(&self, cx: &App) -> String {
        self.query_input.read(cx).value().to_string()
    }

    pub fn toggle_schema_sidebar(&mut self) {
        self.show_schema_sidebar = !self.show_schema_sidebar;
    }

    pub fn save_query(&mut self, name: String, cx: &App) {
        let sql = self.get_query(cx);
        self.query_history.push(SavedQuery {
            name,
            sql,
            timestamp: Instant::now(),
        });
    }

    pub fn load_query(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(saved) = self.query_history.get(index) {
            self.set_query(saved.sql.clone(), window, cx);
        }
    }

    pub fn insert_table_name(&mut self, table_name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.query_input.update(cx, |state, cx| {
            let current = state.value().to_string();
            let new_value = format!("{}{}", current, table_name);
            state.set_value(&new_value, window, cx);
        });
    }

    pub fn execute_query(&mut self, window: &mut Window, cx: &mut Context<Self>) -> anyhow::Result<()> {
        self.is_executing = true;
        self.error = None;

        let start = std::time::Instant::now();
        let query = self.get_query(cx);

        match self.db.execute_query(&query) {
            Ok(rows) => {
                let execution_time_ms = start.elapsed().as_millis() as u64;
                let row_count = rows.len();

                let columns = if !rows.is_empty() && !rows[0].is_empty() {
                    (0..rows[0].len())
                        .map(|i| format!("Column {}", i + 1))
                        .collect()
                } else {
                    Vec::new()
                };

                let result = QueryResult {
                    columns,
                    rows,
                    row_count,
                    execution_time_ms,
                };

                // Create virtualized table for results
                let table_view = QueryResultsTableView::new(result.clone());
                let results_table = cx.new(|cx| {
                    let mut table = Table::new(table_view, window, cx);
                    table.col_fixed = true;
                    table.col_resizable = true;
                    table.sortable = true;
                    table
                });

                self.results = Some(result);
                self.results_table = Some(results_table);
            }
            Err(e) => {
                self.error = Some(format!("Query error: {}", e));
                self.results = None;
                self.results_table = None;
            }
        }

        self.is_executing = false;
        Ok(())
    }

    pub fn clear_results(&mut self) {
        self.results = None;
        self.results_table = None;
        self.error = None;
    }

    pub fn render_query_input(&self, cx: &mut Context<QueryEditor>) -> impl IntoElement {
        v_flex()
            .size_full()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Label::new("SQL Query Editor")
                            .text_sm()
                            .font_semibold()
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("toggle-schema")
                                    .icon(if self.show_schema_sidebar { IconName::PanelLeft } else { IconName::PanelRight })
                                    .tooltip("Toggle Schema Browser")
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener(|editor, _, _, cx| {
                                        editor.toggle_schema_sidebar();
                                        cx.notify();
                                    }))
                            )
                    )
            )
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded_md()
                    .overflow_hidden()
                    .child(
                        TextInput::new(&self.query_input)
                            .size_full()
                            .font_family("monospace")
                            .font(gpui::Font {
                                family: "Jetbrains Mono".to_string().into(),
                                weight: gpui::FontWeight::NORMAL,
                                style: gpui::FontStyle::Normal,
                                features: gpui::FontFeatures::default(),
                                fallbacks: Some(gpui::FontFallbacks::from_fonts(vec!["monospace".to_string()])),
                            })
                            .text_size(px(14.0))
                            .border_0()
                    )
            )
    }

    pub fn render_controls(&self, cx: &Context<QueryEditor>) -> impl IntoElement {
        h_flex()
            .gap_2()
            .items_center()
            .p_2()
            .bg(cx.theme().muted.opacity(0.3))
            .border_y_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("execute")
                    .icon(IconName::ArrowRight)
                    .label(if self.is_executing { "Executing..." } else { "Execute" })
                    .tooltip("Execute Query (F5 or Ctrl+Enter)")
                    .disabled(self.is_executing)
                    .primary()
                    .small()
                    .on_click(cx.listener(|editor, _, window, cx| {
                        if let Err(e) = editor.execute_query(window, cx) {
                            tracing::error!("Failed to execute query: {}", e);
                        }
                        cx.notify();
                    }))
            )
            .child(
                Button::new("clear")
                    .icon(IconName::Close)
                    .label("Clear")
                    .tooltip("Clear Results")
                    .outline()
                    .small()
                    .on_click(cx.listener(|editor, _, _, cx| {
                        editor.clear_results();
                        cx.notify();
                    }))
            )
            .child(Divider::vertical().h_6())
            .child(
                Button::new("save-query")
                    .icon(IconName::FloppyDisk)
                    .label("Save")
                    .tooltip("Save Query to History")
                    .outline()
                    .small()
                    .on_click(cx.listener(|editor, _, _, cx| {
                        let name = format!("Query {}", editor.query_history.len() + 1);
                        editor.save_query(name, cx);
                        cx.notify();
                    }))
            )
            .child(
                Button::new("export-csv")
                    .icon(IconName::Download)
                    .label("Export CSV")
                    .tooltip("Export Results to CSV")
                    .outline()
                    .small()
                    .disabled(self.results.is_none())
                    .on_click(cx.listener(|editor, _, _, cx| {
                        if let Some(ref results) = editor.results {
                            if let Err(e) = editor.export_to_csv(results) {
                                tracing::error!("Failed to export CSV: {}", e);
                            }
                        }
                        cx.notify();
                    }))
            )
            .child(
                Button::new("export-json")
                    .icon(IconName::Download)
                    .label("Export JSON")
                    .tooltip("Export Results to JSON")
                    .outline()
                    .small()
                    .disabled(self.results.is_none())
                    .on_click(cx.listener(|editor, _, _, cx| {
                        if let Some(ref results) = editor.results {
                            if let Err(e) = editor.export_to_json(results) {
                                tracing::error!("Failed to export JSON: {}", e);
                            }
                        }
                        cx.notify();
                    }))
            )
            .when(self.results.is_some(), |this| {
                let result = self.results.as_ref().unwrap();
                this.child(Divider::vertical().h_6())
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "📊 {} rows in {} ms",
                                result.row_count, result.execution_time_ms
                            ))
                    )
            })
    }

    pub fn export_to_csv(&self, results: &QueryResult) -> anyhow::Result<()> {
        use std::fs::File;
        use std::io::Write;

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let filename = format!("query_results_{}.csv", timestamp);

        let mut file = File::create(&filename)?;

        // Write header
        writeln!(file, "{}", results.columns.join(","))?;

        // Write rows
        for row in &results.rows {
            let row_str = row.iter()
                .map(|cell| {
                    let val = cell.display.replace("\"", "\"\"");
                    if val.contains(',') || val.contains('"') || val.contains('\n') {
                        format!("\"{}\"", val)
                    } else {
                        val
                    }
                })
                .collect::<Vec<_>>()
                .join(",");
            writeln!(file, "{}", row_str)?;
        }

            tracing::debug!("✓ Exported {} rows to {}", results.row_count, filename);
        Ok(())
    }

    pub fn export_to_json(&self, results: &QueryResult) -> anyhow::Result<()> {
        use std::fs::File;
        use std::io::Write;

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let filename = format!("query_results_{}.json", timestamp);

        let mut rows_json = Vec::new();
        for row in &results.rows {
            let mut row_map = serde_json::Map::new();
            for (i, cell) in row.iter().enumerate() {
                if let Some(col_name) = results.columns.get(i) {
                    row_map.insert(col_name.clone(), cell.value.clone());
                }
            }
            rows_json.push(serde_json::Value::Object(row_map));
        }

        let json = serde_json::to_string_pretty(&rows_json)?;
        let mut file = File::create(&filename)?;
        file.write_all(json.as_bytes())?;

        tracing::info!("✓ Exported {} rows to {}", results.row_count, filename);
        Ok(())
    }

    pub fn render_schema_sidebar(&self, cx: &Context<QueryEditor>) -> impl IntoElement {
        v_flex()
            .w_64()
            .h_full()
            .bg(cx.theme().muted.opacity(0.2))
            .border_r_1()
            .border_color(cx.theme().border)
            .gap_2()
            .p_2()
            .child(
                Label::new("Database Schema")
                    .text_sm()
                    .font_semibold()
                    .px_2()
            )
            .child(Divider::horizontal())
            .child(
                v_flex()
                    .flex_1()
                    .gap_1()
                    .children(self.available_tables.iter().enumerate().map(|(idx, table)| {
                        let table_name = table.clone();
                        v_flex()
                            .w_full()
                            .gap_1()
                            .child(
                                div()
                                    .id(("schema-table", idx))
                                    .w_full()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .text_sm()
                                    .font_semibold()
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |editor, _, window, cx| {
                                        editor.insert_table_name(&table_name, window, cx);
                                        cx.notify();
                                    }))
                                    .hover(|this| this.bg(cx.theme().muted))
                                    .child(format!("📋 {}", table))
                            )
                            .when_some(self.db.get_schema(table), |this, schema| {
                                this.child(
                                    v_flex()
                                        .pl_4()
                                        .gap_px()
                                        .children(schema.fields.iter().map(|field| {
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .px_2()
                                                .py_1()
                                                .child(format!("  • {} ({:?})", field.name, field.sql_type))
                                        }))
                                )
                            })
                    }))
            )
            .child(Divider::horizontal())
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new("Query History")
                            .text_xs()
                            .font_semibold()
                            .px_2()
                    )
                    .child(
                        v_flex()
                            .max_h_32()
                            .gap_1()
                            .children(self.query_history.iter().enumerate().rev().map(|(idx, saved)| {
                                div()
                                    .id(("history", idx))
                                    .w_full()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |editor, _, window, cx| {
                                        editor.load_query(idx, window, cx);
                                        cx.notify();
                                    }))
                                    .hover(|this| this.bg(cx.theme().muted))
                                    .child(saved.name.clone())
                            }))
                    )
            )
    }

    pub fn render_results(&self, cx: &Context<QueryEditor>) -> impl IntoElement {
        v_flex()
            .size_full()
            .gap_2()
            .when_some(self.error.as_ref(), |this, error| {
                this.child(
                    div()
                        .w_full()
                        .p_4()
                        .bg(cx.theme().red.opacity(0.1))
                        .border_1()
                        .border_color(cx.theme().red)
                        .rounded_md()
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .text_color(cx.theme().red)
                                        .child("❌ Query Error")
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().red)
                                        .child(error.clone())
                                )
                        )
                )
            })
            .when_some(self.results_table.as_ref(), |this, table| {
                this.child(
                    div()
                        .w_full()
                        .flex_1()
                        .min_h_0()
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded_md()
                        .child(table.clone())
                )
            })
            .when(self.results_table.is_none() && self.error.is_none(), |this| {
                this.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            v_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_lg()
                                        .font_semibold()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("💻 Ready to execute query")
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Press F5 or click Execute to run your SQL query")
                                )
                        )
                )
            })
    }
}

pub struct QueryEditorView {
    editor: Entity<QueryEditor>,
}

impl QueryEditorView {
    pub fn new(db: DatabaseManager, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| QueryEditor::new(db, window, cx));
        Self { editor }
    }
}

impl Focusable for QueryEditorView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.read(cx).focus_handle.clone()
    }
}

impl Render for QueryEditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let show_sidebar = self.editor.read(cx).show_schema_sidebar;

        //let schema_sidebar = self.editor.update(cx, |editor, cx| {
        //    editor.render_schema_sidebar(cx)
        //});

        let query_input = self.editor.update(cx, |editor, cx| {
            editor.render_query_input(cx)
        });

        let controls = self.editor.update(cx, |editor, cx| {
            editor.render_controls(cx)
        });

        let results = self.editor.update(cx, |editor, cx| {
            editor.render_results(cx)
        });

        h_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                v_flex()
                    .flex_1()
                    .size_full()
                    .child(controls)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h_0()
                            .gap_4()
                            .p_4()
                            .child(
                                v_flex()
                                    .w_full()
                                    .h_64()
                                    .child(query_input)
                            )
                            .child(
                                div()
                                    .w_full()
                                    .flex_1()
                                    .min_h_0()
                                    .child(results)
                            )
                    )
            )
    }
}
