//! Tests for the provider catalog's templates, endpoint resolution and stream errors.

use agent_chat_core::{ProviderConfig, ProviderCrate};
use serde_json::json;

use crate::entries::{resolve_endpoint, Entry, ENTRIES};
use crate::provider_crate::OpenAiProviderCrate;
use crate::transport::stream_error;
use crate::urls::{build_chat_url, build_models_url};

use std::collections::HashMap;

fn config(pairs: &[(&str, &str)]) -> ProviderConfig {
    ProviderConfig {
        values: pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<HashMap<_, _>>(),
    }
}

#[test]
fn self_hosted_entries_are_templates_with_an_optional_endpoint() {
    let entries = OpenAiProviderCrate.entries();
    let lm = entries.iter().find(|e| e.id == "lm_studio").unwrap();
    assert!(lm.template);
    let endpoint = lm
        .config_fields
        .iter()
        .find(|f| f.key == "endpoint_url")
        .unwrap();
    assert!(!endpoint.required);
    assert_eq!(endpoint.placeholder, Some("http://localhost:1234/v1"));
    assert!(!entries.iter().find(|e| e.id == "openai").unwrap().template);
}

fn entry(id: &str) -> &'static Entry {
    ENTRIES.iter().find(|e| e.id == id).unwrap()
}

#[test]
fn templates_fall_back_to_their_default_endpoint() {
    let lm = entry("lm_studio");
    assert_eq!(
        resolve_endpoint(lm, &config(&[])).unwrap(),
        "http://localhost:1234/v1"
    );
    assert_eq!(
        resolve_endpoint(lm, &config(&[("endpoint_url", " http://gpu-box:1234/v1 ")])).unwrap(),
        "http://gpu-box:1234/v1"
    );
    // No default to fall back on.
    assert!(resolve_endpoint(entry("custom_openai"), &config(&[])).is_err());
    // Fixed cloud endpoints can't be redirected.
    assert_eq!(
        resolve_endpoint(entry("openai"), &config(&[("endpoint_url", "http://evil")])).unwrap(),
        "https://api.openai.com/v1"
    );
}

#[test]
fn chat_url_uses_the_configured_endpoint() {
    assert_eq!(
        build_chat_url("http://gpu-box:1234/v1/", false, None),
        "http://gpu-box:1234/v1/chat/completions"
    );
}

#[test]
fn cohere_uses_its_openai_compatible_api() {
    let cohere = entry("cohere");
    let chat = build_chat_url(cohere.endpoint.unwrap(), false, None);
    assert_eq!(
        chat,
        "https://api.cohere.ai/compatibility/v1/chat/completions"
    );
    assert_eq!(
        build_models_url(&chat, false),
        "https://api.cohere.ai/compatibility/v1/models"
    );
}

#[test]
fn a_stream_error_event_is_reported() {
    assert_eq!(
        stream_error(&json!({ "error": { "message": "quota", "type": "x" } })).as_deref(),
        Some("quota")
    );
    assert_eq!(
        stream_error(&json!({ "error": "down" })).as_deref(),
        Some("down")
    );
    assert_eq!(stream_error(&json!({ "error": null })), None);
    assert_eq!(stream_error(&json!({ "choices": [] })), None);
}
