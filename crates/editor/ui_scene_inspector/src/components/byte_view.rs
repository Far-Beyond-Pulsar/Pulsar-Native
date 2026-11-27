//! The hex and binary viewer.
//!
//! One grid for both: an offset gutter, one cell per byte (two hex digits,
//! or eight bits in two nibbles), and the bytes as text. Bytes are tinted by
//! the field they belong to, named in the legend beside the grid; the footer
//! reads the bytes under the cursor as numbers. It shows one entity's value
//! or, one page at a time, that component of every entity that has it, each
//! starting its own row. Only the rows on screen are built.

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::TextInput,
    v_flex, ActiveTheme, Disableable as _, IconName, Sizable as _,
};

use crate::components::widgets::{caption, chip, empty_state, region_color, MONO};
use crate::screen::{ByteScope, DataTab, SceneInspectorView};
use crate::utils::detail::ComponentData;
use crate::utils::format;
use crate::utils::hex::{self, HexLayout, HexRow, Region};

const ROW_HEIGHT: f32 = 22.0;
const TEXT_SIZE: f32 = 12.0;
const HEX_CELL: f32 = 23.0;
const BINARY_CELL: f32 = 78.0;
const ASCII_CELL: f32 = 9.0;
/// Space before every eighth byte and, narrower, every fourth.
const GAP_8: f32 = 12.0;
const GAP_4: f32 = 5.0;
/// The binary view's hex column, per byte.
const ASIDE_HEX_CELL: f32 = 20.0;
const SEPARATOR: f32 = 18.0;
const VALUE_GUTTER: f32 = 92.0;
const COLUMN_GUTTER: f32 = 190.0;
const LEGEND_WIDTH: f32 = 230.0;

/// The grid's measures.
#[derive(Clone, Copy)]
struct Grid {
    binary: bool,
    column: bool,
    per_row: usize,
}

impl Grid {
    fn cell(self) -> f32 {
        if self.binary {
            BINARY_CELL
        } else {
            HEX_CELL
        }
    }

    fn gutter(self) -> f32 {
        if self.column {
            COLUMN_GUTTER
        } else {
            VALUE_GUTTER
        }
    }

    /// Space before the byte at column `at`.
    fn gap(self, at: usize) -> f32 {
        match at {
            0 => 0.0,
            at if at % 8 == 0 => GAP_8,
            at if at % 4 == 0 => GAP_4,
            _ => 0.0,
        }
    }

    fn width(self) -> f32 {
        let gaps: f32 = (0..self.per_row).map(|at| self.gap(at)).sum();
        let aside = if self.binary {
            SEPARATOR + ASIDE_HEX_CELL * self.per_row as f32
        } else {
            0.0
        };
        self.gutter()
            + self.cell() * self.per_row as f32
            + gaps
            + aside
            + SEPARATOR
            + ASCII_CELL * self.per_row as f32
            + 24.0
    }
}

