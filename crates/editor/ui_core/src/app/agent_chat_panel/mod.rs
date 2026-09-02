mod auth;
mod builder;
mod chat_history;
mod chat_storage;
mod context;
mod custom_provider_wizard;
mod panel;
mod prompt_ranking;
mod provider_catalog;
mod provider_selection;
mod render;
mod render_header;
mod render_messages;
mod render_overlays;
mod streaming;
mod subagent;
pub mod types;

pub use panel::AgentChatPanel;
pub(crate) use panel::SubagentCompletionMode;
pub use types::*;

use crate::custom_providers::{self, CustomProvider};
use agent_chat_core::{
    ChatMessage, ChatProvider, ChatRole, ProviderCrate, ProviderEntry, ProviderRegistry,
};
use agent_chat_tools::ToolRegistry;
use agent_provider_anthropic::AnthropicProviderCrate;
use agent_provider_aws_bedrock::AwsBedrockProviderCrate;
use agent_provider_demo_random::DemoRandomProviderCrate;
use agent_provider_docker_model_runner::DockerModelRunnerProviderCrate;
use agent_provider_gemini::GeminiProviderCrate;
use agent_provider_github_copilot::GithubCopilotProviderCrate;
use agent_provider_openai::OpenAiProviderCrate;
use agent_provider_opencode::OpenCodeProviderCrate;
use agent_provider_vertex_ai::VertexAiProviderCrate;
use gpui::{prelude::FluentBuilder as _, *};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    path::PathBuf,
    rc::Rc,
    sync::{Arc, RwLock},
};
use ui::{
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockItem, Panel, PanelEvent, TabPanel},
    dropdown::{
        SearchableList, SearchableListEvent, SearchableListItemAction, SearchableListItemState,
    },
    h_flex,
    input::Enter,
    input::{InputState, TextInput},
    popover::Popover,
    scroll::{Scrollbar, ScrollbarState},
    spinner::Spinner,
    text::TextView,
    v_flex, v_virtual_list, ActiveTheme as _, Disableable, Icon, IconName, Sizable, Size,
    StyledExt, VirtualListScrollHandle,
};

