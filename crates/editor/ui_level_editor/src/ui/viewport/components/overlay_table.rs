//! The viewport's table overlays: a titled card with fixed columns over a list
//! of rows, as the render pipeline and render stats overlays show.
//!
//! The rows are virtual: the body is a `uniform_list` no taller than
//! [`MAX_LIST_HEIGHT`], which lays out and builds only the rows in view, so a
//! table of thousands of rows costs what the visible ones cost and scrolls
//! within its card. Every overlay of this kind takes its width, height limit,
//! row height and chrome from here, so they line up and behave the same.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::{h_flex, v_flex, ActiveTheme};

/// The card's width.
pub const CARD_WIDTH: f32 = 410.0;
/// The tallest the rows' list grows before it scrolls.
pub const MAX_LIST_HEIGHT: f32 = 300.0;
/// One row's height (uniform: the list measures it, not each row).
pub const ROW_HEIGHT: f32 = 16.0;
/// Room for a row's colour swatch, when the table has them.
const SWATCH_WIDTH: f32 = 16.0;

/// A column: its header, width and alignment.
#[derive(Clone, Copy)]
pub struct Column {
    pub header: &'static str,
    pub width: f32,
    pub align_right: bool,
}

impl Column {
    pub const fn left(header: &'static str, width: f32) -> Self {
        Self {
            header,
            width,
            align_right: false,
        }
    }

    pub const fn right(header: &'static str, width: f32) -> Self {
        Self {
            header,
            width,
            align_right: true,
        }
    }
}

/// A cell: its text and, optionally, its own colour (else the row's).
#[derive(Clone)]
pub struct Cell {
    pub text: SharedString,
    pub color: Option<Hsla>,
}

impl<T: Into<SharedString>> From<T> for Cell {
    fn from(text: T) -> Self {
        Self {
            text: text.into(),
            color: None,
        }
    }
}

/// A row: an optional swatch, then one cell per column.
#[derive(Clone)]
pub struct Row {
    pub swatch: Option<Hsla>,
    pub cells: Vec<Cell>,
}

/// A table overlay's content. Build one in the overlay's `render` and turn it
/// into the card with [`OverlayTable::render`].
pub struct OverlayTable {
    id: ElementId,
    title: SharedString,
    badge: Option<(SharedString, Hsla)>,
    columns: &'static [Column],
    swatches: bool,
    rows: Rc<[Row]>,
    empty: SharedString,
    footer: Vec<AnyElement>,
}

impl OverlayTable {
    pub fn new(
        id: impl Into<ElementId>,
        title: impl Into<SharedString>,
        columns: &'static [Column],
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            badge: None,
            columns,
            swatches: false,
            rows: Rc::from([]),
            empty: "Waiting for the renderer".into(),
            footer: Vec::new(),
        }
    }

    /// A status shown at the right of the title.
    pub fn badge(mut self, text: impl Into<SharedString>, color: Hsla) -> Self {
        self.badge = Some((text.into(), color));
        self
    }

    /// The rows; `swatches` leaves room for each row's colour swatch.
    pub fn rows(mut self, rows: impl Into<Rc<[Row]>>, swatches: bool) -> Self {
        self.rows = rows.into();
        self.swatches = swatches;
        self
    }

    /// What the body says with no rows.
    pub fn empty(mut self, text: impl Into<SharedString>) -> Self {
        self.empty = text.into();
        self
    }

    /// Anything under the rows (totals, notes), after a divider.
    pub fn footer(mut self, element: impl IntoElement) -> Self {
        self.footer.push(element.into_any_element());
        self
    }

    pub fn render(self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let (background, border, foreground, muted) = (
            theme.background,
            theme.border,
            theme.foreground,
            theme.muted_foreground,
        );
        let columns = self.columns;
        let swatches = self.swatches;
        let header = h_flex()
            .w_full()
            .items_center()
            .when(swatches, |row| {
                row.child(div().w(px(SWATCH_WIDTH)).flex_none())
            })
            .children(columns.iter().map(|column| {
                cell_box(column)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child(column.header)
            }));
        let count = self.rows.len();
        let body = if count == 0 {
            div()
                .text_xs()
                .text_color(muted)
                .child(self.empty)
                .into_any_element()
        } else {
            let rows = self.rows.clone();
            let height = (count as f32 * ROW_HEIGHT).min(MAX_LIST_HEIGHT);
            div()
                .w_full()
                .h(px(height))
                .occlude()
                .child(
                    uniform_list(self.id.clone(), count, move |range, _, _| {
                        range
                            .filter_map(|index| rows.get(index))
                            .map(|row| render_row(row, columns, swatches, muted, foreground))
                            .collect()
                    })
                    .size_full(),
                )
                .into_any_element()
        };
        v_flex()
            .gap_2()
            .p_3()
            .w(px(CARD_WIDTH))
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
                            .child(self.title),
                    )
                    .when_some(self.badge, |row, (text, color)| {
                        row.child(div().text_xs().text_color(color).child(text))
                    }),
            )
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(v_flex().gap_1().child(header).child(body))
            .when(!self.footer.is_empty(), |card| {
                card.child(div().w_full().h(px(1.0)).bg(border))
                    .children(self.footer)
            })
            .into_any_element()
    }
}

/// A column's cell box: its width, alignment and the table's text size.
fn cell_box(column: &Column) -> Div {
    div()
        .w(px(column.width))
        .flex_none()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_xs()
        .when(column.align_right, |cell| cell.text_right())
}

fn render_row(
    row: &Row,
    columns: &'static [Column],
    swatches: bool,
    muted: Hsla,
    foreground: Hsla,
) -> Div {
    h_flex()
        .w_full()
        .h(px(ROW_HEIGHT))
        .items_center()
        .when(swatches, |line| {
            line.child(
                div().w(px(SWATCH_WIDTH)).flex_none().when_some(row.swatch, |slot, color| {
                    slot.child(div().w(px(8.0)).h(px(8.0)).rounded(px(2.0)).bg(color))
                }),
            )
        })
        .children(columns.iter().zip(&row.cells).enumerate().map(|(i, (column, cell))| {
            // The first column names the row (muted); the others are values.
            let base = if i == 0 { muted } else { foreground };
            cell_box(column)
                .line_height(relative(1.0))
                .text_color(cell.color.unwrap_or(base))
                .child(cell.text.clone())
        }))
}

/// A footer line with one cell per column, in the table's layout (totals).
pub fn footer_row(
    columns: &'static [Column],
    swatches: bool,
    cells: Vec<Cell>,
    color: Hsla,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_center()
        .when(swatches, |row| {
            row.child(div().w(px(SWATCH_WIDTH)).flex_none())
        })
        .children(columns.iter().zip(cells).map(move |(column, cell)| {
            cell_box(column)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cell.color.unwrap_or(color))
                .child(cell.text)
        }))
}

/// A muted note line under a table.
pub fn footer_note(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}
