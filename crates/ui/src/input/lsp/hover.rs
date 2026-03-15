use anyhow::Result;
use gpui::{App, Context, MouseMoveEvent, Point, Pixels, Task, Window};
use ropey::Rope;

use crate::input::{popovers::HoverPopover, InputState, RopeExt};

// The HoverProvider trait lives in pulsar_lsp to avoid circular dependencies.
pub use pulsar_lsp::traits::HoverProvider;

impl InputState {
    /// Handle hover trigger LSP request.
    /// Creates popover immediately, requests LSP instantly, shows after delay.
    pub(super) fn handle_hover_popover(
        &mut self,
        offset: usize,
        mouse_position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<InputState>,
    ) {
        if self.selecting {
            return;
        }

        let Some(provider) = self.lsp.hover_provider.clone() else {
            return;
        };

        // Check if we already have a hover popover for this location
        if let Some(hover_popover) = self.hover_popover.as_ref() {
            if hover_popover.read(cx).is_same(offset) {
                return;
            }
            
            // Check if mouse is inside the current hover popover
            if hover_popover.read(cx).contains_point(mouse_position) {
                // Don't hide if mouse is in the popover
                return;
            }
        }

        // Clear any existing hover popover when moving to a new location
        self.hover_popover = None;

        // Create popover IMMEDIATELY (invisible, will show after delay)
        let symbol_range = self.text.word_range(offset).unwrap_or(offset..offset);
        let hover_popover = HoverPopover::new(cx.entity(), symbol_range.clone(), mouse_position, cx);
        self.hover_popover = Some(hover_popover.clone());

        // Request hover info from LSP IMMEDIATELY (async, non-blocking)
        let text = self.text.clone();
        let task = provider.hover(&text, offset, window, cx);
        let editor = cx.entity();
        
        self.lsp._hover_task = cx.spawn_in(window, async move |_, cx| {
            // LSP request is already in flight, just wait for it
            let result = task.await?;
            
            // Process the result and set hover data
            _ = editor.update(cx, |editor, cx| {
                match result {
                    Some(hover) => {
                        let mut updated_range = symbol_range;
                        
                        if let Some(range) = hover.range {
                            let start = text.position_to_offset(&range.start);
                            let end = text.position_to_offset(&range.end);
                            updated_range = start..end;
                        }
                        
                        // Update the hover popover with the data
                        if let Some(popover_entity) = &editor.hover_popover {
                            _ = popover_entity.update(cx, |popover, cx| {
                                popover.symbol_range = updated_range;
                                popover.set_hover(hover, cx);
                            });
                        }
                        
                        cx.notify();
                    }
                    None => {
                        // No hover data, remove the popover
                        editor.hover_popover = None;
                    }
                }
            });

            Ok(())
        });
    }
}
