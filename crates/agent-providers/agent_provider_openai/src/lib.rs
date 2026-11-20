//! Chat providers for OpenAI-compatible and Ollama servers.

mod chat_provider;
mod entries;
mod ollama_wire;
mod openai_wire;
mod provider_crate;
mod roles;
mod transport;
mod urls;

#[cfg(test)]
mod template_tests;

pub use provider_crate::OpenAiProviderCrate;

// The editor's chat offers this provider (`agent_chat_core::provider_crates`).
pulsar_plugins::inventory::submit! {
    agent_chat_core::AgentProviderRegistration {
        origin: pulsar_plugins::engine_origin!(),
        id: "agent.openai",
        order: 0,
        create: || Box::new(OpenAiProviderCrate),
    }
}
