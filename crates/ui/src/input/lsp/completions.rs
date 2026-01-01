use anyhow::Result;
use gpui::{Context, Entity, EntityInputHandler, Task, Window};
use lsp_types::{request::Completion, CompletionContext, CompletionItem, CompletionResponse};
use ropey::Rope;
use std::{cell::RefCell, ops::Range, rc::Rc};

use crate::input::{
    popovers::{CompletionMenu, ContextMenu},
    InputState,
};

/// A trait for providing code completions based on the current input state and context.
pub trait CompletionProvider {
    /// Fetches completions based on the given byte offset.
    ///
    /// - The `offset` is in bytes of current cursor.
    ///
    /// textDocument/completion
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_completion
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        window: &mut Window,
        cx: &mut Context<InputState>,
    ) -> Task<Result<CompletionResponse>>;

    fn resolve_completions(
        &self,
        _completion_indices: Vec<usize>,
        _completions: Rc<RefCell<Box<[Completion]>>>,
        _: &mut Context<InputState>,
    ) -> Task<Result<bool>> {
        Task::ready(Ok(false))
    }

    /// Determines if the completion should be triggered based on the given byte offset.
    ///
    /// This is called on the main thread.
    fn is_completion_trigger(
        &self,
        offset: usize,
        new_text: &str,
        cx: &mut Context<InputState>,
    ) -> bool;
}

impl InputState {
    pub(crate) fn handle_completion_trigger(
        &mut self,
        range: &Range<usize>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.completion_inserting {
            return;
        }

        let Some(provider) = self.lsp.completion_provider.clone() else {
            return;
        };

        let start = range.end;
        let new_offset = self.cursor();

        // VSCode behavior: Request on EVERY character that could be part of completion
        // Let rust-analyzer decide what to return
        let should_trigger = provider.is_completion_trigger(start, new_text, cx);

        if !should_trigger {
            // Not a valid completion character - close menu if open
            if let Some(ContextMenu::Completion(menu)) = self.context_menu.as_ref() {
                if menu.read(cx).is_open() {
                    menu.update(cx, |menu, cx| {
                        menu.hide(cx);
                    });
                }
            }
            return;
        }

        // Get or create menu
        let existing_menu = match self.context_menu.as_ref() {
            Some(ContextMenu::Completion(menu)) => Some(menu.clone()),
            _ => None,
        };

        // Determine completion context
        let last_char = new_text.chars().last().unwrap_or(' ');
        
        // ALWAYS request new completions from server on every keystroke
        // The server does all filtering, sorting, and prioritization
        
        self.request_completions_now(new_offset, start, new_text, provider, self.text.clone(), existing_menu, window, cx);
    }
    
    fn request_completions_now(
        &mut self,
        new_offset: usize,
        start: usize,
        new_text: &str,
        provider: Rc<dyn CompletionProvider>,
        text: Rope,
        existing_menu: Option<Entity<CompletionMenu>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Create or get the menu
        let menu = existing_menu.unwrap_or_else(|| {
            let new_menu = CompletionMenu::new(cx.entity(), window, cx);
            self.context_menu = Some(ContextMenu::Completion(new_menu.clone()));
            new_menu
        });

        // Show loading state immediately (non-blocking UI)
        menu.update(cx, |menu, cx| {
            menu.show_loading(new_offset, cx);
        });

        // Determine trigger kind based on what was typed
        let last_char = new_text.chars().last();
        let (trigger_kind, trigger_char) = if last_char.map_or(false, |c| matches!(c, '.' | ':' | '<' | '(')) {
            (lsp_types::CompletionTriggerKind::TRIGGER_CHARACTER, last_char.map(|c| c.to_string()))
        } else {
            (lsp_types::CompletionTriggerKind::INVOKED, None)
        };
        
        let completion_context = CompletionContext {
            trigger_kind,
            trigger_character: trigger_char,
        };

        // Request completions from LSP server (non-blocking!)
        let provider_responses =
            provider.completions(&text, new_offset, completion_context, window, cx);
            
        // Handle response asynchronously - UI stays responsive
        self._context_menu_task = cx.spawn_in(window, async move |editor, cx| {
            let mut completions: Vec<CompletionItem> = vec![];
            
            match provider_responses.await {
                Ok(provider_responses) => {
                    match provider_responses {
                        CompletionResponse::Array(items) => {
                            tracing::info!("📦 Received {} completions (Array)", items.len());
                            completions.extend(items);
                        },
                        CompletionResponse::List(list) => {
                            tracing::info!("📦 Received {} completions (isIncomplete: {})", 
                                list.items.len(), 
                                list.is_incomplete
                            );
                            completions.extend(list.items);
                        },
                    }
                },
                Err(e) => {
                    tracing::error!("❌ Error getting completions: {:?}", e);
                    _ = menu.update(cx, |menu, cx| {
                        menu.hide(cx);
                    });
                    return Ok(());
                }
            }

            if completions.is_empty() {
                tracing::warn!("❌ No completions - hiding menu");
                _ = menu.update(cx, |menu, cx| {
                    menu.hide(cx);
                    cx.notify();
                });
                return Ok(());
            }

            tracing::info!("✅ Showing {} completions from server", completions.len());

            editor
                .update_in(cx, |editor, window, cx| {
                    if !editor.focus_handle.is_focused(window) {
                        return;
                    }

                    _ = menu.update(cx, |menu, cx| {
                        // Show completions exactly as received from rust-analyzer
                        // Server did all filtering, sorting, and prioritization
                        menu.show(new_offset, completions, window, cx);
                    });

                    cx.notify();
                })
                .ok();

            Ok(())
        });
    }
}
