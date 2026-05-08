# Agent Providers

This folder contains chat provider/runtime crates for Pulsar engine chat.

Planned crates:
- agent_chat_core
- agent_chat_tools
- agent_provider_openai
- agent_provider_anthropic
- agent_provider_ollama
- agent_provider_lmstudio
- agent_provider_openrouter
- agent_provider_github_copilot

Notes:
- Each provider crate implements the shared ChatProvider trait from agent_chat_core.
- Tool execution is centralized in agent_chat_tools.
- UI crates should depend on the runtime facade instead of provider-specific crates directly.
