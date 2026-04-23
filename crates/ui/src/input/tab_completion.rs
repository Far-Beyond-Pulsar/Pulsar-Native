/// Tab completion handler that manages tab-based completion cycling
/// Allows users to press Tab to cycle through completions
use gpui::{actions, Context, Window};
use lsp_types::CompletionItem;
use std::rc::Rc;

use crate::input::InputState;

actions!(
    tab_completion,
    [
        TabComplete,
        TabCompleteReverse,
        AcceptCompletion,
        CancelCompletion,
    ]
);

/// Tab completion state for cycling through suggestions
pub struct TabCompletionState {
    /// Current completion items
    pub items: Vec<Rc<CompletionItem>>,
    /// Currently selected item index
    pub selected_index: usize,
    /// Whether tab completion is active
    pub active: bool,
    /// The starting offset of the word being completed
    pub start_offset: usize,
    /// The original text before completion started
    pub original_text: String,
}

impl TabCompletionState {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            selected_index: 0,
            active: false,
            start_offset: 0,
            original_text: String::new(),
        }
    }

    /// Start tab completion with the given items
    pub fn start(
        &mut self,
        items: Vec<CompletionItem>,
        start_offset: usize,
        original_text: String,
    ) {
        self.items = items.into_iter().map(Rc::new).collect();
        self.selected_index = 0;
        self.active = !self.items.is_empty();
        self.start_offset = start_offset;
        self.original_text = original_text;
    }

    /// Cycle to the next completion
    pub fn next(&mut self) {
        if !self.items.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.items.len();
        }
    }

    /// Cycle to the previous completion
    pub fn previous(&mut self) {
        if !self.items.is_empty() {
            if self.selected_index == 0 {
                self.selected_index = self.items.len() - 1;
            } else {
                self.selected_index -= 1;
            }
        }
    }

    /// Get the currently selected completion item
    pub fn current_item(&self) -> Option<&Rc<CompletionItem>> {
        self.items.get(self.selected_index)
    }

    /// Reset the tab completion state
    pub fn reset(&mut self) {
        self.items.clear();
        self.selected_index = 0;
        self.active = false;
        self.start_offset = 0;
        self.original_text.clear();
    }

    /// Check if tab completion is active
    pub fn is_active(&self) -> bool {
        self.active && !self.items.is_empty()
    }
}

impl InputState {
    /// Initialize tab completion keybindings
    pub fn init_tab_completion(cx: &mut gpui::App) {
        cx.bind_keys([
            gpui::KeyBinding::new("tab", TabComplete, Some("Input")),
            gpui::KeyBinding::new("shift-tab", TabCompleteReverse, Some("Input")),
        ]);
    }

    /// Handle tab completion action
    pub fn handle_tab_complete(
        &mut self,
        _action: &TabComplete,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // If context menu is open, accept the selected completion and DON'T insert tab
        if self.is_context_menu_open(cx) {
            self.handle_action_for_context_menu(
                Box::new(super::Enter { secondary: false }),
                window,
                cx,
            );
            return; // Exit early - don't insert tab
        }

        // Otherwise, trigger completions
        // Get current cursor position
        let cursor = self.cursor();

        // Get completion provider
        let Some(provider) = self.lsp.completion_provider.clone() else {
            // No provider - insert tab normally
            self.insert("\t", window, cx);
            return;
        };

        // Trigger completions
        let trigger = lsp_types::CompletionContext {
            trigger_kind: lsp_types::CompletionTriggerKind::INVOKED,
            trigger_character: None,
        };

        let cursor_copy = cursor; // Make a copy to move into closure
        let completion_task = provider.completions(&self.text, cursor, trigger, window, cx);

        // Process completions asynchronously
        let editor_task: gpui::Task<anyhow::Result<()>> =
            cx.spawn_in(window, async move |editor, cx| {
                if let Ok(response) = completion_task.await {
                    let items = match response {
                        lsp_types::CompletionResponse::Array(items) => items,
                        lsp_types::CompletionResponse::List(list) => list.items,
                    };

                    if !items.is_empty() {
                        editor
                            .update_in(cx, |editor, window, cx| {
                                // Show completion menu with items
                                editor.handle_completion_trigger(
                                    &(cursor_copy..cursor_copy),
                                    "",
                                    window,
                                    cx,
                                );
                                cx.notify();
                            })
                            .ok();
                    }
                }
                Ok(())
            });

        editor_task.detach();

        cx.notify();
    }