impl Render for AgentChatPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.maybe_reload_chats_from_disk(cx);
        self.poll_subagent_completion_events(cx);

        let provider = self.active_provider();
        let model = self.active_model();
        let configuring = self.configuring_provider.clone();
        let display_count = self.display_items.len();
        let render_now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let display_item_sizes = std::rc::Rc::new(
            self.display_items
                .iter()
                .enumerate()
                .map(|(ix, item)| {
                    let h = self
                        .display_item_heights
                        .get(&ix)
                        .copied()
                        .unwrap_or_else(|| Self::display_item_height(item));
                    size(px(0.0), h)
                })
                .chain(std::iter::once(size(px(0.0), px(120.0))))
                .collect::<Vec<_>>(),
        );

        let queued_subagent_count = self.pending_subagent_events.len();

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .on_action(cx.listener(Self::on_prompt_enter))
            .child(
                v_flex()
                    .w_full()
                    .px_3()
                    .py(px(6.0))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().tab_bar)
                    .child(self.render_header(cx))
                    .when_some(self.render_config_overlay(cx), |el, overlay| {
                        el.child(overlay)
                    })
                    .when_some(self.render_custom_provider_wizard(cx), |el, overlay| {
                        el.child(overlay)
                    }),
            )
            .child(
                div()
                    .id("agent-chat-scroll-area")
                    .relative()
                    .flex_1()
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                        let scrolled_up = match event.delta {
                            ScrollDelta::Pixels(p) => p.y < px(0.0),
                            ScrollDelta::Lines(l) => l.y < 0.0,
                        };
                        if scrolled_up && this.auto_scroll {
                            this.auto_scroll = false;
                            cx.notify();
                        }
                    }))
                    .child(
                        v_virtual_list(
                            cx.entity().clone(),
                            "agent-chat-messages-virtual-list",
                            display_item_sizes,
                            move |this,
                                  range: std::ops::Range<usize>,
                                  window,
                                  cx: &mut Context<Self>| {
                                range
                                    .map(|ix| {
                                        if ix == display_count {
                                            return div().h(px(120.0)).into_any_element();
                                        }

                                        let Some(item) = this.display_items.get(ix) else {
                                            return div().h(px(52.0)).into_any_element();
                                        };

                                        let panel = cx.entity().clone();

                                        match item {
                                            DisplayItem::ToolCallGroup { calls, is_expanded, started_at_ms, finished_at_ms } => {
                                                let calls = calls.clone();
                                                let is_expanded = *is_expanded;
                                                let copy_tool_block = calls
                                                    .iter()
                                                    .map(|call| {
                                                        let args = if call.args_full.is_empty() {
                                                            call.args_preview.clone()
                                                        } else {
                                                            call.args_full.clone()
                                                        };
                                                        let result = call
                                                            .result_full
                                                            .as_deref()
                                                            .or(call.result_preview.as_deref())
                                                            .unwrap_or("running\u{2026}");
                                                        format!(
                                                            "tool: {}\nargs: {}\nresult: {}{}",
                                                            call.name,
                                                            args,
                                                            result,
                                                            if call.is_error { "\nstatus: error" } else { "" }
                                                        )
                                                    })
                                                    .collect::<Vec<_>>()
                                                    .join("\n\n");
                                                let group_elapsed = Self::format_elapsed(*started_at_ms, *finished_at_ms, render_now_ms);
                                                let tool_names: Vec<String> =
                                                    calls.iter().map(|c| c.name.clone()).collect();
                                                let all_done =
                                                    calls.iter().all(|c| c.result_preview.is_some());
                                                let has_error =
                                                    calls.iter().any(|c| c.is_error);

                                                let accent = if has_error {
                                                    cx.theme().danger
                                                } else if all_done {
                                                    cx.theme().success
                                                } else {
                                                    cx.theme().muted_foreground
                                                };

                                                let status_icon = if has_error {
                                                    IconName::CircleX
                                                } else if all_done {
                                                    IconName::CircleCheck
                                                } else {
                                                    IconName::Loader
                                                };

                                                let header_label = if calls.len() == 1 {
                                                    format!("Used tool: {}", tool_names[0])
                                                } else {
                                                    format!(
                                                        "Used {} tools: {}",
                                                        calls.len(),
                                                        tool_names.join(", ")
                                                    )
                                                };

                                                div()
                                                    .relative()
                                                    .w_full()
                                                    .min_w_0()
                                                    .px_3()
                                                    .py_1()
                                                    .child(
                                                        canvas(
                                                            move |bounds, _, cx| {
                                                                panel.update(cx, |panel, cx| {
                                                                    let measured =
                                                                        bounds.size.height;
                                                                    if panel
                                                                        .display_item_heights
                                                                        .get(&ix)
                                                                        .copied()
                                                                        != Some(measured)
                                                                    {
                                                                        panel
                                                                            .display_item_heights
                                                                            .insert(ix, measured);
                                                                        cx.notify();
                                                                    }
                                                                });
                                                            },
                                                            |_, _, _, _| {},
                                                        )
                                                        .absolute()
                                                        .inset_0(),
                                                    )
                                                    .child(
                                                        v_flex()
                                                            .w_full()
                                                            .gap_px()
                                                            .rounded(px(6.0))
                                                            .border_1()
                                                            .border_color(accent.opacity(0.25))
                                                            .bg(cx.theme().secondary)
                                                            .overflow_hidden()
                                                            .child(
                                                                h_flex()
                                                                    .id(("tool-call-header", ix))
                                                                    .w_full()
                                                                    .px_3()
                                                                    .py(px(6.0))
                                                                    .gap_2()
                                                                    .cursor_pointer()
                                                                    .on_click(cx.listener(
                                                                        move |this, _, _, cx| {
                                                                            if let Some(
                                                                                DisplayItem::ToolCallGroup {
                                                                                    is_expanded,
                                                                                    ..
                                                                                },
                                                                            ) = this
                                                                                .display_items
                                                                                .get_mut(ix)
                                                                            {
                                                                                *is_expanded =
                                                                                    !*is_expanded;
                                                                                this.display_item_heights
                                                                                    .remove(&ix);
                                                                            }
                                                                            cx.notify();
                                                                        },
                                                                    ))
                                                                    .child(
                                                                        Icon::new(IconName::Terminal)
                                                                            .size_3()
                                                                            .text_color(accent),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .flex_1()
                                                                            .text_xs()
                                                                            .text_color(
                                                                                cx.theme()
                                                                                    .muted_foreground,
                                                                            )
                                                                            .child(header_label),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(cx.theme().muted_foreground.opacity(0.6))
                                                                            .font_family("JetBrains Mono")
                                                                            .child(group_elapsed),
                                                                    )
                                                                    .child(
                                                                        Button::new(("tool-call-copy", ix))
                                                                            .xsmall()
                                                                            .ghost()
                                                                            .icon(IconName::Copy)
                                                                            .tooltip("Copy full tool block")
                                                                            .on_click(cx.listener(
                                                                                move |_, _, _, cx| {
                                                                                    cx.write_to_clipboard(
                                                                                        gpui::ClipboardItem::new_string(
                                                                                            copy_tool_block.clone(),
                                                                                        ),
                                                                                    );
                                                                                },
                                                                            )),
                                                                    )
                                                                    .child(
                                                                        Icon::new(status_icon)
                                                                            .size_3()
                                                                            .text_color(accent),
                                                                    )
                                                                    .child(
                                                                        Icon::new(if is_expanded {
                                                                            IconName::ChevronUp
                                                                        } else {
                                                                            IconName::ChevronDown
                                                                        })
                                                                        .size_3()
                                                                        .text_color(
                                                                            cx.theme()
                                                                                .muted_foreground
                                                                                .opacity(0.6),
                                                                        ),
                                                                    ),
                                                            )
                                                            .when(is_expanded, |el| {
                                                                el.child(
                                                                    v_flex()
                                                                        .w_full()
                                                                        .gap_px()
                                                                        .children(
                                                                            calls.iter().map(|call| {
                                                                                v_flex()
                                                                                    .w_full()
                                                                                    .px_3()
                                                                                    .py_2()
                                                                                    .gap_1()
                                                                                    .border_t_1()
                                                                                    .border_color(
                                                                                        cx.theme()
                                                                                            .border,
                                                                                    )
                                                                                    .child(
                                                                                        h_flex()
                                                                                            .gap_2()
                                                                                            .items_center()
                                                                                            .child(
                                                                                                div()
                                                                                                    .text_sm()
                                                                                                    .font_semibold()
                                                                                                    .text_color(cx.theme().foreground)
                                                                                                    .child(call.name.clone()),
                                                                                            )
                                                                                            .when(call.is_error, |el| {
                                                                                                el.child(
                                                                                                    div()
                                                                                                        .text_xs()
                                                                                                        .text_color(cx.theme().danger)
                                                                                                        .child("error"),
                                                                                                )
                                                                                            }),
                                                                                    )
                                                                                    .child(
                                                                                        div()
                                                                                            .text_xs()
                                                                                            .font_family("JetBrains Mono")
                                                                                            .text_color(cx.theme().muted_foreground)
                                                                                            .child(format!("args: {}", call.args_preview)),
                                                                                    )
                                                                                    .when_some(
                                                                                        call.result_preview.as_ref(),
                                                                                        |el, result| {
                                                                                            el.child(
                                                                                                div()
                                                                                                    .text_xs()
                                                                                                    .font_family("JetBrains Mono")
                                                                                                    .text_color(cx.theme().muted_foreground.opacity(0.8))
                                                                                                    .child(format!("\u{2192} {result}")),
                                                                                            )
                                                                                        },
                                                                                    )
                                                                                    .when(
                                                                                        call.result_preview.is_none(),
                                                                                        |el| {
                                                                                            el.child(
                                                                                                div()
                                                                                                    .text_xs()
                                                                                                    .text_color(
                                                                                                        cx.theme()
                                                                                                            .muted_foreground
                                                                                                            .opacity(0.5),
                                                                                                    )
                                                                                                    .child("running\u{2026}"),
                                                                                            )
                                                                                        },
                                                                                    )
                                                                            }),
                                                                        ),
                                                                )
                                                            }),
                                                    )
                                                    .into_any_element()
                                            }

                                            DisplayItem::CompactionSummary {
                                                summary,
                                                is_expanded,
                                                started_at_ms,
                                                finished_at_ms,
                                            } => {
                                                let summary = summary.clone();
                                                let copy_summary = summary.clone();
                                                let is_expanded = *is_expanded;
                                                let compact_elapsed = Self::format_elapsed(*started_at_ms, *finished_at_ms, render_now_ms);
                                                let accent = cx.theme().warning;

                                                div()
                                                    .relative()
                                                    .w_full()
                                                    .min_w_0()
                                                    .px_3()
                                                    .py_1()
                                                    .child(
                                                        canvas(
                                                            move |bounds, _, cx| {
                                                                panel.update(cx, |panel, cx| {
                                                                    let measured = bounds.size.height;
                                                                    if panel.display_item_heights.get(&ix).copied() != Some(measured) {
                                                                        panel.display_item_heights.insert(ix, measured);
                                                                        cx.notify();
                                                                    }
                                                                });
                                                            },
                                                            |_, _, _, _| {},
                                                        )
                                                        .absolute()
                                                        .inset_0(),
                                                    )
                                                    .child(
                                                        v_flex()
                                                            .w_full()
                                                            .rounded(px(6.0))
                                                            .border_1()
                                                            .border_color(accent.opacity(0.3))
                                                            .bg(cx.theme().secondary)
                                                            .overflow_hidden()
                                                            .child(
                                                                h_flex()
                                                                    .id(("compaction-header", ix))
                                                                    .w_full()
                                                                    .px_3()
                                                                    .py(px(5.0))
                                                                    .gap_2()
                                                                    .cursor_pointer()
                                                                    .on_click(cx.listener(
                                                                        move |this, _, _, cx| {
                                                                            if let Some(DisplayItem::CompactionSummary { is_expanded, .. }) = this.display_items.get_mut(ix) {
                                                                                *is_expanded = !*is_expanded;
                                                                                this.display_item_heights.remove(&ix);
                                                                            }
                                                                            cx.notify();
                                                                        },
                                                                    ))
                                                                    .child(
                                                                        Icon::new(IconName::Scissor)
                                                                            .size_3()
                                                                            .text_color(accent),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .flex_1()
                                                                            .text_xs()
                                                                            .text_color(cx.theme().muted_foreground)
                                                                            .child("Context compacted \u{2014} earlier messages summarised"),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(accent.opacity(0.7))
                                                                            .font_family("JetBrains Mono")
                                                                            .child(compact_elapsed),
                                                                    )
                                                                    .child(
                                                                        Button::new(("compaction-copy", ix))
                                                                            .xsmall()
                                                                            .ghost()
                                                                            .icon(IconName::Copy)
                                                                            .tooltip("Copy full compacted summary")
                                                                            .on_click(cx.listener(
                                                                                move |_, _, _, cx| {
                                                                                    cx.write_to_clipboard(
                                                                                        gpui::ClipboardItem::new_string(
                                                                                            copy_summary.clone(),
                                                                                        ),
                                                                                    );
                                                                                },
                                                                            )),
                                                                    )
                                                                    .child(
                                                                        Icon::new(if is_expanded {
                                                                            IconName::ChevronUp
                                                                        } else {
                                                                            IconName::ChevronDown
                                                                        })
                                                                        .size_3()
                                                                        .text_color(cx.theme().muted_foreground.opacity(0.5)),
                                                                    ),
                                                            )
                                                            .when(is_expanded && !summary.is_empty(), |el| {
                                                                el.child(
                                                                    div()
                                                                        .w_full()
                                                                        .px_3()
                                                                        .py_2()
                                                                        .border_t_1()
                                                                        .border_color(cx.theme().border)
                                                                        .text_xs()
                                                                        .font_family("JetBrains Mono")
                                                                        .text_color(cx.theme().muted_foreground)
                                                                        .whitespace_normal()
                                                                        .child(summary),
                                                                )
                                                            }),
                                                    )
                                                    .into_any_element()
                                            }

                                            DisplayItem::SystemPrompt {
                                                content,
                                                is_expanded,
                                                is_outdated,
                                            } => {
                                                let content = content.clone();
                                                let copy_content = content.clone();
                                                let is_expanded = *is_expanded;
                                                let is_outdated = *is_outdated;
                                                let accent = if is_outdated {
                                                    cx.theme().warning
                                                } else {
                                                    cx.theme().muted_foreground
                                                };

                                                div()
                                                    .relative()
                                                    .w_full()
                                                    .min_w_0()
                                                    .px_3()
                                                    .py_1()
                                                    .child(
                                                        canvas(
                                                            move |bounds, _, cx| {
                                                                panel.update(cx, |panel, cx| {
                                                                    let measured = bounds.size.height;
                                                                    if panel.display_item_heights.get(&ix).copied() != Some(measured) {
                                                                        panel.display_item_heights.insert(ix, measured);
                                                                        cx.notify();
                                                                    }
                                                                });
                                                            },
                                                            |_, _, _, _| {},
                                                        )
                                                        .absolute()
                                                        .inset_0(),
                                                    )
                                                    .child(
                                                        v_flex()
                                                            .w_full()
                                                            .rounded(px(6.0))
                                                            .border_1()
                                                            .border_color(accent.opacity(0.25))
                                                            .bg(cx.theme().secondary)
                                                            .overflow_hidden()
                                                            .child(
                                                                h_flex()
                                                                    .id(("system-prompt-header", ix))
                                                                    .w_full()
                                                                    .px_3()
                                                                    .py(px(6.0))
                                                                    .gap_2()
                                                                    .cursor_pointer()
                                                                    .on_click(cx.listener(
                                                                        move |this, _, _, cx| {
                                                                            if let Some(DisplayItem::SystemPrompt { is_expanded, .. }) =
                                                                                this.display_items.get_mut(ix)
                                                                            {
                                                                                *is_expanded = !*is_expanded;
                                                                                this.display_item_heights.remove(&ix);
                                                                            }
                                                                            cx.notify();
                                                                        },
                                                                    ))
                                                                    .child(
                                                                        Icon::new(IconName::Settings)
                                                                            .size_3()
                                                                            .text_color(accent),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .flex_1()
                                                                            .text_xs()
                                                                            .font_semibold()
                                                                            .text_color(cx.theme().muted_foreground)
                                                                            .child("System Prompt"),
                                                                    )
                                                                    .when(is_outdated, |el| {
                                                                        el.child(
                                                                            Button::new("system-prompt-update")
                                                                                .xsmall()
                                                                                .ghost()
                                                                                .label("Update")
                                                                                .tooltip("Replace with current default system prompt")
                                                                                .on_click(cx.listener(|this, _, _, cx| {
                                                                                    this.apply_system_prompt_update(cx);
                                                                                })),
                                                                        )
                                                                    })
                                                                    .child(
                                                                        Button::new(("system-prompt-copy", ix))
                                                                            .xsmall()
                                                                            .ghost()
                                                                            .icon(IconName::Copy)
                                                                            .tooltip("Copy full system prompt")
                                                                            .on_click(cx.listener(
                                                                                move |_, _, _, cx| {
                                                                                    cx.write_to_clipboard(
                                                                                        gpui::ClipboardItem::new_string(
                                                                                            copy_content.clone(),
                                                                                        ),
                                                                                    );
                                                                                },
                                                                            )),
                                                                    )
                                                                    .child(
                                                                        Icon::new(if is_expanded {
                                                                            IconName::ChevronUp
                                                                        } else {
                                                                            IconName::ChevronDown
                                                                        })
                                                                        .size_3()
                                                                        .text_color(cx.theme().muted_foreground.opacity(0.6)),
                                                                    ),
                                                            )
                                                            .when(is_expanded && !content.is_empty(), |el| {
                                                                el.child(
                                                                    div()
                                                                        .w_full()
                                                                        .px_3()
                                                                        .py_2()
                                                                        .border_t_1()
                                                                        .border_color(cx.theme().border)
                                                                        .text_xs()
                                                                        .font_family("JetBrains Mono")
                                                                        .text_color(cx.theme().muted_foreground)
                                                                        .whitespace_normal()
                                                                        .child(content),
                                                                )
                                                            }),
                                                    )
                                                    .into_any_element()
                                            }

                                            DisplayItem::ThinkingBlock {
                                                content,
                                                is_expanded,
                                                is_done,
                                                started_at_ms,
                                                finished_at_ms,
                                            } => {
                                                let content = content.clone();
                                                let copy_content = content.clone();
                                                let is_expanded = *is_expanded;
                                                let is_done = *is_done;
                                                let think_elapsed = Self::format_elapsed(*started_at_ms, *finished_at_ms, render_now_ms);
                                                let accent = cx.theme().info;
                                                let status_icon = if is_done {
                                                    IconName::Brain
                                                } else {
                                                    IconName::Loader
                                                };
                                                let header_label = if is_done {
                                                    "Thought"
                                                } else {
                                                    "Thinking\u{2026}"
                                                };

                                                div()
                                                    .relative()
                                                    .w_full()
                                                    .min_w_0()
                                                    .px_3()
                                                    .py_1()
                                                    .child(
                                                        canvas(
                                                            move |bounds, _, cx| {
                                                                panel.update(cx, |panel, cx| {
                                                                    let measured =
                                                                        bounds.size.height;
                                                                    if panel
                                                                        .display_item_heights
                                                                        .get(&ix)
                                                                        .copied()
                                                                        != Some(measured)
                                                                    {
                                                                        panel
                                                                            .display_item_heights
                                                                            .insert(ix, measured);
                                                                        cx.notify();
                                                                    }
                                                                });
                                                            },
                                                            |_, _, _, _| {},
                                                        )
                                                        .absolute()
                                                        .inset_0(),
                                                    )
                                                    .child(
                                                        v_flex()
                                                            .w_full()
                                                            .rounded(px(6.0))
                                                            .border_1()
                                                            .border_color(accent.opacity(0.25))
                                                            .bg(cx.theme().secondary)
                                                            .overflow_hidden()
                                                            .child(
                                                                h_flex()
                                                                    .id(("thinking-header", ix))
                                                                    .w_full()
                                                                    .px_3()
                                                                    .py(px(6.0))
                                                                    .gap_2()
                                                                    .cursor_pointer()
                                                                    .on_click(cx.listener(
                                                                        move |this, _, _, cx| {
                                                                            if let Some(
                                                                                DisplayItem::ThinkingBlock {
                                                                                    is_expanded,
                                                                                    ..
                                                                                },
                                                                            ) = this
                                                                                .display_items
                                                                                .get_mut(ix)
                                                                            {
                                                                                *is_expanded =
                                                                                    !*is_expanded;
                                                                                this.display_item_heights
                                                                                    .remove(&ix);
                                                                            }
                                                                            cx.notify();
                                                                        },
                                                                    ))
                                                                    .child(
                                                                        Icon::new(IconName::Brain)
                                                                            .size_3()
                                                                            .text_color(accent),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .flex_1()
                                                                            .text_xs()
                                                                            .text_color(
                                                                                cx.theme()
                                                                                    .muted_foreground,
                                                                            )
                                                                            .child(header_label),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(accent.opacity(0.7))
                                                                            .font_family("JetBrains Mono")
                                                                            .child(think_elapsed),
                                                                    )
                                                                    .child(
                                                                        Button::new(("thinking-copy", ix))
                                                                            .xsmall()
                                                                            .ghost()
                                                                            .icon(IconName::Copy)
                                                                            .tooltip("Copy full thinking block")
                                                                            .on_click(cx.listener(
                                                                                move |_, _, _, cx| {
                                                                                    cx.write_to_clipboard(
                                                                                        gpui::ClipboardItem::new_string(
                                                                                            copy_content.clone(),
                                                                                        ),
                                                                                    );
                                                                                },
                                                                            )),
                                                                    )
                                                                    .child(
                                                                        Icon::new(status_icon)
                                                                            .size_3()
                                                                            .text_color(accent),
                                                                    )
                                                                    .child(
                                                                        Icon::new(if is_expanded {
                                                                            IconName::ChevronUp
                                                                        } else {
                                                                            IconName::ChevronDown
                                                                        })
                                                                        .size_3()
                                                                        .text_color(
                                                                            cx.theme()
                                                                                .muted_foreground
                                                                                .opacity(0.6),
                                                                        ),
                                                                    ),
                                                            )
                                                            .when(is_expanded && !content.is_empty(), |el| {
                                                                el.child(
                                                                    div()
                                                                        .w_full()
                                                                        .px_3()
                                                                        .py_2()
                                                                        .border_t_1()
                                                                        .border_color(cx.theme().border)
                                                                        .text_xs()
                                                                        .font_family("JetBrains Mono")
                                                                        .text_color(
                                                                            cx.theme().muted_foreground,
                                                                        )
                                                                        .whitespace_normal()
                                                                        .child(content),
                                                                )
                                                            }),
                                                    )
                                                    .into_any_element()
                                            }

                                            DisplayItem::SubagentInvocation {
                                                subagent_id: _,
                                                name,
                                                task,
                                                steps,
                                                is_expanded,
                                                status,
                                                started_at_ms,
                                                finished_at_ms,
                                            } => {
                                                let name = name.clone();
                                                let task = task.clone();
                                                let steps = steps.clone();
                                                let is_expanded = *is_expanded;
                                                let status_val = *status;
                                                let subagent_elapsed = Self::format_elapsed(*started_at_ms, *finished_at_ms, render_now_ms);

                                                let accent = match status_val {
                                                    SubagentStepStatus::Error => cx.theme().danger,
                                                    SubagentStepStatus::Success => cx.theme().success,
                                                    SubagentStepStatus::Running => cx.theme().info,
                                                    SubagentStepStatus::Pending => cx.theme().muted_foreground,
                                                };

                                                let status_icon = match status_val {
                                                    SubagentStepStatus::Error => IconName::CircleX,
                                                    SubagentStepStatus::Success => IconName::CircleCheck,
                                                    SubagentStepStatus::Running | SubagentStepStatus::Pending => IconName::Loader,
                                                };

                                                div()
                                                    .relative()
                                                    .w_full()
                                                    .min_w_0()
                                                    .px_3()
                                                    .py_1()
                                                    .child(
                                                        canvas(
                                                            move |bounds, _, cx| {
                                                                panel.update(cx, |panel, cx| {
                                                                    let measured = bounds.size.height;
                                                                    if panel.display_item_heights.get(&ix).copied() != Some(measured) {
                                                                        panel.display_item_heights.insert(ix, measured);
                                                                        cx.notify();
                                                                    }
                                                                });
                                                            },
                                                            |_, _, _, _| {},
                                                        )
                                                        .absolute()
                                                        .inset_0(),
                                                    )
                                                    .child(
                                                        v_flex()
                                                            .w_full()
                                                            .gap_px()
                                                            .rounded(px(6.0))
                                                            .border_1()
                                                            .border_color(accent.opacity(0.25))
                                                            .bg(cx.theme().secondary)
                                                            .overflow_hidden()
                                                            .child(
                                                                h_flex()
                                                                    .id(("subagent-header", ix))
                                                                    .w_full()
                                                                    .px_3()
                                                                    .py(px(6.0))
                                                                    .gap_2()
                                                                    .cursor_pointer()
                                                                    .on_click(cx.listener(
                                                                        move |this, _, _, cx| {
                                                                            if let Some(
                                                                                DisplayItem::SubagentInvocation {
                                                                                    is_expanded,
                                                                                    ..
                                                                                },
                                                                            ) = this
                                                                                .display_items
                                                                                .get_mut(ix)
                                                                            {
                                                                                *is_expanded = !*is_expanded;
                                                                                this.display_item_heights.remove(&ix);
                                                                            }
                                                                            cx.notify();
                                                                        },
                                                                    ))
                                                                    .child(
                                                                        Icon::new(IconName::GitBranch)
                                                                            .size_3()
                                                                            .text_color(accent),
                                                                    )
                                                                    .child(
                                                                        v_flex()
                                                                            .flex_1()
                                                                            .gap_px()
                                                                            .child(
                                                                                div()
                                                                                    .text_xs()
                                                                                    .font_semibold()
                                                                                    .text_color(cx.theme().foreground)
                                                                                    .child(format!("Subagent: {}", name)),
                                                                            )
                                                                            .child(
                                                                                div()
                                                                                    .text_xs()
                                                                                    .text_color(cx.theme().muted_foreground)
                                                                                    .child(task),
                                                                            ),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(accent.opacity(0.7))
                                                                            .font_family("JetBrains Mono")
                                                                            .child(subagent_elapsed),
                                                                    )
                                                                    .child(
                                                                        Icon::new(status_icon)
                                                                            .size_3()
                                                                            .text_color(accent),
                                                                    )
                                                                    .child(
                                                                        Icon::new(if is_expanded {
                                                                            IconName::ChevronUp
                                                                        } else {
                                                                            IconName::ChevronDown
                                                                        })
                                                                        .size_3()
                                                                        .text_color(cx.theme().muted_foreground.opacity(0.6)),
                                                                    ),
                                                            )
                                                            .when(is_expanded && !steps.is_empty(), |el| {
                                                                el.child(
                                                                    v_flex()
                                                                        .w_full()
                                                                        .gap_px()
                                                                        .children(
                                                                            steps.iter().map(|step| {
                                                                                let step_accent = match step.status {
                                                                                    SubagentStepStatus::Error => cx.theme().danger,
                                                                                    SubagentStepStatus::Success => cx.theme().success,
                                                                                    SubagentStepStatus::Running | SubagentStepStatus::Pending => cx.theme().info,
                                                                                };
                                                                                let step_icon = match step.status {
                                                                                    SubagentStepStatus::Error => IconName::CircleX,
                                                                                    SubagentStepStatus::Success => IconName::CircleCheck,
                                                                                    SubagentStepStatus::Running => IconName::Loader,
                                                                                    SubagentStepStatus::Pending => IconName::Circle,
                                                                                };

                                                                                v_flex()
                                                                                    .w_full()
                                                                                    .px_3()
                                                                                    .py_2()
                                                                                    .gap_1()
                                                                                    .border_t_1()
                                                                                    .border_color(cx.theme().border)
                                                                                    .child(
                                                                                        h_flex()
                                                                                            .gap_2()
                                                                                            .items_center()
                                                                                            .child(
                                                                                                Icon::new(step_icon)
                                                                                                    .size_2()
                                                                                                    .text_color(step_accent),
                                                                                            )
                                                                                            .child(
                                                                                                div()
                                                                                                    .text_xs()
                                                                                                    .font_semibold()
                                                                                                    .text_color(cx.theme().foreground)
                                                                                                    .child(step.description.clone()),
                                                                                            ),
                                                                                    )
                                                                                    .when(!step.details.is_empty(), |el| {
                                                                                        el.child(
                                                                                            div()
                                                                                                .text_xs()
                                                                                                .font_family("JetBrains Mono")
                                                                                                .text_color(cx.theme().muted_foreground.opacity(0.8))
                                                                                                .whitespace_normal()
                                                                                                .child(step.details.clone()),
                                                                                        )
                                                                                    })
                                                                            }),
                                                                        ),
                                                                )
                                                            }),
                                                    )
                                                    .into_any_element()
                                            }

                                            DisplayItem::UserMessage {
                                                content,
                                                message_index,
                                            }
                                            | DisplayItem::AssistantMessage {
                                                content,
                                                message_index,
                                                ..
                                            } => {
                                                let is_user =
                                                    matches!(item, DisplayItem::UserMessage { .. });
                                                let is_streaming = matches!(
                                                    item,
                                                    DisplayItem::AssistantMessage {
                                                        is_streaming: true,
                                                        ..
                                                    }
                                                );
                                                let content = content.clone();
                                                let copy_content = content.clone();
                                                let msg_index = *message_index;
                                                let hover_group =
                                                    format!("agent-chat-msg-hover-{ix}");
                                                let is_confirming_rollback =
                                                    this.pending_rollback_confirm_ix == Some(ix);

                                                div()
                                                    .relative()
                                                    .group(hover_group.clone())
                                                    .w_full()
                                                    .min_w_0()
                                                    .px_3()
                                                    .py_1()
                                                    .child(
                                                        canvas(
                                                            move |bounds, _, cx| {
                                                                panel.update(cx, |panel, cx| {
                                                                    let measured =
                                                                        bounds.size.height;
                                                                    if panel
                                                                        .display_item_heights
                                                                        .get(&ix)
                                                                        .copied()
                                                                        != Some(measured)
                                                                    {
                                                                        panel
                                                                            .display_item_heights
                                                                            .insert(ix, measured);
                                                                        cx.notify();
                                                                    }
                                                                });
                                                            },
                                                            |_, _, _, _| {},
                                                        )
                                                        .absolute()
                                                        .inset_0(),
                                                    )
                                                    .child(
                                                        h_flex()
                                                            .w_full()
                                                            .min_w_0()
                                                            .justify_start()
                                                            .when(is_user, |el| el.justify_end())
                                                            .child(
                                                                v_flex()
                                                                    .w_auto()
                                                                    .max_w(px(620.0))
                                                                    .min_w_0()
                                                                    .gap_1()
                                                                    .px_3()
                                                                    .py_2()
                                                                    .rounded(px(8.0))
                                                                    .bg(if is_user {
                                                                        cx.theme()
                                                                            .primary
                                                                            .opacity(0.16)
                                                                    } else {
                                                                        cx.theme().secondary
                                                                    })
                                                                    .child(
                                                                        div()
                                                                            .text_xs()
                                                                            .font_semibold()
                                                                            .text_color(
                                                                                cx.theme()
                                                                                    .muted_foreground,
                                                                            )
                                                                            .child(if is_user {
                                                                                "You"
                                                                            } else {
                                                                                "Agent"
                                                                            }),
                                                                    )
                                                                    .child(if is_user || is_streaming {
                                                                        div()
                                                                            .w_full()
                                                                            .min_w_0()
                                                                            .whitespace_normal()
                                                                            .text_sm()
                                                                            .text_color(
                                                                                cx.theme().foreground,
                                                                            )
                                                                            .child(content)
                                                                            .into_any_element()
                                                                    } else {
                                                                        TextView::markdown_with_code_font(
                                                                            ("agent-chat-md", ix),
                                                                            content,
                                                                            "JetBrains Mono",
                                                                            window,
                                                                            cx,
                                                                        )
                                                                        .debounce_ms(0)
                                                                        .selectable()
                                                                        .into_any_element()
                                                                    }),
                                                            )
                                                            .id(("agent-chat-message", ix)),
                                                    )
                                                    .child(
                                                        h_flex()
                                                            .absolute()
                                                            .w_full()
                                                            .bottom(px(-8.0))
                                                            .px_6()
                                                            .justify_start()
                                                            .when(is_user, |this| {
                                                                this.justify_end()
                                                            })
                                                            .invisible()
                                                            .group_hover(
                                                                hover_group,
                                                                |this| this.visible(),
                                                            )
                                                            .child(
                                                                h_flex()
                                                                    .gap_1()
                                                                    .p_1()
                                                                    .rounded(px(8.0))
                                                                    .bg(cx.theme().background)
                                                                    .border_1()
                                                                    .border_color(
                                                                        cx.theme().border,
                                                                    )
                                                                    .when(
                                                                        !is_confirming_rollback,
                                                                        |el| {
                                                                            el.child(
                                                                                Button::new((
                                                                                    "agent-chat-rollback",
                                                                                    ix,
                                                                                ))
                                                                                .xsmall()
                                                                                .ghost()
                                                                                .icon(
                                                                                    IconName::Undo,
                                                                                )
                                                                                .tooltip(
                                                                                    "Rollback to this message",
                                                                                )
                                                                                .disabled(
                                                                                    this.is_request_in_flight,
                                                                                )
                                                                                .on_click(
                                                                                    cx.listener(
                                                                                        move |this,
                                                                                              _,
                                                                                              _,
                                                                                              cx| {
                                                                                            this.request_rollback_confirmation(
                                                                                                ix,
                                                                                                cx,
                                                                                            );
                                                                                        },
                                                                                    ),
                                                                                ),
                                                                            )
                                                                        },
                                                                    )
                                                                    .when(
                                                                        is_confirming_rollback,
                                                                        |el| {
                                                                            el.child(
                                                                                Button::new((
                                                                                    "agent-chat-rollback-confirm",
                                                                                    ix,
                                                                                ))
                                                                                .xsmall()
                                                                                .primary()
                                                                                .icon(
                                                                                    IconName::Check,
                                                                                )
                                                                                .tooltip(
                                                                                    "Confirm rollback",
                                                                                )
                                                                                .disabled(
                                                                                    this.is_request_in_flight,
                                                                                )
                                                                                .on_click(
                                                                                    cx.listener(
                                                                                        move |this,
                                                                                              _,
                                                                                              _,
                                                                                              cx| {
                                                                                            this.rollback_chat_to_message(
                                                                                                ix,
                                                                                                msg_index,
                                                                                                cx,
                                                                                            );
                                                                                        },
                                                                                    ),
                                                                                ),
                                                                            )
                                                                            .child(
                                                                                Button::new((
                                                                                    "agent-chat-rollback-cancel",
                                                                                    ix,
                                                                                ))
                                                                                .xsmall()
                                                                                .ghost()
                                                                                .icon(
                                                                                    IconName::Close,
                                                                                )
                                                                                .tooltip(
                                                                                    "Cancel rollback",
                                                                                )
                                                                                .on_click(
                                                                                    cx.listener(
                                                                                        |this,
                                                                                         _,
                                                                                         _,
                                                                                         cx| {
                                                                                            this.cancel_rollback_confirmation(
                                                                                                cx,
                                                                                            );
                                                                                        },
                                                                                    ),
                                                                                ),
                                                                            )
                                                                        },
                                                                    )
                                                                    .child(
                                                                        Button::new((
                                                                            "agent-chat-fork",
                                                                            ix,
                                                                        ))
                                                                        .xsmall()
                                                                        .ghost()
                                                                        .icon(IconName::GitFork)
                                                                        .tooltip(
                                                                            "Fork conversation from here",
                                                                        )
                                                                        .disabled(
                                                                            this.is_request_in_flight,
                                                                        )
                                                                        .on_click(cx.listener(
                                                                            move |this, _, _, cx| {
                                                                                this.fork_chat_here(
                                                                                    ix,
                                                                                    msg_index,
                                                                                    cx,
                                                                                );
                                                                            },
                                                                        )),
                                                                    )
                                                                    .child({
                                                                        Button::new((
                                                                            "agent-chat-copy",
                                                                            ix,
                                                                        ))
                                                                        .xsmall()
                                                                        .ghost()
                                                                        .icon(IconName::Copy)
                                                                        .tooltip("Copy message")
                                                                        .on_click(cx.listener(
                                                                            move |_, _, _, cx| {
                                                                                cx.write_to_clipboard(
                                                                                    gpui::ClipboardItem::new_string(copy_content.clone())
                                                                                );
                                                                            },
                                                                        ))
                                                                    })
                                                                    .when(is_user && !is_confirming_rollback, |el| {
                                                                        el.child(
                                                                            Button::new((
                                                                                "agent-chat-edit",
                                                                                ix,
                                                                            ))
                                                                            .xsmall()
                                                                            .ghost()
                                                                            .icon(IconName::EditPencil)
                                                                            .tooltip("Edit message")
                                                                            .disabled(this.is_request_in_flight)
                                                                            .on_click(cx.listener(
                                                                                move |this, _, window, cx| {
                                                                                    this.edit_user_message(
                                                                                        ix,
                                                                                        msg_index,
                                                                                        window,
                                                                                        cx,
                                                                                    );
                                                                                },
                                                                            ))
                                                                        )
                                                                    })
                                                                    .when(!is_user && ix + 1 == display_count && !is_confirming_rollback, |el| {
                                                                        el.child(
                                                                            Button::new((
                                                                                "agent-chat-regen",
                                                                                ix,
                                                                            ))
                                                                            .xsmall()
                                                                            .ghost()
                                                                            .icon(IconName::Refresh)
                                                                            .tooltip("Regenerate response")
                                                                            .disabled(this.is_request_in_flight)
                                                                            .on_click(cx.listener(
                                                                                |this, _, _, cx| {
                                                                                    this.regenerate_response(cx);
                                                                                },
                                                                            ))
                                                                        )
                                                                    }),
                                                            ),
                                                    )
                                                    .into_any_element()
                                            }
                                        }
                                    })
                                    .collect::<Vec<_>>()
                            },
                        )
                        .track_scroll(&self.messages_scroll_handle)
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .child(Scrollbar::vertical(
                                &self.messages_scroll_state,
                                &self.messages_scroll_handle,
                            )),
                    )
                    .child(
                        canvas(
                            {
                                let panel = cx.entity().clone();
                                move |bounds, _, cx| {
                                    let h = bounds.size.height;
                                    panel.update(cx, |p, cx| {
                                        if p.chat_viewport_height != h {
                                            p.chat_viewport_height = h;
                                            cx.notify();
                                        }
                                    });
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .when(self.render_auto_scroll_safety_net(), |el| {
                        el.child(
                            div()
                                .absolute()
                                .bottom(px(16.0))
                                .right(px(28.0))
                                .child(
                                    Button::new("agent-chat-jump-bottom")
                                        .icon(IconName::ArrowDown)
                                        .xsmall()
                                        .primary()
                                        .tooltip("Jump to bottom (re-enable auto-scroll)")
                                        .on_click(cx.listener(|this, _, _, _cx| {
                                            this.jump_to_bottom();
                                        })),
                                ),
                        )
                    }),
            )
            .child(
                v_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    // Input row: text input + send/stop
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap_2()
                            .items_end()
                            .child(TextInput::new(&self.prompt_input).flex_1().min_w_0())
                            .when(self.is_request_in_flight, |this| {
                                this.child(
                                    h_flex()
                                        .gap_1()
                                        .items_center()
                                        .child(Spinner::new().with_size(Size::Small))
                                        .child(
                                            Button::new("agent-chat-stop")
                                                .icon(IconName::Square)
                                                .xsmall()
                                                .ghost()
                                                .tooltip("Stop generation")
                                                .on_click(cx.listener(|this, _, _, _cx| {
                                                    if let Some(tx) = this.cancel_tx.take() {
                                                        let _ = tx.try_send(());
                                                    }
                                                })),
                                        ),
                                )
                            })
                            .child(
                                Button::new("agent-chat-send")
                                    .icon(IconName::Send)
                                    .label("Send")
                                    .primary()
                                    .tooltip("Send message (Enter)")
                                    .disabled(
                                        self.is_request_in_flight
                                            || self
                                                .prompt_input
                                                .read(cx)
                                                .text()
                                                .to_string()
                                                .trim()
                                                .is_empty(),
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.send_prompt(window, cx);
                                    })),
                            ),
                    )
                    // Subagent indicator: passive dot + count, only when active
                    .when(queued_subagent_count > 0 || self.is_processing_subagent_event, |el| {
                        el.child(
                            h_flex()
                                .w_full()
                                .gap_1()
                                .items_center()
                                .pt(px(2.0))
                                .child(
                                    div()
                                        .size(px(5.0))
                                        .rounded_full()
                                        .bg(if self.is_processing_subagent_event {
                                            cx.theme().info
                                        } else {
                                            cx.theme().warning
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground.opacity(0.7))
                                        .child(format!(
                                            "{} subagent{} queued",
                                            queued_subagent_count,
                                            if queued_subagent_count == 1 { "" } else { "s" },
                                        )),
                                ),
                        )
                    }),
            )
    }
}
