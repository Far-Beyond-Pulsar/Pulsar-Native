use agent_chat_core::{
    AuthHost, AuthMethod, AuthResult, ChatProvider, ChatRequest, ChatResponse, ModelDescriptor,
    ProviderAvailability, ProviderEnvironment, ProviderKind, ProviderMetadata,
};
use anyhow::anyhow;
use serde_json::json;
use std::{
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub struct DemoRandomProvider;

impl DemoRandomProvider {
    pub fn new() -> Self {
        Self
    }

    fn static_models() -> Vec<ModelDescriptor> {
        vec![
            ModelDescriptor {
                id: "demo-breeze",
                label: "Demo Breeze",
                supports_tools: false,
            },
            ModelDescriptor {
                id: "demo-story",
                label: "Demo Story",
                supports_tools: false,
            },
            ModelDescriptor {
                id: "demo-chaos",
                label: "Demo Chaos",
                supports_tools: false,
            },
        ]
    }

    fn sentence_pool() -> &'static [&'static str] {
        &[
            "When evaluating the lifecycle of a provider bridge, it is vital to consider how the internal state machine transitions from a 'connecting' status to a 'streaming' status, especially when the underlying transport layer is being mocked. This specific mock implementation bypasses the standard handshake protocol to deliver immediate feedback to the UI components, which is essential for rapid prototyping of the editor's primary viewport and ensuring the layout remains responsive under heavy text loads.",
            "The architectural decoupling we've achieved here allows the frontend to remain blissfully unaware of whether it is communicating with a multi-billion parameter model running on a high-end GPU cluster or a simple static array within a Rust binary. By simulating the asynchronous nature of a real-world inference stream, we can verify that our reactive hooks—specifically those responsible for managing the message buffer—are correctly handling append operations without introducing unnecessary re-renders that would degrade the user experience.",
            "Let's take a moment to analyze the signal flow: once the provider receives the request, it triggers a non-deterministic selection process to mimic the probabilistic nature of generative AI. This provides a diverse range of text lengths and complexities, allowing us to stress-test the CSS Grid or Flexbox containers that house the chat bubbles, ensuring that even the most verbose responses don't break the layout or cause awkward horizontal scrolling on smaller viewport dimensions.",
            "A robust testing suite requires more than just 'Hello World' snippets; it needs substantial blocks of prose to validate that the markdown parser is behaving as expected when encountering nested structures. While this current provider is intentionally lightweight, the plan is to eventually integrate a local WASM-based tokenizer that can provide real-time token counts, giving the developer a clear picture of the overhead involved in every round-trip before they ever commit to a paid API tier.",
            "We noticed earlier that the panel state would occasionally flicker during high-frequency updates, but by switching to this stable provider bridge, the data flow has smoothed out significantly. This allows us to focus on the more nuanced aspects of the UX, such as the subtle animations of the typing indicator and the way the scroll-to-bottom logic interacts with a user who is manually trying to read a previous response while a new, extremely long paragraph is being streamed into the view.",
            "Consider the implications of using this synthetic provider for offline development: you can continue to refine the prompt engineering and UI logic in environments with zero connectivity, such as during a flight or in a remote workspace. Because these responses are pre-compiled into the binary, there is zero latency, which provides an idealized 'best-case scenario' benchmark that we can use to compare against the inevitable jitter and latency spikes of a production network environment.",
            "If the goal is to simulate a truly complex reasoning chain, we might want to extend this pool to include even more technical jargon and pseudo-code snippets. This ensures that the syntax highlighter doesn't choke on unexpected character sequences and that the typography remains legible even when the model decides to dump five hundred words of architectural advice directly into the primary interaction thread without any prior warning or formatting."
        ]
    }

    fn pseudo_random_seed(request: &ChatRequest) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        let mut hash = 1469598103934665603u64;
        for ch in request.model.bytes() {
            hash ^= ch as u64;
            hash = hash.wrapping_mul(1099511628211);
        }
        for msg in &request.messages {
            for ch in msg.content.bytes() {
                hash ^= ch as u64;
                hash = hash.wrapping_mul(1099511628211);
            }
        }

        now ^ hash.rotate_left(13)
    }

    fn sentence_count_for_model(model: &str) -> usize {
        match model {
            "demo-breeze" => 2,
            "demo-story" => 4,
            "demo-chaos" => 6,
            _ => 3,
        }
    }

    fn chunk_text(mut seed: u64, text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut chunks = Vec::new();
        let mut cursor = 0usize;

        while cursor < chars.len() {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let chunk_len = 3 + ((seed as usize) % 14);
            let end = (cursor + chunk_len).min(chars.len());
            chunks.push(chars[cursor..end].iter().collect::<String>());
            cursor = end;
        }

        chunks
    }
}

impl Default for DemoRandomProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatProvider for DemoRandomProvider {
    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            id: "demo_random",
            display_name: "Demo Random",
            endpoint: "local://demo-random",
            kind: ProviderKind::Local,
        }
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        Self::static_models()
    }

    fn availability(&self, _env: &dyn ProviderEnvironment) -> ProviderAvailability {
        ProviderAvailability::ready()
    }

    fn auth_methods(&self) -> Vec<AuthMethod> {
        Vec::new()
    }

    fn authenticate(
        &self,
        _method: AuthMethod,
        _host: &mut dyn AuthHost,
    ) -> anyhow::Result<AuthResult> {
        Err(anyhow!("Demo provider does not require authentication"))
    }

    fn list_models_api(&self, _token: &str) -> anyhow::Result<Vec<ModelDescriptor>> {
        Ok(Self::static_models())
    }

    fn chat_completion(&self, _token: &str, request: &ChatRequest) -> anyhow::Result<ChatResponse> {
        let pool = Self::sentence_pool();
        let mut seed = Self::pseudo_random_seed(request);
        let mut parts = Vec::new();
        let count = Self::sentence_count_for_model(&request.model);

        for _ in 0..count {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let ix = (seed as usize) % pool.len();
            parts.push(pool[ix]);
        }

        let full_text = parts.join(" ");
        let assistant_message = Some(full_text.clone());
        let streamed_text_chunks = Self::chunk_text(seed, &full_text);

        Ok(ChatResponse {
            assistant_message,
            streamed_text_chunks,
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_string()),
            raw_response: json!({
                "provider": "demo_random",
                "model": request.model,
                "mode": "synthetic_random_sentences"
            }),
        })
    }

    fn chat_completion_streaming(
        &self,
        token: &str,
        request: &ChatRequest,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<ChatResponse> {
        let response = self.chat_completion(token, request)?;

        let chunk_delay = match request.model.as_str() {
            "demo-breeze" => Duration::from_millis(55),
            "demo-story" => Duration::from_millis(70),
            "demo-chaos" => Duration::from_millis(85),
            _ => Duration::from_millis(65),
        };

        for chunk in response.streamed_text_chunks.iter() {
            on_chunk(chunk.clone());
            thread::sleep(chunk_delay);
        }

        Ok(response)
    }
}