    /// Handle reverse tab completion action
    pub fn handle_tab_complete_reverse(
        &mut self,
        _action: &TabCompleteReverse,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // If context menu is open, navigate backwards
        if self.is_context_menu_open(cx) {
            let handled = self.handle_action_for_context_menu(Box::new(super::MoveUp), window, cx);
            if handled {
                return;
            }
        }

        cx.notify();
    }
}

/// Extension trait for InputState to add tab completion functionality
pub trait TabCompletionExt {
    /// Trigger tab completion at the current cursor position
    fn trigger_tab_completion(&mut self, window: &mut Window, cx: &mut Context<InputState>);

    /// Cycle to the next tab completion
    fn cycle_tab_completion_next(&mut self, window: &mut Window, cx: &mut Context<InputState>);

    /// Cycle to the previous tab completion
    fn cycle_tab_completion_prev(&mut self, window: &mut Window, cx: &mut Context<InputState>);

    /// Accept the current tab completion
    fn accept_tab_completion(&mut self, window: &mut Window, cx: &mut Context<InputState>);

    /// Cancel tab completion and restore original text
    fn cancel_tab_completion(&mut self, window: &mut Window, cx: &mut Context<InputState>);
}

impl TabCompletionExt for InputState {
    fn trigger_tab_completion(&mut self, window: &mut Window, cx: &mut Context<InputState>) {
        let cursor = self.cursor();
        self.handle_completion_trigger(&(cursor..cursor), "", window, cx);
    }

    fn cycle_tab_completion_next(&mut self, window: &mut Window, cx: &mut Context<InputState>) {
        let handled = self.handle_action_for_context_menu(Box::new(super::MoveDown), window, cx);
        if !handled {
            self.trigger_tab_completion(window, cx);
        }
    }

    fn cycle_tab_completion_prev(&mut self, window: &mut Window, cx: &mut Context<InputState>) {
        let handled = self.handle_action_for_context_menu(Box::new(super::MoveUp), window, cx);
        if !handled {
            self.trigger_tab_completion(window, cx);
        }
    }

    fn accept_tab_completion(&mut self, window: &mut Window, cx: &mut Context<InputState>) {
        self.handle_action_for_context_menu(
            Box::new(super::Enter { secondary: false }),
            window,
            cx,
        );
    }

    fn cancel_tab_completion(&mut self, window: &mut Window, cx: &mut Context<InputState>) {
        self.handle_action_for_context_menu(Box::new(super::Escape), window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tab_completion_state() {
        let mut state = TabCompletionState::new();
        assert!(!state.is_active());

        let items = vec![
            CompletionItem {
                label: "test1".to_string(),
                ..Default::default()
            },
            CompletionItem {
                label: "test2".to_string(),
                ..Default::default()
            },
        ];

        state.start(items, 0, "te".to_string());
        assert!(state.is_active());
        assert_eq!(state.selected_index, 0);

        state.next();
        assert_eq!(state.selected_index, 1);

        state.next();
        assert_eq!(state.selected_index, 0); // Wraps around

        state.previous();
        assert_eq!(state.selected_index, 1); // Wraps around backwards
    }

    #[test]
    fn test_tab_completion_reset() {
        let mut state = TabCompletionState::new();
        state.start(
            vec![CompletionItem {
                label: "test".to_string(),
                ..Default::default()
            }],
            0,
            "t".to_string(),
        );

        assert!(state.is_active());
        state.reset();
        assert!(!state.is_active());
        assert!(state.items.is_empty());
    }
}
