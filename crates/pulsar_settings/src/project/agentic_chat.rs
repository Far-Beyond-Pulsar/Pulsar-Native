use pulsar_config::{
    ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
};

pub const NS: &str = "project";
pub const OWNER: &str = "agentic_chat";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new(
        "Agent Chat",
        "Settings for the global agentic chat panel, model providers, and runtime behavior",
    )
    .setting(
        "enabled",
        SchemaEntry::new("Enable the global right-side agent chat panel", true)
            .label("Enable Agent Chat")
            .page("Agent Chat")
            .field_type(FieldType::Checkbox),
    )
    .setting(
        "default_provider",
        SchemaEntry::new("Default model provider used for new chat sessions", "openai")
            .label("Default Provider")
            .page("Agent Chat")
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption::new("OpenAI", "openai"),
                    DropdownOption::new("Anthropic", "anthropic"),
                    DropdownOption::new("Google", "google"),
                    DropdownOption::new("OpenRouter", "openrouter"),
                    DropdownOption::new("Ollama (Local)", "ollama"),
                    DropdownOption::new("LM Studio (Local)", "lmstudio"),
                    DropdownOption::new("Custom", "custom"),
                ],
            })
            .validator(Validator::string_one_of([
                "openai",
                "anthropic",
                "google",
                "openrouter",
                "ollama",
                "lmstudio",
                "custom",
            ])),
    )
    .setting(
        "default_model",
        SchemaEntry::new("Default model identifier", "gpt-5.3-codex")
            .label("Default Model")
            .page("Agent Chat")
            .field_type(FieldType::TextInput {
                placeholder: Some("gpt-5.3-codex".into()),
                multiline: false,
            }),
    )
    .setting(
        "provider_priority",
        SchemaEntry::new(
            "Comma-separated provider preference order used for failover",
            "openai,anthropic,ollama",
        )
        .label("Provider Priority")
        .page("Agent Chat")
        .field_type(FieldType::TextInput {
            placeholder: Some("openai,anthropic,ollama".into()),
            multiline: false,
        }),
    )
    .setting(
        "allow_cloud_providers",
        SchemaEntry::new("Allow cloud-hosted provider endpoints", true)
            .label("Allow Cloud Providers")
            .page("Agent Chat")
            .field_type(FieldType::Checkbox),
    )
    .setting(
        "allow_local_providers",
        SchemaEntry::new("Allow local/self-hosted provider endpoints", true)
            .label("Allow Local Providers")
            .page("Agent Chat")
            .field_type(FieldType::Checkbox),
    )
    .setting(
        "allow_model_auto_discovery",
        SchemaEntry::new("Automatically discover available models from active providers", true)
            .label("Auto-Discover Models")
            .page("Agent Chat")
            .field_type(FieldType::Checkbox),
    )
    .setting(
        "cloud_api_base_url",
        SchemaEntry::new("Override base URL for cloud provider API gateway", "")
            .label("Cloud API Base URL")
            .page("Agent Chat")
            .field_type(FieldType::TextInput {
                placeholder: Some("https://api.openai.com/v1".into()),
                multiline: false,
            }),
    )
    .setting(
        "local_api_base_url",
        SchemaEntry::new("Override base URL for local provider runtime", "http://localhost:11434")
            .label("Local API Base URL")
            .page("Agent Chat")
            .field_type(FieldType::TextInput {
                placeholder: Some("http://localhost:11434".into()),
                multiline: false,
            }),
    )
    .setting(
        "api_key_env_var",
        SchemaEntry::new(
            "Environment variable name used to resolve provider API keys",
            "PULSAR_AGENT_API_KEY",
        )
        .label("API Key Env Var")
        .page("Agent Chat")
        .field_type(FieldType::TextInput {
            placeholder: Some("PULSAR_AGENT_API_KEY".into()),
            multiline: false,
        }),
    )
    .setting(
        "request_timeout_ms",
        SchemaEntry::new("Provider request timeout in milliseconds", 60_000_i64)
            .label("Request Timeout (ms)")
            .page("Agent Chat")
            .field_type(FieldType::NumberInput {
                min: Some(1_000.0),
                max: Some(300_000.0),
                step: Some(1_000.0),
            })
            .validator(Validator::int_range(1_000, 300_000)),
    )
    .setting(
        "max_context_tokens",
        SchemaEntry::new("Maximum context window tokens for chat requests", 65_536_i64)
            .label("Max Context Tokens")
            .page("Agent Chat")
            .field_type(FieldType::NumberInput {
                min: Some(1_024.0),
                max: Some(1_000_000.0),
                step: Some(1_024.0),
            })
            .validator(Validator::int_range(1_024, 1_000_000)),
    )
    .setting(
        "temperature",
        SchemaEntry::new("Sampling temperature for text generation", 0.2_f64)
            .label("Temperature")
            .page("Agent Chat")
            .field_type(FieldType::Slider {
                min: 0.0,
                max: 2.0,
                step: 0.05,
            })
            .validator(Validator::float_range(0.0, 2.0)),
    )
    .setting(
        "top_p",
        SchemaEntry::new("Nucleus sampling threshold", 1.0_f64)
            .label("Top P")
            .page("Agent Chat")
            .field_type(FieldType::Slider {
                min: 0.1,
                max: 1.0,
                step: 0.05,
            })
            .validator(Validator::float_range(0.1, 1.0)),
    )
    .setting(
        "enable_streaming",
        SchemaEntry::new("Enable streaming responses in the chat panel", true)
            .label("Enable Streaming")
            .page("Agent Chat")
            .field_type(FieldType::Checkbox),
    )
    .setting(
        "enable_tool_calls",
        SchemaEntry::new("Allow tool and function-call execution from the selected model", true)
            .label("Enable Tool Calls")
            .page("Agent Chat")
            .field_type(FieldType::Checkbox),
    )
    .setting(
        "chat_history_limit",
        SchemaEntry::new("Maximum number of messages retained in local chat history", 200_i64)
            .label("Chat History Limit")
            .page("Agent Chat")
            .field_type(FieldType::NumberInput {
                min: Some(20.0),
                max: Some(2_000.0),
                step: Some(10.0),
            })
            .validator(Validator::int_range(20, 2_000)),
    );

    let _ = cfg.register(NS, OWNER, schema);
}