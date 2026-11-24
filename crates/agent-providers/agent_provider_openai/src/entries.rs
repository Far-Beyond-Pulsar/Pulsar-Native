//! The provider catalog: each endpoint this crate serves and the config fields it asks for.

use agent_chat_core::{ConfigField, ProviderConfig, ProviderKind};

pub(crate) struct Entry {
    pub(crate) id: &'static str,
    pub(crate) display_name: &'static str,
    pub(crate) kind: ProviderKind,
    pub(crate) endpoint: Option<&'static str>,
    pub(crate) use_ollama_protocol: bool,
    /// Override the chat path appended to the endpoint, e.g. "/responses".
    /// When set, build_chat_url appends this instead of "/chat/completions".
    pub(crate) chat_path: Option<&'static str>,
    /// Connect as many named instances as you like (see `ProviderEntry::template`).
    pub(crate) template: bool,
}

pub(crate) const ENTRIES: &[Entry] = &[
    Entry {
        id: "openai",
        display_name: "OpenAI",
        kind: ProviderKind::Cloud,
        endpoint: Some("https://api.openai.com/v1"),
        use_ollama_protocol: false,
        chat_path: None,
        template: false,
    },
    Entry {
        id: "groq",
        display_name: "Groq",
        kind: ProviderKind::Cloud,
        endpoint: Some("https://api.groq.com/openai/v1"),
        use_ollama_protocol: false,
        chat_path: None,
        template: false,
    },
    Entry {
        id: "together",
        display_name: "Together AI",
        kind: ProviderKind::Cloud,
        endpoint: Some("https://api.together.xyz/v1"),
        use_ollama_protocol: false,
        chat_path: None,
        template: false,
    },
    Entry {
        id: "mistral",
        display_name: "Mistral AI",
        kind: ProviderKind::Cloud,
        endpoint: Some("https://api.mistral.ai/v1"),
        use_ollama_protocol: false,
        chat_path: None,
        template: false,
    },
    Entry {
        id: "deepseek",
        display_name: "DeepSeek",
        kind: ProviderKind::Cloud,
        endpoint: Some("https://api.deepseek.com/v1"),
        use_ollama_protocol: false,
        chat_path: None,
        template: false,
    },
    Entry {
        id: "fireworks",
        display_name: "Fireworks AI",
        kind: ProviderKind::Cloud,
        endpoint: Some("https://api.fireworks.ai/inference/v1"),
        use_ollama_protocol: false,
        chat_path: None,
        template: false,
    },
    Entry {
        id: "perplexity",
        display_name: "Perplexity",
        kind: ProviderKind::Cloud,
        endpoint: Some("https://api.perplexity.ai"),
        use_ollama_protocol: false,
        chat_path: None,
        template: false,
    },
    Entry {
        id: "xai",
        display_name: "xAI Grok",
        kind: ProviderKind::Cloud,
        endpoint: Some("https://api.x.ai/v1"),
        use_ollama_protocol: false,
        chat_path: None,
        template: false,
    },
    Entry {
        id: "openrouter",
        display_name: "OpenRouter",
        kind: ProviderKind::Cloud,
        endpoint: Some("https://openrouter.ai/api/v1"),
        use_ollama_protocol: false,
        chat_path: None,
        template: false,
    },
    Entry {
        id: "cohere",
        display_name: "Cohere",
        kind: ProviderKind::Cloud,
        // Cohere's own v2 API is not OpenAI-shaped; its compatibility API is.
        endpoint: Some("https://api.cohere.ai/compatibility/v1"),
        use_ollama_protocol: false,
        chat_path: None,
        template: false,
    },
    Entry {
        id: "azure_openai",
        display_name: "Azure OpenAI",
        kind: ProviderKind::Cloud,
        endpoint: None,
        use_ollama_protocol: false,
        chat_path: None,
        template: false,
    },
    Entry {
        id: "ollama",
        display_name: "Ollama",
        kind: ProviderKind::Local,
        endpoint: Some("http://localhost:11434"),
        use_ollama_protocol: true,
        chat_path: None,
        template: true,
    },
    Entry {
        id: "lm_studio",
        display_name: "LM Studio",
        kind: ProviderKind::Local,
        endpoint: Some("http://localhost:1234/v1"),
        use_ollama_protocol: false,
        chat_path: None,
        template: true,
    },
    Entry {
        id: "llama_cpp",
        display_name: "llama.cpp",
        kind: ProviderKind::Local,
        endpoint: Some("http://localhost:8080/v1"),
        use_ollama_protocol: false,
        chat_path: None,
        template: true,
    },
    Entry {
        id: "vllm",
        display_name: "vLLM",
        kind: ProviderKind::Local,
        endpoint: Some("http://localhost:8000/v1"),
        use_ollama_protocol: false,
        chat_path: None,
        template: true,
    },
    Entry {
        id: "custom_openai",
        display_name: "Custom OpenAI Compatible",
        kind: ProviderKind::Local,
        endpoint: None,
        use_ollama_protocol: false,
        chat_path: None,
        template: true,
    },
];

pub(crate) fn entry_config_fields(entry: &Entry, is_ollama: bool) -> Vec<ConfigField> {
    let mut fields = Vec::new();
    if entry.template {
        // Every instance points at its own server; the key is optional
        // because most self-hosted servers don't check one.
        fields.push(ConfigField {
            key: "endpoint_url",
            label: "Endpoint URL",
            description: match entry.endpoint {
                Some(default) => format!("Server URL. Leave empty for {default}").leak(),
                None => "API base URL",
            },
            sensitive: false,
            required: entry.endpoint.is_none(),
            placeholder: Some(entry.endpoint.unwrap_or("http://localhost:8080/v1")),
        });
        fields.push(ConfigField {
            key: "api_key",
            label: "API Key",
            description: "Optional. Leave empty if the server doesn't require one",
            sensitive: true,
            required: false,
            placeholder: None,
        });
        return fields;
    }
    if entry.endpoint.is_none() {
        fields.push(ConfigField {
            key: "endpoint_url",
            label: "Endpoint URL",
            description: if is_ollama {
                "Ollama server URL"
            } else {
                "API base URL"
            },
            sensitive: false,
            required: true,
            placeholder: if is_ollama {
                Some("http://localhost:11434")
            } else {
                Some("https://api.example.com/v1")
            },
        });
    }
    if entry.kind == ProviderKind::Cloud {
        fields.push(ConfigField {
            key: "api_key",
            label: "API Key",
            description: format!("{} API key", entry.display_name).leak(),
            sensitive: true,
            required: true,
            placeholder: None,
        });
    }
    fields
}

/// Templates take the configured endpoint, falling back to their default;
/// fixed entries always use their own.
pub(crate) fn resolve_endpoint(entry: &Entry, config: &ProviderConfig) -> anyhow::Result<String> {
    let configured = config
        .get("endpoint_url")
        .map(str::trim)
        .filter(|url| !url.is_empty() && (entry.template || entry.endpoint.is_none()));
    Ok(match (configured, entry.endpoint) {
        (Some(url), _) => url.to_string(),
        (None, Some(default)) => default.to_string(),
        (None, None) => config.require("endpoint_url")?.to_string(),
    })
}
