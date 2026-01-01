use gpui::{prelude::*, *};
use ui::{
    h_flex, v_flex, button::Button, table::{Column, ColumnSort, Table, TableDelegate, TableEvent},
    input::{TextInput, InputState, TabSize},
    label::Label, IconName,
    ActiveTheme, Sizable, Size, StyleSized, StyledExt, Selectable,
};
use std::ops::Range;
use crate::{
    database::{DatabaseManager, RowData, CellValue},
    reflection::TypeSchema,
    cell_editors::{CellEditor, CellEditorView},
};

pub struct DataTableState {
    pub editing_cell: Option<(usize, usize)>, // (row_idx, col_idx)
    pub selected_row: Option<usize>,
    pub selected_rows: Vec<usize>, // Multi-select support
    pub edit_input: Option<Entity<InputState>>,
    pub filter_text: String,
    pub validation_error: Option<String>,
    pub show_only_modified: bool,
    pub copied_cell: Option<String>,
}

pub struct DataTableView {
    db: DatabaseManager,
    table_name: String,
    schema: TypeSchema,
    rows: Vec<RowData>,
    columns: Vec<Column>,
    size: Size,
    total_rows: usize,
    visible_range: Range<usize>,
    pub state: DataTableState,
}

impl DataTableView {
    pub fn new(db: DatabaseManager, table_name: String) -> anyhow::Result<Self> {
        let schema = db
            .get_schema(&table_name)
            .ok_or_else(|| anyhow::anyhow!("Schema not found for table: {}", table_name))?;

        let total_rows = db.get_row_count(&table_name)?;
        let rows = db.fetch_rows(&table_name, 0, 100)?;

        let mut columns = vec![
            Column::new("id", "ID")
                .width(60.)
                .resizable(false)
                .fixed(ui::table::ColumnFixed::Left)
                .sortable(),
        ];

        for field in &schema.fields {
            columns.push(
                Column::new(&field.name, &field.name)
                    .width(150.)
                    .sortable()
            );
        }

        Ok(Self {
            db,
            table_name,
            schema,
            rows,
            columns,
            size: Size::default(),
            total_rows,
            visible_range: 0..0,
            state: DataTableState {
                editing_cell: None,
                selected_row: None,
                selected_rows: Vec::new(),
                edit_input: None,
                filter_text: String::new(),
                validation_error: None,
                show_only_modified: false,
                copied_cell: None,
            },
        })
    }

    pub fn set_filter(&mut self, filter: String) -> anyhow::Result<()> {
        self.state.filter_text = filter.clone();

        // If filter is empty, fetch all rows
        if filter.is_empty() {
            self.refresh_rows(0, 100)?;
            return Ok(());
        }

        // Apply filter by fetching rows that match
        // This is a simple implementation - could be improved with SQL WHERE clauses
        self.refresh_rows(0, 1000)?; // Fetch more rows for filtering

        // Filter rows based on text match in any column
        self.rows.retain(|row| {
            row.cells.iter().any(|cell| {
                cell.display.to_lowercase().contains(&filter.to_lowercase())
            })
        });

        Ok(())
    }

    pub fn start_edit_cell(&mut self, row_idx: usize, col_idx: usize, window: &mut Window, cx: &mut Context<Table<Self>>) {
        if let Some(row) = self.rows.get(row_idx) {
            if col_idx > 0 && col_idx <= self.schema.fields.len() {
                let cell_idx = col_idx - 1;
                if let Some(cell) = row.cells.get(cell_idx) {
                    // Create an input state for editing
                    let edit_input = cx.new(|cx| {
                        let mut state = InputState::new(window, cx)
                            .tab_size(TabSize {
                                tab_size: 4,
                                hard_tabs: false,
                            });
                        state.set_value(&cell.display, window, cx);
                        state
                    });

                    self.state.editing_cell = Some((row_idx, col_idx));
                    self.state.edit_input = Some(edit_input);
                    self.state.validation_error = None;
                }
            }
        }
    }

