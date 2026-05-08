use super::*;
use agent_chat_core::{AuthHost, AuthMethod, AuthResult, AvailabilityState, ProcessEnvironment, PromptTokenRequest};
use smol::Timer;
use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};

impl AgentChatPanel {
    pub(super) fn auth_token_for_provider(&self, provider_id: &str) -> Option<String> {
        self.provider_tokens.get(provider_id).cloned()
    }

    pub(super) fn maybe_require_auth_for_active_provider(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.active_provider() else {
            self.pending_auth_provider = None;
            return;
        };

        if self.wip_providers.contains_key(provider.id) {
            self.pending_auth_provider = None;
            return;
        }

        if self.auth_token_for_provider(provider.id).is_some() {
            self.pending_auth_provider = None;
            return;
        }

        if let Some(provider_impl) = self.provider_registry.get(provider.id) {
            let availability = provider_impl.availability(&ProcessEnvironment);
            if matches!(availability.state, AvailabilityState::RequiresAuth) {
                self.pending_auth_provider = Some(provider.id);
                cx.notify();
                return;
            }
        }

        self.pending_auth_provider = None;
    }

    pub(super) fn complete_prompt_auth(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(provider_id) = self.pending_auth_provider else {
            return;
        };

        let token = self.auth_token_input.read(cx).text().to_string();
        let token = token.trim().to_string();
        if token.is_empty() {
            return;
        }

        let Some(provider) = self.provider_registry.get(provider_id).cloned() else {
            return;
        };

        struct PromptOnlyAuthHost {
            token: String,
        }

        impl AuthHost for PromptOnlyAuthHost {
            fn prompt_for_token(
                &mut self,
                _request: PromptTokenRequest,
            ) -> anyhow::Result<Option<String>> {
                Ok(Some(self.token.clone()))
            }

            fn open_browser_for_token(
                &mut self,
                _request: agent_chat_core::OpenBrowserRequest,
            ) -> anyhow::Result<Option<String>> {
                Ok(None)
            }
        }

        let mut host = PromptOnlyAuthHost { token };
        match provider.authenticate(AuthMethod::PromptToken, &mut host) {
            Ok(AuthResult::Authenticated { token }) => {
                self.provider_tokens.insert(provider_id, token);
                self.pending_auth_provider = None;
                self.auth_token_input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                });
                self.messages.push(ChatMessage {
                    role: "system",
                    content: format!("{} authenticated successfully.", provider_id),
                });
                self.save_current_chat();
                self.refresh_chat_history_list(cx);
                self.scroll_messages_to_bottom();
                cx.notify();
            }
            Ok(AuthResult::Cancelled) => {}
            Err(err) => {
                self.messages.push(ChatMessage {
                    role: "system",
                    content: format!("Authentication failed: {err}"),
                });
                self.save_current_chat();
                self.refresh_chat_history_list(cx);
                self.scroll_messages_to_bottom();
                cx.notify();
            }
        }
    }

    pub(super) fn begin_browser_auth(&mut self, cx: &mut Context<Self>) {
        let Some(provider_id) = self.pending_auth_provider else {
            return;
        };
        let Some(provider) = self.provider_registry.get(provider_id).cloned() else {
            return;
        };

        // If the provider supports the OAuth device-code flow, use it instead of
        // asking the user to paste a token (PATs are rejected by the Copilot API).
        if let Some(flow_result) = provider.start_device_flow() {
            match flow_result {
                Ok(info) => {
                    self.messages.push(ChatMessage {
                        role: "system",
                        content: format!(
                            "Open {} in your browser and enter code: **{}**",
                            info.verification_uri, info.user_code
                        ),
                    });
                    self.pending_device_code = Some(info.device_code.clone());
                    self.scroll_messages_to_bottom();
                    cx.notify();
                    cx.open_url(&info.verification_uri);

                    let device_code = info.device_code;
                    let interval = info.interval.max(5);

                    cx.spawn(async move |this, cx| {
                        loop {
                            Timer::after(Duration::from_secs(interval)).await;

                            // Perform a single blocking poll on whatever thread GPUI picks.
                            let poll = cx.update(|cx| {
                                this.update(cx, |panel, _cx| {
                                    panel
                                        .provider_registry
                                        .get(provider_id)
                                        .cloned()
                                        .map(|p| p.poll_device_code(&device_code))
                                })
                                .ok()
                                .flatten()
                            });

                            match poll {
                                Ok(Some(Ok(Some(token)))) => {
                                    cx.update(|cx| {
                                        this.update(cx, |panel, cx| {
                                            panel.provider_tokens.insert(provider_id, token);
                                            panel.pending_device_code = None;
                                            panel.pending_auth_provider = None;
                                            panel.messages.push(ChatMessage {
                                                role: "system",
                                                content: format!(
                                                    "{} authenticated successfully.",
                                                    provider_id
                                                ),
                                            });
                                            panel.save_current_chat();
                                            panel.refresh_chat_history_list(cx);
                                            panel.scroll_messages_to_bottom();
                                            cx.notify();
                                        })
                                        .ok();
                                    })
                                    .ok();
                                    break;
                                }
                                // authorization_pending or slow_down — keep polling
                                Ok(Some(Ok(None))) => {}
                                // error or the entity/context was dropped
                                _ => {
                                    cx.update(|cx| {
                                        this.update(cx, |panel, cx| {
                                            panel.pending_device_code = None;
                                            panel.messages.push(ChatMessage {
                                                role: "system",
                                                content: "Device code authentication failed or timed out.".to_string(),
                                            });
                                            panel.scroll_messages_to_bottom();
                                            cx.notify();
                                        })
                                        .ok();
                                    })
                                    .ok();
                                    break;
                                }
                            }
                        }
                    })
                    .detach();
                }
                Err(err) => {
                    self.messages.push(ChatMessage {
                        role: "system",
                        content: format!("Failed to start device flow: {err}"),
                    });
                    self.scroll_messages_to_bottom();
                    cx.notify();
                }
            }
            return;
        }

        // Fallback: providers that only support opening a URL (no device-code polling).
        struct BrowserOnlyAuthHost {
            browser_url: Option<String>,
        }

        impl AuthHost for BrowserOnlyAuthHost {
            fn prompt_for_token(
                &mut self,
                _request: PromptTokenRequest,
            ) -> anyhow::Result<Option<String>> {
                Ok(None)
            }

            fn open_browser_for_token(
                &mut self,
                request: agent_chat_core::OpenBrowserRequest,
            ) -> anyhow::Result<Option<String>> {
                self.browser_url = Some(request.url);
                Ok(None)
            }
        }

        let mut host = BrowserOnlyAuthHost { browser_url: None };
        if provider
            .authenticate(AuthMethod::BrowserDeviceCode, &mut host)
            .is_ok()
        {
            if let Some(url) = host.browser_url {
                cx.open_url(&url);
            }
        }
    }
}
