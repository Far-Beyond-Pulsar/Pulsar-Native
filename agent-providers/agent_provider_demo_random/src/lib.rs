use agent_chat_core::{
    AuthHost, AuthMethod, AuthResult, ChatProvider, ChatRequest, ChatResponse, ModelDescriptor,
    ProviderAvailability, ProviderEnvironment, ProviderKind, ProviderMetadata,
};
use anyhow::anyhow;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

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
            "I traced the signal through the editor and found a tiny shortcut no one noticed.",
            "This response is synthetic, but it is behaving like a real provider round-trip.",
            "A calm compiler and a noisy log can still make a very productive afternoon.",
            "The panel state is stable, and the provider bridge is doing exactly what it should.",
            "I can keep riffing while you wire the next provider implementation.",
            "Tool calls are optional here; this model is intentionally lightweight for demos.",
            "The model picked this line at random to mimic non-deterministic generation.",
            "If this feels alive, your provider abstraction is already paying off.",
            "Every prompt nudges the sentence mix, so no two replies should look identical.",
            "Use this provider for UI and flow testing when network auth is not available.",
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

        let assistant_message = Some(parts.join(" "));

        Ok(ChatResponse {
            assistant_message,
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_string()),
            raw_response: json!({
                "provider": "demo_random",
                "model": request.model,
                "mode": "synthetic_random_sentences"
            }),
        })
    }
}
