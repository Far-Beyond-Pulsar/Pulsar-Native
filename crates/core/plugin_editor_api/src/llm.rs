//! The language model editors can talk to.
//!
//! The agent chat panel owns provider setup (keys, sign-in, model choice).
//! Editors that want a one-shot completion of their own -- inline edits,
//! explain, quick fixes -- must not duplicate that, so the panel publishes
//! whatever provider and model the user has selected here and editors read it
//! back. Nothing is published until a provider is ready; callers show their
//! own "set up a provider in the chat panel" message then.
//!
//! The service is process-wide, which is right for the built-in editors that
//! link this crate once. A plugin loaded as a separate library has its own
//! copy of the static and sees no service.

use std::sync::{Arc, RwLock};

/// Who said a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmRole {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct LlmMessage {
    pub role: LlmRole,
    pub content: String,
}

impl LlmMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: LlmRole::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: LlmRole::Assistant,
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LlmRequest {
    pub system: String,
    pub messages: Vec<LlmMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

/// A model that completes text. Calls block, so run them off the UI thread.
pub trait EditorLlm: Send + Sync {
    /// Names the model for the UI ("Claude Sonnet via Anthropic").
    fn label(&self) -> String;

    /// Runs `request`, handing each streamed piece of the answer to
    /// `on_chunk` as it arrives, and returns the whole answer. Providers
    /// that cannot stream deliver everything in one chunk.
    fn complete(
        &self,
        request: LlmRequest,
        on_chunk: &mut dyn FnMut(&str),
    ) -> Result<String, String>;
}

fn slot() -> &'static RwLock<Option<Arc<dyn EditorLlm>>> {
    static SLOT: RwLock<Option<Arc<dyn EditorLlm>>> = RwLock::new(None);
    &SLOT
}

/// Publishes `llm` as the model editors use, or withdraws it with `None`.
pub fn set_editor_llm(llm: Option<Arc<dyn EditorLlm>>) {
    if let Ok(mut guard) = slot().write() {
        *guard = llm;
    }
}

/// The model editors should use now, if a provider is ready.
pub fn editor_llm() -> Option<Arc<dyn EditorLlm>> {
    slot().read().ok().and_then(|guard| guard.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    impl EditorLlm for Echo {
        fn label(&self) -> String {
            "echo".into()
        }

        fn complete(
            &self,
            request: LlmRequest,
            on_chunk: &mut dyn FnMut(&str),
        ) -> Result<String, String> {
            let text = request
                .messages
                .last()
                .map(|message| message.content.clone())
                .unwrap_or_default();
            on_chunk(&text);
            Ok(text)
        }
    }

    #[test]
    fn a_published_model_is_found_and_can_be_withdrawn() {
        set_editor_llm(Some(Arc::new(Echo)));
        let llm = editor_llm().expect("published");
        let mut seen = String::new();
        let answer = llm
            .complete(
                LlmRequest {
                    messages: vec![LlmMessage::user("hi")],
                    ..Default::default()
                },
                &mut |chunk| seen.push_str(chunk),
            )
            .unwrap();
        assert_eq!((answer.as_str(), seen.as_str()), ("hi", "hi"));
        set_editor_llm(None);
        assert!(editor_llm().is_none());
    }
}
