//! The byte viewer's cursor, selection and keyboard.

use gpui::{Context, KeyDownEvent, ScrollStrategy, Window};

use crate::screen::{ByteScope, DataTab, SceneInspectorView};
use crate::utils::hex;

impl SceneInspectorView {
    /// Put the cursor on `index`; `extend` keeps the other end of the
    /// selection.
    pub(crate) fn place_cursor(&mut self, index: usize, extend: bool, cx: &mut Context<Self>) {
        self.bytes.cursor = index.min(self.layout().len().saturating_sub(1));
        if !extend {
            self.bytes.anchor = self.bytes.cursor;
        }
        cx.notify();
    }

    pub(crate) fn select_bytes(&mut self, start: usize, len: usize, cx: &mut Context<Self>) {
        self.bytes.anchor = start;
        self.bytes.cursor = start + len.saturating_sub(1);
        self.clamp_cursor();
        self.scroll_to_byte(self.bytes.anchor);
        cx.notify();
    }

    pub(crate) fn scroll_to_byte(&mut self, index: usize) {
        let row = self.layout().row_of(index);
        self.bytes
            .scroll
            .scroll_to_item(row, ScrollStrategy::Center);
    }

    pub(crate) fn set_scope(&mut self, scope: ByteScope, cx: &mut Context<Self>) {
        if self.bytes.scope == scope {
            return;
        }
        // Keep the cursor on the same byte of the same entity.
        let within = self
            .bytes
            .cursor
            .checked_rem(self.record_len())
            .unwrap_or(0);
        self.bytes.scope = scope;
        self.bytes.page = 0;
        self.bytes.page_data = None;
        self.changed_bytes.clear();
        self.rebuild_column();
        let record = match (scope, self.data.as_ref()) {
            (ByteScope::Column, Some(data)) => {
                let position = self
                    .bytes
                    .column
                    .iter()
                    .position(|&entity| entity == data.target.entity)
                    .unwrap_or(0);
                self.bytes.page = position / self.records_per_page();
                position % self.records_per_page()
            }
            _ => 0,
        };
        let index = record * self.record_len() + within;
        self.bytes.cursor = index;
        self.bytes.anchor = index;
        self.clamp_cursor();
        self.scroll_to_byte(self.bytes.cursor);
        self.request_refresh(false, cx);
        cx.notify();
    }

    pub(crate) fn set_per_row(&mut self, per_row: usize, cx: &mut Context<Self>) {
        // The page boundary moves with the row width: keep the same entity.
        let first = self.bytes.page * self.records_per_page();
        match self.tab {
            DataTab::Binary => self.bytes.binary_per_row = per_row,
            _ => self.bytes.hex_per_row = per_row,
        }
        self.bytes.page = (first / self.records_per_page()).min(self.page_count() - 1);
        self.clamp_cursor();
        self.scroll_to_byte(self.bytes.cursor);
        self.request_refresh(false, cx);
        cx.notify();
    }

    pub(crate) fn set_tab(&mut self, tab: DataTab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        let first = self.bytes.page * self.records_per_page();
        self.tab = tab;
        self.bytes.page = (first / self.records_per_page()).min(self.page_count() - 1);
        self.clamp_cursor();
        self.request_refresh(false, cx);
        cx.notify();
    }

    pub(crate) fn set_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let page = page.min(self.page_count() - 1);
        if page != self.bytes.page {
            self.bytes.page = page;
            self.bytes.cursor = 0;
            self.bytes.anchor = 0;
            self.changed_bytes.clear();
            self.bytes.scroll.scroll_to_item(0, ScrollStrategy::Top);
            self.request_refresh(false, cx);
            cx.notify();
        }
    }

    /// Jump to an offset: within the value, or in the column counted from
    /// its first entity's first byte, across pages.
    pub(crate) fn go_to_offset(&mut self, text: &str, cx: &mut Context<Self>) {
        let stride = self.record_len();
        let total = match self.bytes.scope {
            ByteScope::Value => stride,
            ByteScope::Column => stride * self.bytes.column.len(),
        };
        let Some(offset) = hex::parse_offset(text).filter(|&offset| offset < total) else {
            self.notify_user(format!(
                "Enter an offset below {total:#x} (31, 0x1f or 1fh)"
            ));
            cx.notify();
            return;
        };
        let mut local = offset;
        if self.bytes.scope == ByteScope::Column {
            let per_page = self.records_per_page();
            let record = offset / stride;
            self.set_page(record / per_page, cx);
            local = (record % per_page) * stride + offset % stride;
        }
        self.place_cursor(local, false, cx);
        self.scroll_to_byte(local);
    }

    /// The selected bytes that could be read, in order.
    pub(crate) fn selected_bytes(&self) -> Vec<u8> {
        let (start, end) = self.bytes.selection();
        (start..=end).filter_map(|index| self.byte(index)).collect()
    }

    pub(crate) fn on_bytes_key(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let layout = self.layout();
        if layout.len() == 0 {
            return;
        }
        let cursor = self.bytes.cursor;
        let modifiers = &event.keystroke.modifiers;
        let command = modifiers.control || modifiers.platform;
        let next = match event.keystroke.key.as_str() {
            "left" => layout.step(cursor, -1),
            "right" => layout.step(cursor, 1),
            "up" => layout.step_rows(cursor, -1),
            "down" => layout.step_rows(cursor, 1),
            "pageup" => layout.step_rows(cursor, -24),
            "pagedown" => layout.step_rows(cursor, 24),
            "home" if command => 0,
            "end" if command => layout.len() - 1,
            "home" => layout.row_span(cursor).0,
            "end" => layout.row_span(cursor).1,
            "c" if command => {
                let text = hex::hex_string(&self.selected_bytes());
                self.copy(text, "the selected bytes as hex", cx);
                cx.stop_propagation();
                return;
            }
            "a" if command => {
                cx.stop_propagation();
                self.select_bytes(0, layout.len(), cx);
                return;
            }
            _ => return,
        };
        cx.stop_propagation();
        self.place_cursor(next, modifiers.shift, cx);
        self.scroll_to_byte(next);
    }
}
