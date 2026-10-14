//! Deterministic mock provider for evals and replay.
//!
//! [`MockProvider`] answers from a script of [`ScriptedStep`]s (or from a
//! handler closure) and records every [`ChatRequest`] it receives, so a test
//! can assert on exactly what the runtime sent.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use agent_chat_core::{ChatProvider, ChatRequest, ChatResponse, ModelDescriptor, ToolCall};

/// One scripted model response.
#[derive(Clone, Debug, Default)]
pub struct ScriptedStep {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

impl ScriptedStep {
    pub fn say(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tool_calls: Vec::new(),
        }
    }

    pub fn call(name: &str, id: &str, arguments: serde_json::Value) -> Self {
        Self {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments_json: arguments,
            }],
        }
    }

    pub fn into_response(self) -> ChatResponse {
        ChatResponse {
            assistant_message: (!self.text.is_empty()).then_some(self.text),
            streamed_text_chunks: Vec::new(),
            tool_calls: self.tool_calls,
            finish_reason: None,
            raw_response: serde_json::Value::Null,
        }
    }
}

/// Handler form: gets the request and its zero-based index.
pub type Handler = Box<dyn Fn(&ChatRequest, usize) -> anyhow::Result<ChatResponse> + Send + Sync>;

enum Mode {
    Script(Mutex<VecDeque<ScriptedStep>>),
    Handler(Handler),
}

/// A provider that never touches the network.
pub struct MockProvider {
    mode: Mode,
    requests: Mutex<Vec<ChatRequest>>,
}

impl MockProvider {
    /// Pops one step per request; errors once the script is exhausted.
    pub fn scripted(steps: Vec<ScriptedStep>) -> Arc<Self> {
        Arc::new(Self {
            mode: Mode::Script(Mutex::new(steps.into())),
            requests: Mutex::new(Vec::new()),
        })
    }

    /// Answers every request through `handler`.
    pub fn from_fn(
        handler: impl Fn(&ChatRequest, usize) -> anyhow::Result<ChatResponse> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            mode: Mode::Handler(Box::new(handler)),
            requests: Mutex::new(Vec::new()),
        })
    }

    /// Replies with the same text forever.
    pub fn constant(text: &str) -> Arc<Self> {
        let text = text.to_string();
        Self::from_fn(move |_, _| Ok(ScriptedStep::say(text.clone()).into_response()))
    }

    pub fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    /// Scripted steps not yet consumed (0 for handler providers).
    pub fn remaining(&self) -> usize {
        match &self.mode {
            Mode::Script(steps) => steps.lock().unwrap().len(),
            Mode::Handler(_) => 0,
        }
    }
}

impl ChatProvider for MockProvider {
    fn id(&self) -> &str {
        "mock"
    }

    fn display_name(&self) -> &str {
        "Mock"
    }

    fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        Ok(Vec::new())
    }

    fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request.clone());
            requests.len() - 1
        };
        match &self.mode {
            Mode::Script(steps) => steps
                .lock()
                .unwrap()
                .pop_front()
                .map(ScriptedStep::into_response)
                .ok_or_else(|| anyhow::anyhow!("mock script exhausted at request {index}")),
            Mode::Handler(handler) => handler(&request, index),
        }
    }
}
