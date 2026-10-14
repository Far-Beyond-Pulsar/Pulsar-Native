//! Panel side of speculative context prefetch (`agent_runtime::prefetch`).
//!
//! While the user types, and only when the setting is on, a debounced local
//! lookup (file names, definitions, mentions; no model, no tokens) fills a
//! "Suggested context" tray with a few files. Clicking one inserts an
//! `@file` reference into the draft, which `expand_file_references` resolves
//! on send. Nothing is attached by itself.

use super::*;
use agent_runtime::prefetch::{
    referenced_paths, suggest, PrefetchSettings, Suggestion, MAX_SUGGESTIONS,
};
use smol::Timer;
use std::time::Duration;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, ActiveTheme as _, Sizable as _,
};

/// How long typing must pause before the lookup runs.
const DEBOUNCE: Duration = Duration::from_millis(450);
/// Drafts shorter than this are not worth searching for.
const MIN_DRAFT_CHARS: usize = 4;

pub struct PrefetchState {
    pub settings: PrefetchSettings,
    /// Bumped on every keystroke; a lookup only lands if it is still current.
    generation: u64,
    pub suggestions: Vec<Suggestion>,
}

impl PrefetchState {
    pub fn load() -> Self {
        Self {
            settings: engine_state::get_project_path()
                .map(|root| PrefetchSettings::load(&PathBuf::from(root)))
                .unwrap_or_default(),
            generation: 0,
            suggestions: Vec::new(),
        }
    }
}

impl AgentChatPanel {
    /// The draft changed (wired to the prompt input's change event).
    pub(super) fn on_prompt_changed(&mut self, cx: &mut Context<Self>) {
        self.prefetch.generation += 1;
        let generation = self.prefetch.generation;
        let draft = self.prompt_input.read(cx).text().to_string();
        if !self.prefetch.settings.enabled || draft.trim().chars().count() < MIN_DRAFT_CHARS {
            if !self.prefetch.suggestions.is_empty() {
                self.prefetch.suggestions.clear();
                cx.notify();
            }
            return;
        }
        let Some(root) = engine_state::get_project_path().map(PathBuf::from) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            Timer::after(DEBOUNCE).await;
            let current = cx.update(|cx| {
                this.update(cx, |panel, _| panel.prefetch.generation == generation)
                    .unwrap_or(false)
            });
            if !current {
                return;
            }
            let found = cx
                .background_executor()
                .spawn(async move {
                    let exclude = referenced_paths(&draft);
                    suggest(&root, &draft, &exclude, MAX_SUGGESTIONS)
                })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |panel, cx| {
                    if panel.prefetch.generation == generation {
                        panel.prefetch.suggestions = found;
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    fn toggle_prefetch(&mut self, cx: &mut Context<Self>) {
        self.prefetch.settings.enabled = !self.prefetch.settings.enabled;
        if let Some(root) = engine_state::get_project_path() {
            if let Err(err) = self.prefetch.settings.save(&PathBuf::from(root)) {
                tracing::warn!("[agent_chat] could not save agent_prefetch.json: {err}");
            }
        }
        self.prefetch.generation += 1;
        if self.prefetch.settings.enabled {
            self.on_prompt_changed(cx);
        } else {
            self.prefetch.suggestions.clear();
        }
        cx.notify();
    }

    /// Insert the suggestion as an `@file` reference the user can see and edit.
    fn attach_suggestion(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.prefetch.suggestions.len() {
            return;
        }
        let suggestion = self.prefetch.suggestions.remove(index);
        let reference = suggestion.reference();
        let mut text = self.prompt_input.read(cx).text().to_string();
        if !text.is_empty() && !text.ends_with(char::is_whitespace) {
            text.push(' ');
        }
        text.push_str(&reference);
        text.push(' ');
        self.prompt_input
            .update(cx, |input, cx| input.set_value(&text, window, cx));
        cx.notify();
    }

    pub(super) fn render_prefetch_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.prefetch.settings.enabled;
        let button = Button::new("agent-chat-prefetch")
            .xsmall()
            .label(if on { "Suggest context: on" } else { "Suggest context: off" })
            .tooltip(
                "While you type, look for related files in the project (no model call, no tokens) \
                 and offer them as clickable @file references. Nothing is attached automatically.",
            )
            .on_click(cx.listener(|this, _, _, cx| this.toggle_prefetch(cx)));
        if on {
            button.primary()
        } else {
            button.ghost()
        }
    }

    /// The tray above the input, or `None` when there is nothing to suggest.
    pub(super) fn render_prefetch_tray(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.prefetch.settings.enabled || self.prefetch.suggestions.is_empty() {
            return None;
        }
        let mut tray = h_flex()
            .w_full()
            .flex_wrap()
            .gap_1()
            .items_center()
            .pb_1()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Suggested context:"),
            );
        for (i, suggestion) in self.prefetch.suggestions.iter().enumerate() {
            tray = tray.child(
                Button::new(SharedString::from(format!("agent-chat-suggest-{i}")))
                    .xsmall()
                    .ghost()
                    .label(suggestion.label())
                    .tooltip(SharedString::from(format!(
                        "{} ({}). Click to attach as {}",
                        suggestion.reason,
                        suggestion.path,
                        suggestion.reference()
                    )))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.attach_suggestion(i, window, cx)
                    })),
            );
        }
        Some(tray)
    }
}