    pub fn validate_cell_value(&self, col_idx: usize, value: &str) -> Result<serde_json::Value, String> {
        if col_idx == 0 || col_idx > self.schema.fields.len() {
            return Err("Invalid column index".to_string());
        }

        let field = &self.schema.fields[col_idx - 1];

        match &field.sql_type {
            crate::reflection::SqlType::Integer => {
                value.parse::<i64>()
                    .map(|v| serde_json::Value::Number(v.into()))
                    .map_err(|_| format!("'{}' is not a valid integer", value))
            }
            crate::reflection::SqlType::Real => {
                value.parse::<f64>()
                    .ok()
                    .and_then(|v| serde_json::Number::from_f64(v))
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| format!("'{}' is not a valid number", value))
            }
            crate::reflection::SqlType::Boolean => {
                match value.to_lowercase().as_str() {
                    "true" | "1" | "yes" | "t" | "y" => Ok(serde_json::Value::Bool(true)),
                    "false" | "0" | "no" | "f" | "n" => Ok(serde_json::Value::Bool(false)),
                    _ => Err(format!("'{}' is not a valid boolean (use true/false)", value))
                }
            }
            _ => {
                if field.nullable && value.trim().is_empty() {
                    Ok(serde_json::Value::Null)
                } else {
                    Ok(serde_json::Value::String(value.to_string()))
                }
            }
        }
    }

    pub fn refresh_rows(&mut self, offset: usize, limit: usize) -> anyhow::Result<()> {
        self.rows = self.db.fetch_rows(&self.table_name, offset, limit)?;
        self.total_rows = self.db.get_row_count(&self.table_name)?;
        Ok(())
    }

    pub fn add_new_row(&mut self) -> anyhow::Result<()> {
        let default_values: Vec<serde_json::Value> = self
            .schema
            .fields
            .iter()
            .map(|field| {
                if field.nullable {
                    serde_json::Value::Null
                } else {
                    match field.sql_type {
                        crate::reflection::SqlType::Integer => serde_json::Value::Number(0.into()),
                        crate::reflection::SqlType::Real => {
                            serde_json::Number::from_f64(0.0)
                                .map(serde_json::Value::Number)
                                .unwrap_or(serde_json::Value::Null)
                        }
                        crate::reflection::SqlType::Boolean => serde_json::Value::Bool(false),
                        _ => serde_json::Value::String(String::new()),
                    }
                }
            })
            .collect();

        self.db.insert_row(&self.table_name, default_values)?;
        self.refresh_rows(0, 100)?;
        Ok(())
    }

    pub fn delete_row(&mut self, row_idx: usize) -> anyhow::Result<()> {
        if let Some(row) = self.rows.get(row_idx) {
            self.db.delete_row(&self.table_name, row.id)?;
            self.refresh_rows(0, 100)?;
        }
        Ok(())
    }

    pub fn update_cell(
        &mut self,
        row_idx: usize,
        col_idx: usize,
        value: serde_json::Value,
    ) -> anyhow::Result<()> {
        if let Some(row) = self.rows.get(row_idx) {
            if col_idx > 0 && col_idx <= self.schema.fields.len() {
                let field = &self.schema.fields[col_idx - 1];
                self.db.update_cell(&self.table_name, row.id, &field.name, value)?;
                self.refresh_rows(0, 100)?;
            }
        }
        Ok(())
    }
    
    pub fn save_editing_cell(&mut self, cx: &App) -> anyhow::Result<()> {
        if let (Some((row_idx, col_idx)), Some(ref edit_input)) = (self.state.editing_cell, &self.state.edit_input) {
            let value_str = edit_input.read(cx).value().to_string();

            // Validate the value
            match self.validate_cell_value(col_idx, &value_str) {
                Ok(value) => {
                    if let Some(row) = self.rows.get(row_idx) {
                        if col_idx > 0 && col_idx <= self.schema.fields.len() {
                            let field = &self.schema.fields[col_idx - 1];
                            self.db.update_cell(&self.table_name, row.id, &field.name, value)?;
                            self.refresh_rows(0, 100)?;
                        }
                    }

                    self.state.editing_cell = None;
                    self.state.edit_input = None;
                    self.state.validation_error = None;
                }
                Err(err) => {
                    self.state.validation_error = Some(err);
                    return Err(anyhow::anyhow!("Validation error"));
                }
            }
        }
        Ok(())
    }

    pub fn cancel_edit(&mut self) {
        self.state.editing_cell = None;
        self.state.edit_input = None;
        self.state.validation_error = None;
    }

    pub fn copy_cell_value(&mut self, row_idx: usize, col_idx: usize) {
        if let Some(row) = self.rows.get(row_idx) {
            if col_idx == 0 {
                self.state.copied_cell = Some(row.id.to_string());
            } else if let Some(cell) = row.cells.get(col_idx - 1) {
                self.state.copied_cell = Some(cell.display.clone());
            }
        }
    }

    pub fn duplicate_row(&mut self, row_idx: usize) -> anyhow::Result<()> {
        if let Some(row) = self.rows.get(row_idx) {
            let values: Vec<serde_json::Value> = row.cells.iter()
                .map(|cell| cell.value.clone())
                .collect();

            self.db.insert_row(&self.table_name, values)?;
            self.refresh_rows(0, 1000)?;
        }
        Ok(())
    }

    pub fn copy_row_as_insert(&self, row_idx: usize) -> Option<String> {
        if let Some(row) = self.rows.get(row_idx) {
            let field_names: Vec<String> = self.schema.fields.iter()
                .map(|f| f.name.clone())
                .collect();

            let values: Vec<String> = row.cells.iter()
                .map(|cell| match &cell.value {
                    serde_json::Value::Null => "NULL".to_string(),
                    serde_json::Value::String(s) => format!("'{}'", s.replace("'", "''")),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
                    _ => format!("'{}'", cell.display.replace("'", "''")),
                })
                .collect();

            Some(format!(
                "INSERT INTO {} ({}) VALUES ({});",
                self.table_name,
                field_names.join(", "),
                values.join(", ")
            ))
        } else {
            None
        }
    }

    pub fn get_table_stats(&self) -> String {
        format!(
            "Total: {} rows | Virtual Scrolling (infinite)",
            self.total_rows
        )
    }

}

