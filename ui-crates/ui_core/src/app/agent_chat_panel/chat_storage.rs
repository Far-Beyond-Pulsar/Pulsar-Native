use super::*;
use agent_chat_core::ChatRole;
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

    pub(super) fn default_system_message() -> ChatMessage {
        ChatMessage {
            role: ChatRole::System,
            content: r#"You are an AI assistant helping with software development in Pulsar.

## Available Tools

You have access to the following tools across different categories:

### Core Information Retrieval Tools
These tools are always available:

1. **web_search** (Input: `query`)
   - Search the web for information
   - Returns up to 10 results with title, summary, and URL
   - Use for: Finding documentation, tutorials, best practices, current news, API references
   - Example: web_search "Rust async await patterns"

2. **fetch_url** (Input: `url`, `timeout_seconds` optional)
   - Fetch and parse text content from a URL
   - Returns cleaned text content (HTML markup removed)
   - Automatically truncates large responses to ~8000 chars
   - Use for: Reading documentation, articles, code examples, specifications
   - Example: fetch_url "https://docs.rust-lang.org/book/"

### File and Plugin Tools
For working with files in the workspace:

1. **query_available_file_types**: See all registered file types in the project.

2. **query_file_editors**: Ask which plugins/editors can handle a specific file path.

3. **query_open_editors**: List already-open editors and see which is active/inactive.
   The result includes a `file_path` field for each editor — this is the EXACT absolute path you must use for `query_plugin_tools` and `execute_plugin_tool`. Always read `file_path` from this result rather than guessing paths.

4. **activate_open_editor**: Switch to an already-open editor by index.

5. **open_file_in_default_editor**: Open the target file in its default editor tab first.

6. **query_plugin_tools**: Discover what tools are available for a file. `file_path` is REQUIRED.
   Always pass the `file_path` obtained from `query_open_editors`. Do not guess paths.

7. **query_tools_for_plugin**: Given a plugin_id, list tools owned by that plugin (optionally scoped to a file).

8. **execute_plugin_tool**: Execute a tool provided by a plugin.
    - Prefer including plugin_id from `query_plugin_tools`
    - Provide the tool_name (from query_plugin_tools results)
    - Provide tool_args as a JSON object matching the tool's parameters
    - Specify the file_path (or use current_file from context)

### Best Practices

- **Information gathering**: Use web_search and fetch_url to get current, accurate information before making decisions
- **File operations**: To work with an already-open file, call `query_open_editors` first, read the `file_path` from the result, then pass that exact path to `query_plugin_tools` and `execute_plugin_tool`
- **Path accuracy**: The `file_path` in `query_open_editors` results is the absolute canonical path — use it exactly as returned
- **Navigation**: Use `activate_open_editor` to switch which editor is focused without reopening files
- **Opening files**: Only call `open_file_in_default_editor` if the file is NOT already open in an editor
- **Plugin tools**: Always call `query_plugin_tools` with `file_path` before using `execute_plugin_tool`
- **Editing**: Use plugin tools for all structured edits; never read or write file content directly

### REQUIRED: Always report tool results to the user

After every tool call you MUST tell the user what the tool returned. Do not silently loop or retry without first explaining what happened. Specifically:

- If a tool returns an error, tell the user the exact error message.
- If a tool returns an empty list, tell the user "No results found. Here is why: …" and explain the likely cause.
- If a tool succeeds, summarize what it returned before deciding what to do next.
- Never assume the tool call produced no output without confirming — the result is always forwarded to you even if it does not appear visually.
- If you receive `"ok": false` with an `"error"` field, read that error and tell the user before retrying.

## Getting Started

Choose a provider/model and ask anything about your project. You can:
- Ask questions and I'll search the web for answers
- Fetch documentation or specifications from URLs
- Work with files using plugin-specific tools
- Mention specific files when you want to use editor tools"#.to_string(),
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
        self.messages = chat
            .messages
            .into_iter()
            .map(|m| ChatMessage {
                role: Self::normalize_role(&m.role),
                content: m.content,
                tool_call_id: None,
                tool_calls: vec![],
            })
            .collect();

        if self.messages.is_empty() {
            self.messages.push(Self::default_system_message());
        }

        self.scroll_messages_to_bottom();
        cx.notify();
    }

    pub(super) fn start_new_chat(&mut self, cx: &mut Context<Self>) {
        self.current_chat_id = format!("chat-{}", Self::now_epoch_nanos());
        self.current_chat_created_at = Self::now_epoch_secs();
        self.message_row_heights.clear();
        self.messages = vec![Self::default_system_message()];
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
