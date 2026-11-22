//! Building the chat and model-list URLs for an endpoint.

pub(crate) fn build_chat_url(endpoint: &str, ollama: bool, chat_path: Option<&str>) -> String {
    let t = endpoint.trim_end_matches('/');
    if ollama {
        if t.ends_with("/api/chat") {
            t.to_string()
        } else {
            format!("{t}/api/chat")
        }
    } else if let Some(path) = chat_path {
        if t.ends_with(path) {
            t.to_string()
        } else {
            format!("{t}{path}")
        }
    } else if t.ends_with("/chat/completions") {
        t.to_string()
    } else {
        format!("{t}/chat/completions")
    }
}

pub(crate) fn build_models_url(chat_url: &str, ollama: bool) -> String {
    let t = chat_url.trim_end_matches('/');
    if ollama {
        // Ollama uses a separate tags endpoint
        let base = if t.ends_with("/api/chat") {
            t.trim_end_matches("/api/chat")
        } else {
            t
        };
        return format!("{}/api/tags", base.trim_end_matches('/'));
    }
    // Strip the chat path to get the API base
    let base = if t.ends_with("/chat/completions") {
        t.trim_end_matches("/chat/completions")
    } else if t.ends_with("/api/chat") {
        t.trim_end_matches("/api/chat")
    } else if t.ends_with("/responses") {
        // OpenCode uses /responses instead of /chat/completions
        t.trim_end_matches("/responses")
    } else {
        t
    };
    format!("{}/models", base.trim_end_matches('/'))
}