impl DataTableView {
    pub fn enable_features(&self, table: &mut ui::table::Table<Self>, cx: &mut Context<ui::table::Table<Self>>) {
        table.col_fixed = true;
        table.col_resizable = true;
        table.sortable = true;
        cx.notify();
    }
}

impl TableDelegate for DataTableView {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn render_th(
        &self,
        col_ix: usize,
        _: &mut Window,
        _cx: &mut Context<Table<Self>>,
    ) -> impl IntoElement {
        let col = &self.columns[col_ix];

        div()
            .child(col.name.clone())
            .text_sm()
            .font_semibold()
            .px_2()
            .py_1()
    }

    fn render_tr(
        &self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<Table<Self>>,
    ) -> Stateful<Div> {
        let is_selected = self.state.selected_row == Some(row_ix);
        div()
            .id(row_ix)
            .cursor_pointer()
            .on_click(cx.listener(move |table, _, _, cx| {
                table.delegate_mut().state.selected_row = Some(row_ix);
                cx.notify();
            }))
            .when(is_selected, |this| {
                this.bg(cx.theme().accent.opacity(0.1))
            })
    }

    fn render_td(
        &self,
        row_ix: usize,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<Table<Self>>,
    ) -> impl IntoElement {
        if let Some(row) = self.rows.get(row_ix) {
            if col_ix == 0 {
                return div()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(row.id.to_string())
                    .into_any_element();
            }

            let cell_idx = col_ix - 1;
            if let Some(cell) = row.cells.get(cell_idx) {
                let is_editing = self.state.editing_cell == Some((row_ix, col_ix));
                let display = cell.display.clone();

                if is_editing {
                    if let Some(ref edit_input) = self.state.edit_input {
                        // Show proper text input for editing
                        let has_error = self.state.validation_error.is_some();

                        return div()
                            .id(("cell-edit", row_ix * 1000 + col_ix))
                            .w_full()
                            .h_full()
                            .relative()
                            .child(
                                div()
                                    .size_full()
                                    .border_2()
                                    .border_color(if has_error {
                                        cx.theme().red
                                    } else {
                                        cx.theme().accent
                                    })
                                    .rounded_sm()
                                    .overflow_hidden()
                                    .child(
                                        TextInput::new(edit_input)
                                            .w_full()
                                            .h_full()
                                            .text_sm()
                                            .px_2()
                                            .py_1()
                                            .border_0()
                                    )
                            )
                            .when_some(self.state.validation_error.as_ref(), |this, error| {
                                this.child(
                                    div()
                                        .absolute()
                                        .top_full()
                                        .left_0()
                                        .mt_1()
                                        .px_2()
                                        .py_1()
                                        .bg(cx.theme().red)
                                        .text_color(cx.theme().background)
                                        .text_xs()
                                        .rounded_sm()
                                        .shadow_lg()
                                        .child(error.clone())
                                )
                            })
                            .into_any_element();
                    }
                }

                // Regular cell display
                return div()
                    .id(("cell", row_ix * 1000 + col_ix))
                    .px_2()
                    .py_1()
                    .text_sm()
                    .cursor_pointer()
                    .on_click(cx.listener(move |table, _, window, cx| {
                        let delegate = table.delegate_mut();
                        delegate.start_edit_cell(row_ix, col_ix, window, cx);
                        cx.notify();
                    }))
                    .hover(|this| this.bg(cx.theme().muted.opacity(0.5)))
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
            .px_2()
            .py_1()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child("—")
            .into_any_element()
    }

    fn visible_rows_changed(
        &mut self,
        visible_range: Range<usize>,
        _: &mut Window,
        _cx: &mut Context<Table<Self>>,
    ) {
        self.visible_range = visible_range.clone();

        if visible_range.end > self.rows.len() && self.rows.len() < self.total_rows {
            if let Err(e) = self.refresh_rows(0, visible_range.end + 50) {
                eprintln!("Failed to load more rows: {}", e);
            }
        }
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<Table<Self>>,
    ) {
        tracing::debug!("Sort column {} {:?}", col_ix, sort);
    }

    fn loading(&self, _: &App) -> bool {
        false
    }

    fn is_eof(&self, _: &App) -> bool {
        self.rows.len() >= self.total_rows
    }
}