impl SceneInspectorView {
    pub(crate) fn render_bytes(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(data) = self.data.clone() else {
            return div().into_any_element();
        };
        let theme = cx.theme().clone();
        let layout = self.layout();
        let grid = Grid {
            binary: self.tab == DataTab::Binary,
            column: self.bytes.scope == ByteScope::Column,
            per_row: layout.per_row,
        };

        let body = if data.bytes.is_empty() {
            empty_state(
                IconName::Database,
                "Zero bytes",
                "A marker component: its presence is the whole value.",
                cx,
            )
        } else if layout.rows() == 0 {
            empty_state(IconName::Database, "Reading the column…", "", cx)
        } else {
            self.render_grid(&data, layout, grid, cx)
        };

        let legend = (!data.regions.is_empty()).then(|| self.render_legend(&data, cx));
        let toolbar = self.byte_toolbar(&data, cx);
        let footer = self.byte_footer(&data, cx);

        v_flex()
            .size_full()
            .child(toolbar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        v_flex()
                            .id("byte-grid-keys")
                            .track_focus(&self.bytes_focus)
                            .on_key_down(cx.listener(Self::on_bytes_key))
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .bg(theme.background)
                            .child(body),
                    )
                    .children(legend),
            )
            .child(footer)
            .into_any_element()
    }

    fn byte_toolbar(&self, data: &ComponentData, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let scope = self.bytes.scope;
        let column_label = match self
            .index
            .as_ref()
            .and_then(|index| index.type_row(data.target.id))
        {
            Some(row) => format!("All {} entities (column)", format::count(row.count)),
            None => "All entities (column)".to_string(),
        };
        let widths: &[usize] = if self.tab == DataTab::Binary {
            &[4, 8]
        } else {
            &[8, 16, 32]
        };
        let per_row = self.per_row();
        let width_chips: Vec<AnyElement> = widths
            .iter()
            .map(|&width| {
                chip(
                    ("bytes-per-row", width),
                    width.to_string(),
                    per_row == width,
                    cx,
                    move |this, cx| this.set_per_row(width, cx),
                )
                .into_any_element()
            })
            .collect();
        let pages = self.page_count();
        let page = self.bytes.page;

        h_flex()
            .w_full()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_3()
            .py_1p5()
            .border_b_1()
            .border_color(theme.border)
            .child(chip(
                "scope-value",
                "This entity",
                scope == ByteScope::Value,
                cx,
                |this, cx| this.set_scope(ByteScope::Value, cx),
            ))
            .child(chip(
                "scope-column",
                column_label,
                scope == ByteScope::Column,
                cx,
                |this, cx| this.set_scope(ByteScope::Column, cx),
            ))
            .child(div().w(px(8.0)))
            .child(caption("Bytes per row", cx))
            .children(width_chips)
            .child(div().w(px(8.0)))
            .child(
                div()
                    .w(px(130.0))
                    .child(TextInput::new(&self.bytes.goto).small()),
            )
            .when(scope == ByteScope::Column && pages > 1, |bar| {
                bar.child(
                    Button::new("page-previous")
                        .ghost()
                        .xsmall()
                        .icon(IconName::ChevronLeft)
                        .disabled(page == 0)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_page(page.saturating_sub(1), cx)
                        })),
                )
                .child(caption(format!("Page {} of {}", page + 1, pages), cx))
                .child(
                    Button::new("page-next")
                        .ghost()
                        .xsmall()
                        .icon(IconName::ChevronRight)
                        .disabled(page + 1 >= pages)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.set_page(page + 1, cx)),
                        ),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("copy-selection")
                    .ghost()
                    .xsmall()
                    .label("Selection")
                    .icon(IconName::Copy)
                    .tooltip("Copy the selected bytes as hex (Ctrl+C)")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let text = hex::hex_string(&this.selected_bytes());
                        this.copy(text, "the selected bytes as hex", cx)
                    })),
            )
            .into_any_element()
    }

    fn render_grid(
        &mut self,
        data: &ComponentData,
        layout: HexLayout,
        grid: Grid,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();

        let heading = |text: String, width: f32| {
            div().flex_none().w(px(width)).text_center().child(text)
        };
        let column_heads = |cell: f32, gaps: bool| {
            (0..grid.per_row).map(move |at| {
                heading(format!("{at:02x}"), cell).ml(px(if gaps { grid.gap(at) } else { 0.0 }))
            })
        };
        let header = h_flex()
            .flex_none()
            .h(px(ROW_HEIGHT + 2.0))
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .text_color(theme.muted_foreground)
            .child(
                div()
                    .flex_none()
                    .w(px(grid.gutter()))
                    .px_2()
                    .child(if grid.column { "Entity · offset" } else { "Offset" }),
            )
            .children(column_heads(grid.cell(), true))
            .when(grid.binary, |header| {
                header
                    .child(div().flex_none().w(px(SEPARATOR)))
                    .child(
                        div()
                            .flex_none()
                            .w(px(ASIDE_HEX_CELL * grid.per_row as f32))
                            .child("Hex"),
                    )
            })
            .child(div().flex_none().w(px(SEPARATOR)))
            .child(div().flex_none().child("Text"));

        let regions: std::rc::Rc<[Region]> = data.regions.clone().into();
        let list = uniform_list(
            "scene-bytes",
            layout.rows(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|position| {
                        let row = layout.row(position)?;
                        Some(this.byte_row(layout, grid, row, &regions, cx))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .min_h_0()
        .w_full()
        .track_scroll(&self.bytes.scroll);

        div()
            .relative()
            .size_full()
            .font_family(MONO)
            .text_size(px(TEXT_SIZE))
            .child(
                // A narrow pane scrolls sideways; the rows never wrap.
                div()
                    .id("byte-grid-sideways")
                    .size_full()
                    .overflow_x_scroll()
                    .child(
                        v_flex()
                            .h_full()
                            .min_w(px(grid.width()))
                            .w_full()
                            .child(header)
                            .child(list),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top(px(ROW_HEIGHT + 2.0))
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .child(ui::scroll::Scrollbar::uniform_scroll(
                        &self.bytes.scrollbar,
                        &self.bytes.scroll,
                    )),
            )
            .into_any_element()
    }

    fn byte_row(
        &self,
        layout: HexLayout,
        grid: Grid,
        row: HexRow,
        regions: &[Region],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let start = row.record * layout.record_len + row.offset;
        let (first, last) = self.bytes.selection();
        let cursor = self.bytes.cursor;

        // How one byte is painted, wherever in the row it appears.
        let paint = |index: usize, offset: usize| {
            let byte = self.byte(index);
            let region = hex::region_at(regions, offset);
            let selected = (first..=last).contains(&index);
            let background = if selected {
                Some(theme.primary.opacity(0.45))
            } else if self.changed_bytes.contains_key(&index) {
                Some(theme.warning.opacity(0.45))
            } else {
                region.map(|region| region_color(region).opacity(0.17))
            };
            // Zeros recede; so do bytes no field names, in a type with named
            // fields (padding, or what its type does not describe).
            let color = match byte {
                None => theme.muted_foreground.opacity(0.4),
                Some(0) => theme.muted_foreground.opacity(0.55),
                Some(_) if region.is_none() && !regions.is_empty() => theme.muted_foreground,
                Some(_) => theme.foreground,
            };
            (byte, background, color)
        };

        // A clickable, drag-selectable cell.
        let cell = |index: usize,
                    width: f32,
                    background: Option<Hsla>,
                    color: Hsla,
                    text: String,
                    cx: &mut Context<Self>| {
            div()
                .flex_none()
                .w(px(width))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(if index == cursor {
                    theme.foreground
                } else {
                    transparent_black()
                })
                .when_some(background, |cell, background| cell.bg(background))
                .text_color(color)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        window.focus(&this.bytes_focus, cx);
                        this.place_cursor(index, event.modifiers.shift, cx);
                    }),
                )
                .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                    if event.pressed_button == Some(MouseButton::Left)
                        && this.bytes.cursor != index
                    {
                        this.place_cursor(index, true, cx);
                    }
                }))
                .child(text)
        };

        let offset_text = format!("{:08x}", row.offset);
        let gutter = if grid.column {
            let entity = (row.offset == 0)
                .then(|| self.page_entities().get(row.record).copied())
                .flatten();
            let label = entity.map(|entity| {
                let name = self
                    .index
                    .as_ref()
                    .and_then(|index| index.entity(entity))
                    .map_or_else(String::new, |row| row.label.to_string());
                (entity, name)
            });
            h_flex()
                .flex_none()
                .w(px(grid.gutter()))
                .h_full()
                .px_2()
                .gap_1()
                .overflow_hidden()
                .when_some(label, |gutter, (entity, name)| {
                    gutter
                        .child(
                            div()
                                .id(("byte-record", row.record))
                                .flex_none()
                                .cursor_pointer()
                                .text_color(theme.link)
                                .child(format::entity(entity))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.follow_link(entity, cx)
                                })),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.foreground)
                                .child(name),
                        )
                })
                .when(row.offset != 0, |gutter| gutter.child(div().flex_1()))
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.muted_foreground)
                        .child(format!("+{:04x}", row.offset)),
                )
        } else {
            h_flex()
                .flex_none()
                .w(px(grid.gutter()))
                .h_full()
                .px_2()
                .text_color(theme.muted_foreground)
                .child(offset_text)
        };

        let mut line = h_flex()
            .w_full()
            .h(px(ROW_HEIGHT))
            .items_center()
            .when(grid.column && row.offset == 0 && row.record > 0, |line| {
                line.border_t_1().border_color(theme.border)
            })
            .child(gutter);

        for at in 0..grid.per_row {
            let gap = grid.gap(at);
            if at >= row.len {
                line = line.child(div().flex_none().w(px(grid.cell())).ml(px(gap)));
                continue;
            }
            let index = start + at;
            let (byte, background, color) = paint(index, row.offset + at);
            let text = match byte {
                Some(byte) if grid.binary => format!("{:04b} {:04b}", byte >> 4, byte & 0x0f),
                Some(byte) => format!("{byte:02x}"),
                None if grid.binary => "···· ····".to_string(),
                None => "··".to_string(),
            };
            line = line.child(cell(index, grid.cell(), background, color, text, cx).ml(px(gap)));
        }

        if grid.binary {
            line = line.child(div().flex_none().w(px(SEPARATOR)));
            for at in 0..grid.per_row {
                if at >= row.len {
                    line = line.child(div().flex_none().w(px(ASIDE_HEX_CELL)));
                    continue;
                }
                let index = start + at;
                let (byte, background, color) = paint(index, row.offset + at);
                let text = byte.map_or_else(|| "··".to_string(), |byte| format!("{byte:02x}"));
                line = line.child(cell(index, ASIDE_HEX_CELL, background, color, text, cx));
            }
        }

        line = line.child(div().flex_none().w(px(SEPARATOR)));
        for at in 0..row.len {
            let index = start + at;
            let (byte, background, color) = paint(index, row.offset + at);
            let text = byte.map_or('·', hex::printable).to_string();
            line = line.child(cell(index, ASCII_CELL, background, color, text, cx));
        }
        line.into_any_element()
    }

    fn render_legend(&self, data: &ComponentData, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let regions: std::rc::Rc<[Region]> = data.regions.clone().into();
        let named: usize = regions.iter().map(|region| region.len).sum();
        let unnamed = data.bytes.len().saturating_sub(named);
        let count = regions.len();
        let list = crate::components::widgets::virtual_list(
            "scene-byte-legend",
            count,
            &self.bytes.legend_scroll,
            cx,
            move |this, range, cx| {
                let theme = cx.theme().clone();
                let record_len = this.record_len().max(1);
                let record = this.bytes.cursor / record_len;
                let under = hex::region_at(&regions, this.bytes.cursor % record_len);
                range
                    .filter_map(|ix| {
                        let region = regions.get(ix)?;
                        let (offset, len) = (region.offset, region.len);
                        Some(
                            v_flex()
                                .id(("byte-region", ix))
                                .w_full()
                                .h(px(36.0))
                                .px_2()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|el| el.bg(theme.list_hover))
                                .when(under == Some(ix), |el| el.bg(theme.list_active))
                                .child(
                                    h_flex()
                                        .gap_1p5()
                                        .items_center()
                                        .child(
                                            div()
                                                .flex_none()
                                                .size(px(9.0))
                                                .rounded(px(2.0))
                                                .bg(region_color(ix)),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_xs()
                                                .text_color(theme.foreground)
                                                .child(region.name.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .pl(px(15.0))
                                        .truncate()
                                        .font_family(MONO)
                                        .text_size(px(10.5))
                                        .text_color(theme.muted_foreground)
                                        .child(format!(
                                            "+{offset:#04x} · {len} B · {}",
                                            region.type_label
                                        )),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    window.focus(&this.bytes_focus, cx);
                                    this.select_bytes(record * record_len + offset, len, cx);
                                }))
                                .into_any_element(),
                        )
                    })
                    .collect()
            },
        );
        v_flex()
            .flex_none()
            .w(px(LEGEND_WIDTH))
            .h_full()
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child("Fields in these bytes"),
            )
            .child(list)
            .when(unnamed > 0, |legend| {
                legend.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "{unnamed} B untinted: padding, or not described by the type"
                        )),
                )
            })
            .into_any_element()
    }

    fn byte_footer(&self, data: &ComponentData, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let record_len = data.bytes.len();
        let footer = v_flex()
            .flex_none()
            .w_full()
            .gap_1()
            .px_3()
            .py_1p5()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .text_xs();
        if record_len == 0 {
            return footer.into_any_element();
        }
        let cursor = self.bytes.cursor;
        let offset = cursor % record_len;
        let (first, last) = self.bytes.selection();
        // Numbers are read from the cursor to the end of its record.
        let ahead: Vec<u8> = (cursor..cursor + (record_len - offset).min(8))
            .map_while(|index| self.byte(index))
            .collect();
        let mut place = format!("Offset {offset:#06x} ({offset})");
        if let Some(region) = hex::region_at(&data.regions, offset) {
            let region = &data.regions[region];
            place.push_str(&format!(
                " · {}: {} +{}",
                region.name,
                region.type_label,
                offset - region.offset
            ));
        }
        if last > first {
            place.push_str(&format!(" · {} bytes selected", last - first + 1));
        }
        let big_endian = self.bytes.big_endian;

        let readings: Vec<AnyElement> = hex::interpret(&ahead, big_endian)
            .into_iter()
            .enumerate()
            .map(|(ix, (label, value))| {
                let copied = value.clone();
                h_flex()
                    .id(("reading", ix))
                    .flex_none()
                    .gap_1()
                    .px_1p5()
                    .rounded(px(4.0))
                    .bg(theme.muted.opacity(0.25))
                    .cursor_pointer()
                    .hover(|el| el.bg(theme.list_hover))
                    .child(div().text_color(theme.muted_foreground).child(label))
                    .child(
                        div()
                            .font_family(MONO)
                            .text_color(theme.foreground)
                            .child(value),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.copy(copied.clone(), label, cx)
                    }))
                    .into_any_element()
            })
            .collect();

        footer
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().min_w_0().truncate().child(place))
                    .child(chip(
                        "little-endian",
                        "Little endian",
                        !big_endian,
                        cx,
                        |this, cx| {
                            this.bytes.big_endian = false;
                            cx.notify();
                        },
                    ))
                    .child(chip(
                        "big-endian",
                        "Big endian",
                        big_endian,
                        cx,
                        |this, cx| {
                            this.bytes.big_endian = true;
                            cx.notify();
                        },
                    )),
            )
            .child(h_flex().flex_wrap().gap_1().children(readings))
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(
                        "These are the value's bytes as stored in its column. Heap data (a \
                         String's text, a Vec's items) appears only as pointer, length and \
                         capacity.",
                    ),
            )
            .into_any_element()
    }
}
