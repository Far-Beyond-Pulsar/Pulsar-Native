# agent-providers

AI provider crates — one per supported backend. Each implements the `ChatProvider` trait from `agent_chat_core`.

| Crate | Provider |
|---|---|
| `agent_chat_core` | Shared traits, types, and runtime |
| `agent_chat_tools` | Centralized tool execution |
| `agent_provider_anthropic` | Anthropic Claude |
| `agent_provider_aws_bedrock` | AWS Bedrock |
| `agent_provider_azure_openai` | Azure OpenAI |
| `agent_provider_cohere` | Cohere |
| `agent_provider_deepseek` | DeepSeek |
| `agent_provider_demo_random` | Demo / test provider |
| `agent_provider_docker_model_runner` | Docker-hosted models |
| `agent_provider_fireworks` | Fireworks AI |
| `agent_provider_gemini` | Google Gemini |
| `agent_provider_github_copilot` | GitHub Copilot |
| `agent_provider_groq` | Groq |
| `agent_provider_llama_cpp` | Local llama.cpp |
| `agent_provider_lmstudio` | LM Studio |
| `agent_provider_mistral` | Mistral AI |
| `agent_provider_ollama` | Ollama |
| `agent_provider_openai` | OpenAI |
| `agent_provider_openrouter` | OpenRouter |
| `agent_provider_perplexity` | Perplexity |
| `agent_provider_together` | Together AI |
| `agent_provider_vertex_ai` | Google Vertex AI |
| `agent_provider_vllm` | vLLM |
| `agent_provider_xai` | xAI |

All provider crates depend on `agent_chat_core`. Tool execution is centralized in `agent_chat_tools`.

