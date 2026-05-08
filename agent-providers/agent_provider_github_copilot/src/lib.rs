use agent_chat_core::{
    AuthHost, AuthMethod, AuthResult, ChatMessage, ChatProvider, ChatRequest, ChatResponse,
    ChatRole, ModelDescriptor, OpenBrowserRequest, PromptTokenRequest, ProviderAvailability,
    ProviderEnvironment, ProviderKind, ProviderMetadata, ToolCall,
};
use anyhow::{anyhow, Context};
use reqwest::blocking::Client;
use serde_json::{json, Value};

const COPILOT_CHAT_COMPLETIONS_URL: &str = "https://api.githubcopilot.com/chat/completions";
const GITHUB_MODELS_LIST_URL: &str = "https://models.inference.ai.azure.com/models";

pub struct GithubCopilotProvider {
    client: Client,
}

impl GithubCopilotProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    fn static_models() -> Vec<ModelDescriptor> {
        vec![
            ModelDescriptor {
                id: "gpt-5.3-codex",
                label: "GPT-5.3 Codex (Copilot)",
                supports_tools: true,
            },
            ModelDescriptor {
                id: "claude-sonnet-4",
                label: "Claude Sonnet 4 (Copilot)",
                supports_tools: true,
            },
            ModelDescriptor {
                id: "o4-mini",
                label: "o4 Mini (Copilot)",
                supports_tools: true,
            },
            ModelDescriptor {
                id: "gemini-2.5-pro",
                label: "Gemini 2.5 Pro (Copilot)",
                supports_tools: true,
            },
        ]
    }

    fn map_role(role: ChatRole) -> &'static str {
        match role {
            ChatRole::System => "system",
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
            ChatRole::Tool => "tool",
        }
    }

    fn auth_token_from_env(env: &dyn ProviderEnvironment) -> Option<String> {
        env.get_env("GITHUB_COPILOT_TOKEN")
            .or_else(|| env.get_env("COPILOT_TOKEN"))
            .or_else(|| env.get_env("GITHUB_TOKEN"))
    }

    fn parse_tool_calls(raw: &Value) -> Vec<ToolCall> {
        raw.get("choices")
            .and_then(|choices| choices.as_array())
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("tool_calls"))
            .and_then(|tool_calls| tool_calls.as_array())
            .map(|calls| {
                calls
                    .iter()
                    .filter_map(|call| {
                        let id = call.get("id")?.as_str()?.to_string();
                        let function = call.get("function")?;
                        let name = function.get("name")?.as_str()?.to_string();
                        let args_raw = function
                            .get("arguments")
                            .and_then(|v| v.as_str())
                            .unwrap_or("{}");

                        let arguments_json =
                            serde_json::from_str::<Value>(args_raw).unwrap_or_else(|_| json!({}));

                        Some(ToolCall {
                            id,
                            name,
                            arguments_json,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn parse_assistant_message(raw: &Value) -> Option<String> {
        raw.get("choices")
            .and_then(|choices| choices.as_array())
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_str())
            .map(|s| s.to_string())
    }
}

impl Default for GithubCopilotProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatProvider for GithubCopilotProvider {
    fn metadata(&self) -> ProviderMetadata {
        ProviderMetadata {
            id: "github_copilot",
            display_name: "GitHub Copilot",
            endpoint: COPILOT_CHAT_COMPLETIONS_URL,
            kind: ProviderKind::Cloud,
        }
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        Self::static_models()
    }

    fn availability(&self, env: &dyn ProviderEnvironment) -> ProviderAvailability {
        if Self::auth_token_from_env(env).is_some() {
            ProviderAvailability::ready()
        } else {
            ProviderAvailability::requires_auth(
                "Authentication required. Select provider and complete token auth.",
            )
        }
    }

    fn auth_methods(&self) -> Vec<AuthMethod> {
        vec![AuthMethod::PromptToken, AuthMethod::BrowserDeviceCode]
    }

    fn authenticate(
        &self,
        method: AuthMethod,
        host: &mut dyn AuthHost,
    ) -> anyhow::Result<AuthResult> {
        match method {
            AuthMethod::PromptToken => {
                let token = host.prompt_for_token(PromptTokenRequest {
                    title: "GitHub Copilot Authentication".to_string(),
                    prompt: "Paste your GitHub Copilot/GitHub Models token".to_string(),
                    placeholder: Some("ghp_xxx / github_pat_xxx".to_string()),
                    env_var_hint: Some("GITHUB_COPILOT_TOKEN".to_string()),
                })?;

                Ok(match token {
                    Some(token) => AuthResult::Authenticated { token },
                    None => AuthResult::Cancelled,
                })
            }
            AuthMethod::BrowserDeviceCode => {
                let token = host.open_browser_for_token(OpenBrowserRequest {
                    url: "https://github.com/login/device".to_string(),
                    instructions: "Authorize in browser, then paste the resulting token in Pulsar."
                        .to_string(),
                    code_hint: None,
                })?;

                Ok(match token {
                    Some(token) => AuthResult::Authenticated { token },
                    None => AuthResult::Cancelled,
                })
            }
        }
    }

    fn list_models_api(&self, token: &str) -> anyhow::Result<Vec<ModelDescriptor>> {
        let response = self
            .client
            .get(GITHUB_MODELS_LIST_URL)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/json")
            .send()
            .context("failed to call GitHub Models list API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!(
                "GitHub Models API returned {}: {}",
                status,
                body
            ));
        }

        let raw: Value = response.json().context("invalid JSON from models API")?;
        let has_any_models = raw
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.get("id").and_then(|v| v.as_str()).is_some())
            })
            .unwrap_or(false);

        if has_any_models {
            Ok(Self::static_models())
        } else {
            Ok(Self::static_models())
        }
    }

    fn chat_completion(
        &self,
        token: &str,
        request: &ChatRequest,
    ) -> anyhow::Result<ChatResponse> {
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|message: &ChatMessage| {
                json!({
                    "role": Self::map_role(message.role),
                    "content": message.content,
                })
            })
            .collect();

        let tools = if request.enable_tool_calls {
            request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters_json_schema,
                        }
                    })
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let mut payload = json!({
            "model": request.model,
            "messages": messages,
        });

        if let Some(temperature) = request.temperature {
            payload["temperature"] = json!(temperature);
        }
        if let Some(top_p) = request.top_p {
            payload["top_p"] = json!(top_p);
        }
        if let Some(max_tokens) = request.max_tokens {
            payload["max_tokens"] = json!(max_tokens);
        }
        if request.enable_tool_calls && !tools.is_empty() {
            payload["tools"] = Value::Array(tools);
        }

        let response = self
            .client
            .post(COPILOT_CHAT_COMPLETIONS_URL)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .json(&payload)
            .send()
            .context("failed to call Copilot chat completions API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!(
                "Copilot chat API returned {}: {}",
                status,
                body
            ));
        }

        let raw_response: Value = response
            .json()
            .context("invalid JSON from Copilot chat API")?;
        let assistant_message = Self::parse_assistant_message(&raw_response);
        let tool_calls = Self::parse_tool_calls(&raw_response);
        let streamed_text_chunks = assistant_message
            .as_ref()
            .map(|text| {
                text.chars()
                    .collect::<Vec<_>>()
                    .chunks(20)
                    .map(|chunk| chunk.iter().collect::<String>())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let finish_reason = raw_response
            .get("choices")
            .and_then(|choices| choices.as_array())
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(ChatResponse {
            assistant_message,
            streamed_text_chunks,
            tool_calls,
            finish_reason,
            raw_response,
        })
    }
}
