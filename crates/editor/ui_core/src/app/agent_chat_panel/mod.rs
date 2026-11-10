mod auth;
mod builder;
mod chat_history;
mod chat_storage;
mod context;
mod custom_provider_wizard;
mod editor_llm;
mod panel;
mod prompt_ranking;
mod provider_catalog;
mod provider_selection;
mod render;
mod render_header;
mod render_items;
mod render_overlays;
mod render_regions;
mod request_prep;
mod stream_event;
mod stream_tools;
mod stream_worker;
mod streaming;
mod subagent;
mod think_parser;
pub mod types;

pub use panel::AgentChatPanel;
pub(crate) use panel::SubagentCompletionMode;
pub use types::*;

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
    bubble::{Bubble, BubbleVariant},
    button::{Button, ButtonVariants as _},
    dock::{DockArea, DockItem, Panel, PanelEvent, TabPanel},
    dropdown::{
        SearchableList, SearchableListEvent, SearchableListItemAction, SearchableListItemState,
    },
    h_flex,
    input::Enter,
    input::{InputState, TextInput},
    message::{Message, MessageAlignment, MessageContent},
    popover::Popover,
    scroll::{Scrollbar, ScrollbarState},
    spinner::Spinner,
    text::TextView,
    v_flex, v_virtual_list, ActiveTheme as _, Disableable, Icon, IconName, Sizable, Size,
    StyledExt, VirtualListScrollHandle,
};

impl Render for AgentChatPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.maybe_reload_chats_from_disk(cx);
        self.poll_subagent_completion_events(cx);

        let display_count = self.display_items.len();
        let render_now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let display_item_sizes = self.build_display_item_sizes();

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .on_action(cx.listener(Self::on_prompt_enter))
            .child(self.render_top_bar(cx))
            .child(self.render_messages_area(display_item_sizes, display_count, render_now_ms, cx))
            .child(self.render_input_area(cx))
    }
}
