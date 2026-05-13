use anyhow::Result;
use gpui::{Context, Entity, EntityInputHandler, Task, Window};
use lsp_types::{request::Completion, CompletionContext, CompletionItem, CompletionResponse};
use ropey::Rope;
use std::{cell::RefCell, ops::Range, rc::Rc};

use crate::input::{
    popovers::{CompletionMenu, ContextMenu},
    InputState, RopeExt,
};

/// A trait for providing code completions based on the current input state and context.
pub trait CompletionProvider {
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

        let existing_menu = match self.context_menu.as_ref() {
            Some(ContextMenu::Completion(menu)) => Some(menu.clone()),
            _ => None,
        };

        let menu = existing_menu.clone().unwrap_or_else(|| {
            let new_menu = CompletionMenu::new(cx.entity(), window, cx);
            self.context_menu = Some(ContextMenu::Completion(new_menu.clone()));
            new_menu
        });

        // Build the prefix the user has typed so far (walk back from cursor to word start).
        // word_at/word_range spans the whole word; we only want chars LEFT of the cursor.
        let (word_start, query) = {
            let mut q = String::new();
            for c in self.text.chars_at(new_offset).reversed() {
                if c.is_alphanumeric() || c == '_' {
                    q.insert(0, c);
                } else {
                    break;
                }
            }
            let ws = new_offset.saturating_sub(q.len());
            (ws, q)
        };
        println!("[FILTER] handle_completion_trigger: cursor={}, word_start={}, query='{}', text.len()={}",
            new_offset, word_start, query, self.text.len());
        tracing::info!("🎯 handle_completion_trigger: cursor={}, word_start={}, query='{}', text.len()={}", new_offset, word_start, query, self.text.len());
        // Instantly re-filter whatever items the menu already has.
        menu.update(cx, |menu, cx| {
            tracing::info!("📢 Calling menu.apply_query with query='{}', trigger_start={}", query, word_start);
            menu.apply_query(word_start, &query, cx);
        });

        self.request_completions_now(
            new_offset,
            start,
            new_text,
            provider,
            self.text.clone(),
            Some(menu),
            window,
            cx,
        );
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
        let menu = existing_menu.unwrap_or_else(|| {
            let new_menu = CompletionMenu::new(cx.entity(), window, cx);
            self.context_menu = Some(ContextMenu::Completion(new_menu.clone()));
            new_menu
        });

        // Mark as loading (keeps existing filtered items visible).
        menu.update(cx, |menu, cx| {
            menu.show_loading(new_offset, cx);
        });

        let completion_context = CompletionContext {
            trigger_kind: lsp_types::CompletionTriggerKind::INVOKED,
            trigger_character: None,
        };

        let request_id = self.completion_request_id.wrapping_add(1);
        self.completion_request_id = request_id;

        let provider_responses =
            provider.completions(&text, new_offset, completion_context, window, cx);

        self._context_menu_task = cx.spawn_in(window, async move |editor, cx| {
            let response = provider_responses.await;

            editor
                .update_in(cx, |editor, window, cx| {
                    if editor.completion_request_id != request_id {
                        tracing::debug!(
                            "Ignoring stale completion response: request_id={} current={}",
                            request_id,
                            editor.completion_request_id
                        );
                        return;
                    }

                    if !editor.focus_handle.is_focused(window) {
                        return;
                    }

                    let mut completions: Vec<CompletionItem> = vec![];

                    match response {
                        Ok(provider_responses) => match provider_responses {
                            CompletionResponse::Array(items) => {
                                tracing::info!("📦 Received {} completions (Array)", items.len());
                                completions.extend(items);
                            }
                            CompletionResponse::List(list) => {
                                tracing::info!(
                                    "📦 Received {} completions (isIncomplete: {})",
                                    list.items.len(),
                                    list.is_incomplete
                                );
                                completions.extend(list.items);
                            }
                        },
                        Err(e) => {
                            tracing::error!("❌ Error getting completions: {:?}", e);
                            _ = menu.update(cx, |menu, cx| {
                                menu.hide(cx);
                            });
                            return;
                        }
                    }

                    if completions.is_empty() {
                        tracing::warn!("❌ No completions - hiding menu");
                        _ = menu.update(cx, |menu, cx| {
                            menu.hide(cx);
                            cx.notify();
                        });
                        return;
                    }

                    tracing::info!("✅ Showing {} completions from server", completions.len());

                    _ = menu.update(cx, |menu, cx| {
                        menu.show(new_offset, completions, window, cx);
                    });

                    cx.notify();
                })
                .ok();

            Ok(())
        });
    }
}
