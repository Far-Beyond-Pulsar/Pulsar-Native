//! Renderers for the individual rows of the chat virtual list.
//!
//! Each `render_*_item` function turns one [`DisplayItem`] variant into an
//! element: tool-call groups, compaction summaries, the system-prompt card,
//! thinking blocks, subagent invocations and user/assistant message bubbles.
//! They are dispatched from `AgentChatPanel::render_display_row` in
//! `render_regions.rs`. Every row embeds a measuring `canvas` that reports its
//! laid-out height back into `display_item_heights`.

use super::*;

#[allow(clippy::too_many_arguments)]
impl AgentChatPanel {
    /// Collapsible card listing the tool calls the model made in one turn.
    pub(super) fn render_tool_call_group_item(
        ix: usize,
        calls: &[ToolCallDisplay],
        is_expanded: bool,
        started_at_ms: u64,
        finished_at_ms: Option<u64>,
        render_now_ms: u64,
        panel: Entity<Self>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let calls = calls.to_vec();
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
        let group_elapsed = Self::format_elapsed(started_at_ms, finished_at_ms, render_now_ms);
        let tool_names: Vec<String> = calls.iter().map(|c| c.name.clone()).collect();
        let all_done = calls.iter().all(|c| c.result_preview.is_some());
        let has_error = calls.iter().any(|c| c.is_error);

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
            format!("Used {} tools: {}", calls.len(), tool_names.join(", "))
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
                            .id(("tool-call-header", ix))
                            .w_full()
                            .px_3()
                            .py(px(6.0))
                            .gap_2()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(DisplayItem::ToolCallGroup { is_expanded, .. }) =
                                    this.display_items.get_mut(ix)
                                {
                                    *is_expanded = !*is_expanded;
                                    this.display_item_heights.remove(&ix);
                                }
                                cx.notify();
                            }))
                            .child(Icon::new(IconName::Terminal).size_3().text_color(accent))
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
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
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            copy_tool_block.clone(),
                                        ));
                                    })),
                            )
                            .child(Icon::new(status_icon).size_3().text_color(accent))
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
                    .when(is_expanded, |el| {
                        el.child(
                            v_flex()
                                .w_full()
                                .gap_px()
                                .children(calls.iter().map(|call| {
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
                                        .when_some(call.result_preview.as_ref(), |el, result| {
                                            el.child(
                                                div()
                                                    .text_xs()
                                                    .font_family("JetBrains Mono")
                                                    .text_color(
                                                        cx.theme().muted_foreground.opacity(0.8),
                                                    )
                                                    .child(format!("\u{2192} {result}")),
                                            )
                                        })
                                        .when(call.result_preview.is_none(), |el| {
                                            el.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(
                                                        cx.theme().muted_foreground.opacity(0.5),
                                                    )
                                                    .child("running\u{2026}"),
                                            )
                                        })
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    /// Card shown where older messages were dropped and summarised.
    pub(super) fn render_compaction_summary_item(
        ix: usize,
        summary: &str,
        is_expanded: bool,
        started_at_ms: u64,
        finished_at_ms: Option<u64>,
        render_now_ms: u64,
        panel: Entity<Self>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let summary = summary.to_string();
        let copy_summary = summary.clone();
        let compact_elapsed = Self::format_elapsed(started_at_ms, finished_at_ms, render_now_ms);
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
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(DisplayItem::CompactionSummary {
                                    is_expanded, ..
                                }) = this.display_items.get_mut(ix)
                                {
                                    *is_expanded = !*is_expanded;
                                    this.display_item_heights.remove(&ix);
                                }
                                cx.notify();
                            }))
                            .child(Icon::new(IconName::Scissor).size_3().text_color(accent))
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        "Context compacted \u{2014} earlier messages summarised",
                                    ),
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
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            copy_summary.clone(),
                                        ));
                                    })),
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

    /// Collapsible card showing the system prompt, with an update button when stale.
    pub(super) fn render_system_prompt_item(
        ix: usize,
        content: &str,
        is_expanded: bool,
        is_outdated: bool,
        panel: Entity<Self>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let content = content.to_string();
        let copy_content = content.clone();
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
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(DisplayItem::SystemPrompt { is_expanded, .. }) =
                                    this.display_items.get_mut(ix)
                                {
                                    *is_expanded = !*is_expanded;
                                    this.display_item_heights.remove(&ix);
                                }
                                cx.notify();
                            }))
                            .child(Icon::new(IconName::Settings).size_3().text_color(accent))
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
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            copy_content.clone(),
                                        ));
                                    })),
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

    /// Collapsible card for a `<think>` block (live while the model is thinking).
    pub(super) fn render_thinking_block_item(
        ix: usize,
        content: &str,
        is_expanded: bool,
        is_done: bool,
        started_at_ms: u64,
        finished_at_ms: Option<u64>,
        render_now_ms: u64,
        panel: Entity<Self>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let content = content.to_string();
        let copy_content = content.clone();
        let think_elapsed = Self::format_elapsed(started_at_ms, finished_at_ms, render_now_ms);
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
                            .id(("thinking-header", ix))
                            .w_full()
                            .px_3()
                            .py(px(6.0))
                            .gap_2()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(DisplayItem::ThinkingBlock { is_expanded, .. }) =
                                    this.display_items.get_mut(ix)
                                {
                                    *is_expanded = !*is_expanded;
                                    this.display_item_heights.remove(&ix);
                                }
                                cx.notify();
                            }))
                            .child(Icon::new(IconName::Brain).size_3().text_color(accent))
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
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
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            copy_content.clone(),
                                        ));
                                    })),
                            )
                            .child(Icon::new(status_icon).size_3().text_color(accent))
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

    /// Collapsible card for a subagent run, with its nested steps.
    pub(super) fn render_subagent_invocation_item(
        ix: usize,
        name: &str,
        task: &str,
        steps: &[SubagentStepDisplay],
        is_expanded: bool,
        status: SubagentStepStatus,
        started_at_ms: u64,
        finished_at_ms: Option<u64>,
        render_now_ms: u64,
        panel: Entity<Self>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = name.to_string();
        let task = task.to_string();
        let steps = steps.to_vec();
        let status_val = status;
        let subagent_elapsed = Self::format_elapsed(started_at_ms, finished_at_ms, render_now_ms);

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
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(DisplayItem::SubagentInvocation {
                                    is_expanded, ..
                                }) = this.display_items.get_mut(ix)
                                {
                                    *is_expanded = !*is_expanded;
                                    this.display_item_heights.remove(&ix);
                                }
                                cx.notify();
                            }))
                            .child(Icon::new(IconName::GitBranch).size_3().text_color(accent))
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
                            .child(Icon::new(status_icon).size_3().text_color(accent))
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
                                .children(steps.iter().map(|step| {
                                    let step_accent = match step.status {
                                        SubagentStepStatus::Error => cx.theme().danger,
                                        SubagentStepStatus::Success => cx.theme().success,
                                        SubagentStepStatus::Running
                                        | SubagentStepStatus::Pending => cx.theme().info,
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
                                                    .text_color(
                                                        cx.theme().muted_foreground.opacity(0.8),
                                                    )
                                                    .whitespace_normal()
                                                    .child(step.details.clone()),
                                            )
                                        })
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    /// User or assistant message bubble with its hover toolbar
    /// (rollback, fork, copy, edit, regenerate).
    pub(super) fn render_message_item(
        &self,
        ix: usize,
        display_count: usize,
        is_user: bool,
        is_streaming: bool,
        content: &str,
        message_index: usize,
        panel: Entity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = self;
        let content = content.to_string();
        let copy_content = content.clone();
        let msg_index = message_index;
        let hover_group = format!("agent-chat-msg-hover-{ix}");
        let is_confirming_rollback = this.pending_rollback_confirm_ix == Some(ix);

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
                h_flex()
                    .w_full()
                    .min_w_0()
                    .justify_start()
                    .when(is_user, |el| el.justify_end())
                    .child(
                        Message::new()
                            .alignment(if is_user {
                                MessageAlignment::End
                            } else {
                                MessageAlignment::Start
                            })
                            .content(
                                MessageContent::new().bubble(
                                    Bubble::new()
                                        .with_variant(if is_user {
                                            BubbleVariant::Tinted
                                        } else {
                                            BubbleVariant::Secondary
                                        })
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_semibold()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(if is_user { "You" } else { "Agent" }),
                                        )
                                        .child(if is_user || is_streaming {
                                            div()
                                                .w_full()
                                                .min_w_0()
                                                .whitespace_normal()
                                                .text_sm()
                                                .text_color(cx.theme().foreground)
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
                                ),
                            ),
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
                    .when(is_user, |this| this.justify_end())
                    .invisible()
                    .group_hover(hover_group, |this| this.visible())
                    .child(
                        h_flex()
                            .gap_1()
                            .p_1()
                            .rounded(px(8.0))
                            .bg(cx.theme().background)
                            .border_1()
                            .border_color(cx.theme().border)
                            .when(!is_confirming_rollback, |el| {
                                el.child(
                                    Button::new(("agent-chat-rollback", ix))
                                        .xsmall()
                                        .ghost()
                                        .icon(IconName::Undo)
                                        .tooltip("Rollback to this message")
                                        .disabled(this.is_request_in_flight)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.request_rollback_confirmation(ix, cx);
                                        })),
                                )
                            })
                            .when(is_confirming_rollback, |el| {
                                el.child(
                                    Button::new(("agent-chat-rollback-confirm", ix))
                                        .xsmall()
                                        .primary()
                                        .icon(IconName::Check)
                                        .tooltip("Confirm rollback")
                                        .disabled(this.is_request_in_flight)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.rollback_chat_to_message(ix, msg_index, cx);
                                        })),
                                )
                                .child(
                                    Button::new(("agent-chat-rollback-cancel", ix))
                                        .xsmall()
                                        .ghost()
                                        .icon(IconName::Close)
                                        .tooltip("Cancel rollback")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_rollback_confirmation(cx);
                                        })),
                                )
                            })
                            .child(
                                Button::new(("agent-chat-fork", ix))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::GitFork)
                                    .tooltip("Fork conversation from here")
                                    .disabled(this.is_request_in_flight)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.fork_chat_here(ix, msg_index, cx);
                                    })),
                            )
                            .child({
                                Button::new(("agent-chat-copy", ix))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Copy)
                                    .tooltip("Copy message")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            copy_content.clone(),
                                        ));
                                    }))
                            })
                            .when(is_user && !is_confirming_rollback, |el| {
                                el.child(
                                    Button::new(("agent-chat-edit", ix))
                                        .xsmall()
                                        .ghost()
                                        .icon(IconName::EditPencil)
                                        .tooltip("Edit message")
                                        .disabled(this.is_request_in_flight)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.edit_user_message(ix, msg_index, window, cx);
                                        })),
                                )
                            })
                            .when(
                                !is_user && ix + 1 == display_count && !is_confirming_rollback,
                                |el| {
                                    el.child(
                                        Button::new(("agent-chat-regen", ix))
                                            .xsmall()
                                            .ghost()
                                            .icon(IconName::Refresh)
                                            .tooltip("Regenerate response")
                                            .disabled(this.is_request_in_flight)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.regenerate_response(cx);
                                            })),
                                    )
                                },
                            ),
                    ),
            )
            .into_any_element()
    }
}
