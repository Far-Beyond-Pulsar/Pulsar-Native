use super::*;
use agent_chat_core::ChatRole;
use agent_chat_tools::ToolRegistry;
use engine_state;
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

impl AgentChatPanel {
    pub(super) fn chats_dir() -> Option<PathBuf> {
        let project_root = engine_state::get_project_path().map(PathBuf::from)?;
        Some(project_root.join(".pulsar").join("chats"))
    }

    pub(super) fn ensure_chats_dir() -> Option<PathBuf> {
        let dir = Self::chats_dir()?;
        if fs::create_dir_all(&dir).is_ok() {
            Some(dir)
        } else {
            None
        }
    }

    pub(super) fn chat_file_path(chat_id: &str) -> Option<PathBuf> {
        Some(Self::ensure_chats_dir()?.join(format!("{chat_id}.json")))
    }

    pub(super) fn now_epoch_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    pub(super) fn now_epoch_nanos() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }

    pub(super) fn normalize_role(role: &str) -> ChatRole {
        match role {
            "user" => ChatRole::User,
            "assistant" => ChatRole::Assistant,
            "system" => ChatRole::System,
            "tool" => ChatRole::Tool,
            _ => ChatRole::Assistant,
        }
    }

    pub(super) fn default_system_message(tool_registry: &ToolRegistry) -> ChatMessage {
        let tool_docs = tool_registry.system_prompt_tool_docs();
        let content = format!(
            "You are an AI assistant integrated into Pulsar, a software development environment.\n\
Use your tools whenever they help answer the user's question.\n\
Tool usage is displayed automatically in the UI — do not narrate or repeat tool call details in your text responses.\n\
For file operations call query_open_editors first and use the exact file_path it returns.\n\n\
{tool_docs}\n\n\
Plugin-provided tools are discovered dynamically via query_plugin_tools — call that first whenever you want to edit a file."
        );
        ChatMessage {
            role: ChatRole::System,
            content,
            tool_call_id: None,
            tool_calls: vec![],
        }
    }

    pub(super) fn inferred_chat_title(messages: &[ChatMessage]) -> String {
        if let Some(user_message) = messages.iter().find(|m| m.role == ChatRole::User) {
            user_message
                .content
                .chars()
                .take(60)
                .collect::<String>()
                .trim()
                .to_string()
        } else {
            "New Chat".to_string()
        }
    }

    pub(super) fn read_chat_index() -> Vec<ChatHistoryEntry> {
        let Some(dir) = Self::ensure_chats_dir() else {
            return Vec::new();
        };

        let mut entries = Vec::new();
        let Ok(files) = fs::read_dir(dir) else {
            return entries;
        };

        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }

            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(chat) = serde_json::from_str::<ChatSessionFile>(&raw) else {
                continue;
            };

            entries.push(ChatHistoryEntry {
                id: chat.id,
                title: chat.title,
                updated_at: chat.updated_at,
            });
        }

        entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        entries
    }

    pub(super) fn save_current_chat(&self) {
        if self.current_chat_id.is_empty() {
            return;
        }

        let Some(path) = Self::chat_file_path(&self.current_chat_id) else {
            return;
        };

        let payload = ChatSessionFile {
            id: self.current_chat_id.clone(),
            title: Self::inferred_chat_title(&self.messages),
            created_at: self.current_chat_created_at,
            updated_at: Self::now_epoch_secs(),
            messages: self
                .messages
                .iter()
                .map(|m| PersistedChatMessage {
                    role: match m.role {
                        ChatRole::User => "user",
                        ChatRole::Assistant => "assistant",
                        ChatRole::System => "system",
                        ChatRole::Tool => "tool",
                    }
                    .to_string(),
                    content: m.content.clone(),
                })
                .collect(),
        };

        if let Ok(serialized) = serde_json::to_string_pretty(&payload) {
            let _ = fs::write(path, serialized);
        }
    }

    pub(super) fn refresh_chat_history_list(&mut self, cx: &mut Context<Self>) {
        let entries = Self::read_chat_index();
        self.chat_history_list.update(cx, |list, cx| {
            list.set_items(entries, cx);
        });
    }

    pub(super) fn load_chat_session(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        let Some(path) = Self::chat_file_path(chat_id) else {
            return;
        };
        let Ok(raw) = fs::read_to_string(path) else {
            return;
        };
        let Ok(chat) = serde_json::from_str::<ChatSessionFile>(&raw) else {
            return;
        };

        self.current_chat_id = chat.id;
        self.current_chat_created_at = chat.created_at;
        self.message_row_heights.clear();
        self.display_item_heights.clear();
        self.streaming_display_item_ix = None;

        let messages: Vec<ChatMessage> = chat
            .messages
            .into_iter()
            .map(|m| ChatMessage {
                role: Self::normalize_role(&m.role),
                content: m.content,
                tool_call_id: None,
                tool_calls: vec![],
            })
            .collect();

        self.display_items = messages
            .iter()
            .enumerate()
            .filter_map(|(ix, m)| match m.role {
                ChatRole::User => Some(DisplayItem::UserMessage {
                    content: m.content.clone(),
                    message_index: ix,
                }),
                ChatRole::Assistant => Some(DisplayItem::AssistantMessage {
                    content: m.content.clone(),
                    message_index: ix,
                    is_streaming: false,
                }),
                _ => None,
            })
            .collect();

        self.messages = messages;
        if self.messages.is_empty() {
            self.messages.push(Self::default_system_message(&self.tool_registry));
        }

        self.scroll_messages_to_bottom();
        cx.notify();
    }

    pub(super) fn start_new_chat(&mut self, cx: &mut Context<Self>) {
        self.current_chat_id = format!("chat-{}", Self::now_epoch_nanos());
        self.current_chat_created_at = Self::now_epoch_secs();
        self.message_row_heights.clear();
        self.display_item_heights.clear();
        self.display_items.clear();
        self.streaming_display_item_ix = None;
        self.messages = vec![Self::default_system_message(&self.tool_registry)];
        self.save_current_chat();
        self.refresh_chat_history_list(cx);
        self.scroll_messages_to_bottom();
        cx.notify();
    }

    pub(super) fn bootstrap_chat_storage(&mut self, cx: &mut Context<Self>) {
        let entries = Self::read_chat_index();
        self.chat_history_list.update(cx, |list, cx| {
            list.set_items(entries.clone(), cx);
        });

        if let Some(latest) = entries.first() {
            self.load_chat_session(&latest.id, cx);
        } else {
            self.start_new_chat(cx);
        }

        self.loaded_chat_project_root = engine_state::get_project_path().map(PathBuf::from);
    }

    pub(super) fn maybe_reload_chats_from_disk(&mut self, cx: &mut Context<Self>) {
        let current_root = engine_state::get_project_path().map(PathBuf::from);
        if current_root.is_none() {
            return;
        }

        if self.loaded_chat_project_root != current_root {
            self.bootstrap_chat_storage(cx);
        }
    }

    pub(super) fn export_current_chat(&self) {
        let Some(path) = Self::chat_file_path(&self.current_chat_id) else {
            return;
        };
        let Ok(raw) = fs::read_to_string(&path) else {
            return;
        };

        let file = rfd::FileDialog::new()
            .set_file_name(format!(
                "{}.json",
                Self::inferred_chat_title(&self.messages)
            ))
            .add_filter("JSON Chat Files", &["json"])
            .add_filter("All Files", &["*"])
            .save_file();

        if let Some(save_path) = file {
            let _ = fs::write(save_path, raw);
        }
    }

    pub(super) fn export_all_chats(&self) {
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };

        if let Some(chats_dir) = Self::chats_dir() {
            if let Ok(entries) = fs::read_dir(&chats_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                        if let Ok(content) = fs::read(&path) {
                            if let Some(file_name) = path.file_name() {
                                let dest_path = dir.join(file_name);
                                let _ = fs::write(dest_path, &content);
                            }
                        }
                    }
                }
            }
        }
    }

    pub(super) fn import_chat(&mut self, cx: &mut Context<Self>) {
        let file = rfd::FileDialog::new()
            .add_filter("JSON Chat Files", &["json"])
            .add_filter("All Files", &["*"])
            .pick_file();

        if let Some(file_path) = file {
            if let Ok(raw) = fs::read_to_string(&file_path) {
                if let Ok(chat) = serde_json::from_str::<ChatSessionFile>(&raw) {
                    if let Some(save_path) = Self::chat_file_path(&chat.id) {
                        if let Ok(serialized) = serde_json::to_string_pretty(&chat) {
                            let _ = fs::write(save_path, serialized);
                            self.refresh_chat_history_list(cx);
                            self.load_chat_session(&chat.id, cx);
                        }
                    }
                }
            }
        }
    }
}
