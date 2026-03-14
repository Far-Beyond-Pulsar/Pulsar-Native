//! A text input field that allows the user to enter text.
//!
//! Based on the `Input` example from the `gpui` crate.
//! https://github.com/zed-industries/zed/blob/main/crates/gpui/examples/input.rs
use anyhow::Result;
use gpui::{
    actions, div, point, prelude::FluentBuilder as _, px, Action, App, AppContext, Bounds,
    ClipboardItem, Context, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render, ScrollHandle,
    ScrollWheelEvent, SharedString, Styled as _, Subscription, Task, UTF16Selection, Window,
};
use ropey::{Rope, RopeSlice};
use serde::Deserialize;
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use gpui_sum_tree::Bias;
use unicode_segmentation::*;

use super::{
    blink_cursor::BlinkCursor,
    change::Change,
    element::TextElement,
    mask_pattern::MaskPattern,
    mode::{InputMode, TabSize},
    number_input,
    text_wrapper::TextWrapper,
};
use crate::input::{
    element::RIGHT_MARGIN,
    popovers::{ContextMenu, DiagnosticPopover, HoverPopover, MouseContextMenu},
    search::{self, SearchPanel},
    text_wrapper::LineLayout,
    HoverDefinition, Lsp, Position,
};
use crate::input::{RopeExt as _, Selection};
use crate::{highlighter::DiagnosticSet, input::text_wrapper::LineItem};
use crate::{history::History, scroll::ScrollbarState, Root};
use super::core::InputState;

impl InputState {
    pub(in crate::input) fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // If there have IME marked range and is empty (Means pressed Esc to abort IME typing)
        // Clear the marked range.
        if let Some(ime_marked_range) = &self.ime_marked_range {
            if ime_marked_range.len() == 0 {
                self.ime_marked_range = None;
            }
        }

        self.selecting = true;
        let offset = self.index_for_mouse_position(event.position);

        if self.handle_click_hover_definition(event, offset, window, cx) {
            return;
        }

        // Double click to select word
        if event.button == MouseButton::Left && event.click_count == 2 {
            self.select_word(offset, window, cx);
            return;
        }

        // Show Mouse context menu
        if event.button == MouseButton::Right {
            self.handle_right_click_menu(event, offset, window, cx);
            return;
        }

        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx)
        }
    }

    pub(in crate::input) fn on_mouse_up(
        &mut self,
        _: &MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.selecting = false;
        self.selected_word_range = None;
    }

    pub(in crate::input) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Show diagnostic popover on mouse move
        let offset = self.index_for_mouse_position(event.position);
        self.handle_mouse_move(offset, event, window, cx);

        if self.mode.is_code_editor() {
            if let Some(diagnostic) = self
                .mode
                .diagnostics()
                .and_then(|set| set.for_offset(offset))
            {
                if let Some(diagnostic_popover) = self.diagnostic_popover.as_ref() {
                    if diagnostic_popover.read(cx).diagnostic.range == diagnostic.range {
                        diagnostic_popover.update(cx, |this, cx| {
                            this.show(cx);
                        });

                        return;
                    }
                }

                self.diagnostic_popover = Some(DiagnosticPopover::new(diagnostic, cx.entity(), cx));
                cx.notify();
            } else {
                if let Some(diagnostic_popover) = self.diagnostic_popover.as_mut() {
                    diagnostic_popover.update(cx, |this, cx| {
                        this.check_to_hide(event.position, cx);
                    })
                }
            }
        }
    }

    pub(in crate::input) fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let line_height = self
            .last_layout
            .as_ref()
            .map(|layout| layout.line_height)
            .unwrap_or(window.line_height());

        // If shift is held and soft_wrap is off, scroll horizontally
        if event.modifiers.shift && !self.soft_wrap {
            let delta = event.delta.pixel_delta(line_height);
            let mut offset = self.scroll_handle.offset();
            // Swap y and x for horizontal scroll
            offset.x += delta.y;
            self.update_scroll_offset(Some(offset), cx);
        } else {
            let delta = event.delta.pixel_delta(line_height);
            self.update_scroll_offset(Some(self.scroll_handle.offset() + delta), cx);
        }
        self.diagnostic_popover = None;
    }

    fn update_scroll_offset(&mut self, offset: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let mut offset = offset.unwrap_or(self.scroll_handle.offset());

        let safe_y_range =
            (-self.scroll_size.height + self.input_bounds.size.height).min(px(0.0))..px(0.);
        let safe_x_range =
            (-self.scroll_size.width + self.input_bounds.size.width).min(px(0.0))..px(0.);

        offset.y = if self.mode.is_single_line() {
            px(0.)
        } else {
            offset.y.clamp(safe_y_range.start, safe_y_range.end)
        };
        offset.x = offset.x.clamp(safe_x_range.start, safe_x_range.end);
        self.scroll_handle.set_offset(offset);
        cx.notify();
    }

    pub(crate) fn scroll_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let Some(last_layout) = self.last_layout.as_ref() else {
            return;
        };
        let Some(bounds) = self.last_bounds.as_ref() else {
            return;
        };

        let mut scroll_offset = self.scroll_handle.offset();
        let line_height = last_layout.line_height;

        let point = self.text.offset_to_point(offset);
        let row = point.row;

        let mut row_offset_y = px(0.);
        for (ix, wrap_line) in self.text_wrapper.lines.iter().enumerate() {
            if ix == row {
                break;
            }

            row_offset_y += wrap_line.height(line_height);
        }

        if let Some(line) = last_layout
            .lines
            .get(row.saturating_sub(last_layout.visible_range.start))
        {
            // Check to scroll horizontally
            if let Some(pos) = line.position_for_index(point.column, line_height) {
                let bounds_width = bounds.size.width - last_layout.line_number_width;
                let col_offset_x = pos.x;
                if col_offset_x - RIGHT_MARGIN < -scroll_offset.x {
                    // If the position is out of the visible area, scroll to make it visible
                    scroll_offset.x = -col_offset_x + RIGHT_MARGIN;
                } else if col_offset_x + RIGHT_MARGIN > -scroll_offset.x + bounds_width {
                    scroll_offset.x = -(col_offset_x - bounds_width + RIGHT_MARGIN);
                }
            }
        }

        // Check if row_offset_y is out of the viewport
        // If row offset is not in the viewport, scroll to make it visible
        let edge_height = 3 * line_height;
        if row_offset_y - edge_height < -scroll_offset.y {
            // Scroll up
            scroll_offset.y = -row_offset_y + edge_height;
        } else if row_offset_y + edge_height > -scroll_offset.y + bounds.size.height {
            // Scroll down
            scroll_offset.y = -(row_offset_y - bounds.size.height + edge_height);
        }

        scroll_offset.x = scroll_offset.x.min(px(0.));
        scroll_offset.y = scroll_offset.y.min(px(0.));
        self.deferred_scroll_offset = Some(scroll_offset);
        cx.notify();
    }
    pub(crate) fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        // If the text is empty, always return 0
        if self.text.len() == 0 {
            return 0;
        }

        let (Some(_bounds), Some(last_layout)) =
            (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };

        let line_height = last_layout.line_height;
        let line_number_width = last_layout.line_number_width;

        // TIP: About the IBeam cursor
        //
        // If cursor style is IBeam, the mouse position is in the middle of the cursor (This is special in OS)

        // IMPORTANT: Convert from window coordinates to element coordinates
        // position is in window coordinates
        // input_bounds.origin is the element's fixed position in window space (no scroll)
        // bounds.origin includes the scroll offset applied during rendering
        
        // The mouse position relative to the element (not scrolled)
        let element_relative_position = position - self.input_bounds.origin - point(line_number_width, px(0.));
        
        // Now we need to find which line this corresponds to
        // The visible_top is already relative to the scroll, so we work in "content space"
        // where y=0 is the start of the content (before any scrolling)
        
        // Get the scroll offset to convert element coords to content coords
        let scroll_y = self.scroll_handle.offset().y;
        
        // Content-relative position (where y=0 is top of first line)
        // When scrolled down, scroll_y is negative, so subtracting it adds the scroll amount
        let content_y = element_relative_position.y - scroll_y;

        let mut index = last_layout.visible_range_offset.start;
        // y_offset tracks position in content space (starts at 0 for first visible line in content)
        let mut y_offset = px(0.);
        
        // Start from the first visible line and check each one
        for (ix, line) in self
            .text_wrapper
            .lines
            .iter()
            .skip(last_layout.visible_range.start)
            .enumerate()
        {
            let line_origin = self.line_origin_with_y_offset(&mut y_offset, line, line_height);
            
            // Adjust for visible_top offset
            let line_y_in_content = last_layout.visible_top + line_origin.y;
            let relative_y = content_y - line_y_in_content;
            let pos = point(element_relative_position.x - line_origin.x, relative_y);

            let Some(line_layout) = last_layout.lines.get(ix) else {
                if relative_y < line_height {
                    break;
                }

                continue;
            };

            // Return offset by use closest_index_for_x if is single line mode.
            if self.mode.is_single_line() {
                return line_layout.closest_index_for_x(pos.x);
            }

            if let Some(v) = line_layout.closest_index_for_position(pos, line_height) {
                index += v;
                break;
            } else if relative_y < px(0.) {
                break;
            }

            // +1 for `\n`
            index += line_layout.len() + 1;
        }

        if index > self.text.len() {
            self.text.len()
        } else {
            index
        }
    }

    /// Returns a y offsetted point for the line origin.
    fn line_origin_with_y_offset(
        &self,
        y_offset: &mut Pixels,
        line: &LineItem,
        line_height: Pixels,
    ) -> Point<Pixels> {
        // NOTE: About line.wrap_boundaries.len()
        //
        // If only 1 line, the value is 0
        // If have 2 line, the value is 1
        if self.mode.is_multi_line() {
            let p = point(px(0.), *y_offset);
            *y_offset += line.height(line_height);
            p
        } else {
            point(px(0.), px(0.))
        }
    }

    /// Select the text from the current cursor position to the given offset.
    ///
    /// The offset is the UTF-8 offset.
    ///
    /// Ensure the offset use self.next_boundary or self.previous_boundary to get the correct offset.
    pub(crate) fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = offset.clamp(0, self.text.len());
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };

        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = (self.selected_range.end..self.selected_range.start).into();
        }

        // Ensure keep word selected range
        if let Some(word_range) = self.selected_word_range.as_ref() {
            if self.selected_range.start > word_range.start {
                self.selected_range.start = word_range.start;
            }
            if self.selected_range.end < word_range.end {
                self.selected_range.end = word_range.end;
            }
        }
        if self.selected_range.is_empty() {
            self.update_preferred_column();
        }
        cx.notify()
    }

    /// Select the word at the given offset.
    ///
    /// The offset is the UTF-8 offset.
    ///
    /// FIXME: When click on a non-word character, the word is not selected.
    fn select_word(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        #[inline(always)]
        fn is_word(c: char) -> bool {
            c.is_alphanumeric() || matches!(c, '_')
        }

        let mut start = offset;
        let mut end = start;
        let prev_text = self
            .text_for_range(self.range_to_utf16(&(0..start)), &mut None, window, cx)
            .unwrap_or_default();
        let next_text = self
            .text_for_range(
                self.range_to_utf16(&(end..self.text.len())),
                &mut None,
                window,
                cx,
            )
            .unwrap_or_default();

        let prev_chars = prev_text.chars().rev();
        let next_chars = next_text.chars();

        let pre_chars_count = prev_chars.clone().count();
        for (ix, c) in prev_chars.enumerate() {
            if !is_word(c) {
                break;
            }

            if ix < pre_chars_count {
                start = start.saturating_sub(c.len_utf8());
            }
        }

        for (_, c) in next_chars.enumerate() {
            if !is_word(c) {
                break;
            }

            end += c.len_utf8();
        }

        if start == end {
            return;
        }

        self.selected_range = (start..end).into();
        self.selected_word_range = Some(self.selected_range);
        cx.notify()
    }

    /// Unselects the currently selected text.
    pub fn unselect(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.cursor();
        self.selected_range = (offset..offset).into();
        cx.notify()
    }
}