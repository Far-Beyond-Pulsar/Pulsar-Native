use super::*;
use agent_chat_core::ChatRole;
use agent_chat_tools::ToolRegistry;
use editor_task_queue::{TaskDescription, TaskDuration};
use engine_fs::{virtual_fs as vfs, FsChangeKind, FsProvider};
use engine_state;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

// Keep the active write in the FIFO until it is durable, so a chat opened
// immediately after a save can still read the newest snapshot.
static CHAT_SAVES: Mutex<VecDeque<PendingSave>> = Mutex::new(VecDeque::new());

// A queued task can be cancelled before its closure runs. Release the FIFO
// in that case and on panic so the next save can start a fresh worker.
struct SaveDrain(bool);

impl Drop for SaveDrain {
    fn drop(&mut self) {
        if self.0 {
            CHAT_SAVES.lock().unwrap().clear();
        }
    }
}

struct PendingSave {
    provider: Arc<dyn FsProvider>,
    path: PathBuf,
    chat: Arc<ChatSessionFile>,
}

fn pending_chat(path: &Path) -> Option<Arc<ChatSessionFile>> {
    let provider = vfs::provider();
    CHAT_SAVES
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|save| save.path == path && Arc::ptr_eq(&save.provider, &provider))
        .map(|save| save.chat.clone())
}

fn queue_chat_save(provider: Arc<dyn FsProvider>, path: PathBuf, chat: ChatSessionFile) {
    let mut saves = CHAT_SAVES.lock().unwrap();
    let start_worker = saves.is_empty();
    saves.push_back(PendingSave {
        provider,
        path,
        chat: Arc::new(chat),
    });
    drop(saves);
    if !start_worker {
        return;
    }

    let mut drain = SaveDrain(true);
    editor_task_queue::global().submit(
        TaskDescription::new("Save AI chats", "AI chat", TaskDuration::Short),
        move |task| {
            let drain = &mut drain;
            let mut failure = None;
            loop {
                let (provider, path, chat) = {
                    let saves = CHAT_SAVES.lock().unwrap();
                    let save = saves.front().expect("the drain owns the nonempty queue");
                    (save.provider.clone(), save.path.clone(), save.chat.clone())
                };
                task.set_phase("Serializing chat");
                let result = serde_json::to_string_pretty(&*chat)
                    .map_err(anyhow::Error::from)
                    .and_then(|serialized| {
                        task.set_phase("Writing chat file");
                        let dir = path.parent().expect("chat path has a parent");
                        provider.create_dir_all(dir)?;
                        write_chat_file(&provider, &path, &serialized)
                    });
                if let Err(error) = result {
                    let error = format!("Could not save chat to {}: {error}", path.display());
                    tracing::warn!("{error}");
                    failure = Some(error);
                }
                let mut saves = CHAT_SAVES.lock().unwrap();
                saves.pop_front();
                if saves.is_empty() {
                    drain.0 = false;
                    return failure.map_or(Ok(()), Err);
                }
            }
        },
    );
}

impl AgentChatPanel {
    pub(super) fn chats_dir() -> Option<PathBuf> {
        let project_root = engine_state::get_project_path().map(PathBuf::from)?;
        Some(project_root.join(".pulsar").join("chats"))
    }

    pub(super) fn ensure_chats_dir() -> Option<PathBuf> {
        let dir = Self::chats_dir()?;
        if vfs::create_dir_all(&dir).is_ok() {
            Some(dir)
        } else {
            None
        }
    }

    /// Where the chat `chat_id` is stored. `None` for an id that is not a
    /// plain file name (see [`is_valid_chat_id`]): ids also come from file
    /// contents, and must never name a path outside the chats folder.
    pub(super) fn chat_file_path(chat_id: &str) -> Option<PathBuf> {
        if !is_valid_chat_id(chat_id) {
            tracing::warn!("Refusing chat id {chat_id:?}: not a plain file name");
            return None;
        }
        Some(Self::chats_dir()?.join(format!("{chat_id}.json")))
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
            "agent_event" => ChatRole::AgentEvent,
            _ => ChatRole::Assistant,
        }
    }

    /// Build the system-prompt `DisplayItem` that heads every chat.
    /// `is_outdated` is true when the stored message differs from the current default.
    pub(super) fn system_prompt_display_item(content: &str, current_default: &str) -> DisplayItem {
        DisplayItem::SystemPrompt {
            content: content.to_string(),
            is_expanded: false,
            is_outdated: content != current_default,
        }
    }

    /// Replace the stored system message with the current default and refresh the card.
    pub(super) fn apply_system_prompt_update(&mut self, cx: &mut Context<Self>) {
        let new_msg = Self::default_system_message(&self.tool_registry);
        // Update provider history
        if let Some(msg) = self
            .messages
            .iter_mut()
            .find(|m| m.role == ChatRole::System)
        {
            msg.content = new_msg.content.clone();
        }
        // Refresh the card at the top of display_items
        if let Some(DisplayItem::SystemPrompt {
            content,
            is_outdated,
            ..
        }) = self.display_items.first_mut()
        {
            *content = new_msg.content;
            *is_outdated = false;
        }
        self.display_item_heights.remove(&0);
        self.save_current_chat(cx);
        cx.notify();
    }

    pub(super) fn default_system_message(tool_registry: &ToolRegistry) -> ChatMessage {
        let tool_docs = tool_registry.system_prompt_section();
        let content = format!(
            "You are an AI assistant integrated into Pulsar, a software development environment.\n\
Use your tools whenever they help answer the user's question.\n\
Tool usage is displayed automatically in the UI — do not narrate or repeat tool call details in your text responses.\n\
    For file operations call query_open_editors first and use the exact file_path it returns.\n\
    Use cd to change the AI working directory, list_files to inspect directories, and tree for bounded recursive directory views.\n\n\
## Task Tracking\n\
Use task_list_read and task_list_update to maintain a task list for any multi-step work. \
Read it at the start of complex tasks and update it as steps complete. \
The list is injected into every turn so you stay oriented even after context compaction.\n\n\
{tool_docs}\n\n\
Plugin-provided tools are discovered dynamically via query_plugin_tools — call that first whenever you want to edit a file.\n\
Plugin-provided tool execution should use call_plugin_tool with an explicit file_path and plugin_id for reliable, unambiguous routing."
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
        let Ok(files) = vfs::list_dir(&dir) else {
            return entries;
        };

        for file in files {
            let path = dir.join(&file.name);
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }

            // The entry's id is the file's name, not the id written inside
            // it: the id is what the chat is opened and saved by, so it must
            // be the file it came from, never a path of the file's choosing.
            let Some(id) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .filter(|stem| is_valid_chat_id(stem))
                .map(str::to_string)
            else {
                continue;
            };

            let Ok(raw) = vfs::read_to_string(&path) else {
                continue;
            };
            let Ok(chat) = serde_json::from_str::<ChatSessionFile>(&raw) else {
                continue;
            };

            entries.push(ChatHistoryEntry {
                id,
                title: chat.title,
                updated_at: chat.updated_at,
            });
        }

        entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        entries
    }

    pub(super) fn save_current_chat(&mut self, cx: &mut Context<Self>) {
        if self.current_chat_id.is_empty() {
            return;
        }

        let Some(project_root) = &self.loaded_chat_project_root else {
            return;
        };
        let path = project_root
            .join(".pulsar")
            .join("chats")
            .join(format!("{}.json", self.current_chat_id));

        // Snapshot display items: strip ephemeral state and SystemPrompt (reconstructed on load).
        let persisted_display_items: Vec<DisplayItem> = self
            .display_items
            .iter()
            .filter(|item| !matches!(item, DisplayItem::SystemPrompt { .. }))
            .map(|item| match item {
                DisplayItem::AssistantMessage {
                    content,
                    message_index,
                    ..
                } => DisplayItem::AssistantMessage {
                    content: content.clone(),
                    message_index: *message_index,
                    is_streaming: false,
                },
                DisplayItem::ThinkingBlock {
                    content,
                    started_at_ms,
                    finished_at_ms,
                    ..
                } => DisplayItem::ThinkingBlock {
                    content: content.clone(),
                    is_expanded: false, // always collapsed on disk
                    is_done: true,
                    started_at_ms: *started_at_ms,
                    finished_at_ms: *finished_at_ms,
                },
                DisplayItem::SubagentInvocation {
                    subagent_id,
                    name,
                    task,
                    steps,
                    started_at_ms,
                    finished_at_ms,
                    status,
                    ..
                } => DisplayItem::SubagentInvocation {
                    subagent_id: subagent_id.clone(),
                    name: name.clone(),
                    task: task.clone(),
                    steps: steps.clone(),
                    is_expanded: false, // always collapsed on disk
                    status: *status,
                    started_at_ms: *started_at_ms,
                    finished_at_ms: *finished_at_ms,
                },
                other => other.clone(),
            })
            .collect();

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
                        ChatRole::AgentEvent => "agent_event",
                    }
                    .to_string(),
                    content: m.content.clone(),
                })
                .collect(),
            display_items: persisted_display_items,
        };

        self.update_chat_history_entry(&payload, cx);
        queue_chat_save(self.chat_storage_provider.clone(), path, payload);
    }

    fn update_chat_history_entry(&mut self, chat: &ChatSessionFile, cx: &mut Context<Self>) {
        self.chat_history_entries
            .retain(|entry| entry.id != chat.id);
        self.chat_history_entries.push(ChatHistoryEntry {
            id: chat.id.clone(),
            title: chat.title.clone(),
            updated_at: chat.updated_at,
        });
        self.chat_history_entries
            .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        self.chat_history_list.update(cx, |list, cx| {
            list.set_items(self.chat_history_entries.clone(), cx);
        });
    }

    pub(super) fn load_chat_session(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        if self.is_request_in_flight {
            return;
        }
        let Some(path) = Self::chat_file_path(chat_id) else {
            return;
        };
        let chat = if let Some(chat) = pending_chat(&path) {
            (*chat).clone()
        } else {
            let Ok(raw) = vfs::read_to_string(&path) else {
                return;
            };
            let Ok(chat) = serde_json::from_str::<ChatSessionFile>(&raw) else {
                return;
            };
            chat
        };

        // The id the chat was opened by (a checked file name), not the one
        // written inside the file.
        self.current_chat_id = chat_id.to_string();
        self.current_chat_created_at = chat.created_at;
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

        // Use saved display items if present; reconstruct from messages for old files.
        // Either way, strip any stale SystemPrompt card — we rebuild it below.
        let mut display_items: Vec<DisplayItem> = if !chat.display_items.is_empty() {
            chat.display_items
                .into_iter()
                .filter(|item| !matches!(item, DisplayItem::SystemPrompt { .. }))
                .collect()
        } else {
            messages
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
                .collect()
        };

        // Always prepend a fresh system-prompt card.
        let current_default = Self::default_system_message(&self.tool_registry);
        let stored_content = messages
            .iter()
            .find(|m| m.role == ChatRole::System)
            .map(|m| m.content.as_str())
            .unwrap_or("");
        display_items.insert(
            0,
            Self::system_prompt_display_item(stored_content, &current_default.content),
        );

        self.display_items = display_items;
        self.messages = messages;
        if self.messages.is_empty() {
            self.messages
                .push(Self::default_system_message(&self.tool_registry));
        }

        self.scroll_messages_to_bottom();
        cx.notify();
    }

    pub(super) fn start_new_chat(&mut self, cx: &mut Context<Self>) {
        if self.is_request_in_flight {
            return;
        }
        self.current_chat_id = format!("chat-{}", Self::now_epoch_nanos());
        self.current_chat_created_at = Self::now_epoch_secs();
        self.display_item_heights.clear();
        self.streaming_display_item_ix = None;
        let system_msg = Self::default_system_message(&self.tool_registry);
        self.display_items = vec![DisplayItem::SystemPrompt {
            content: system_msg.content.clone(),
            is_expanded: false,
            is_outdated: false,
        }];
        self.messages = vec![system_msg];
        self.save_current_chat(cx);
        self.scroll_messages_to_bottom();
        cx.notify();
    }

    pub(super) fn bootstrap_chat_storage(&mut self, cx: &mut Context<Self>) {
        self.loaded_chat_project_root = engine_state::get_project_path().map(PathBuf::from);
        self.chat_storage_provider = vfs::provider();
        let entries = Self::read_chat_index();
        self.chat_history_entries = entries.clone();
        self.chat_history_list.update(cx, |list, cx| {
            list.set_items(entries.clone(), cx);
        });

        if let Some(latest) = entries.first() {
            self.load_chat_session(&latest.id, cx);
        } else {
            self.start_new_chat(cx);
        }
    }

    pub(super) fn maybe_reload_chats_from_disk(&mut self, cx: &mut Context<Self>) {
        let current_root = engine_state::get_project_path().map(PathBuf::from);
        if current_root.is_none() || self.is_request_in_flight {
            return;
        }

        if self.loaded_chat_project_root != current_root {
            self.bootstrap_chat_storage(cx);
        }
    }

    // Allowed std::fs: the destination is a path the user picked in a native
    // save dialog on this machine, not a project file.
    #[allow(clippy::disallowed_methods)]
    pub(super) fn export_current_chat(&self) {
        let Some(path) = Self::chat_file_path(&self.current_chat_id) else {
            return;
        };
        let raw = if let Some(chat) = pending_chat(&path) {
            let Ok(raw) = serde_json::to_string_pretty(&*chat) else {
                return;
            };
            raw
        } else {
            let Ok(raw) = vfs::read_to_string(&path) else {
                return;
            };
            raw
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
            if let Err(error) = std::fs::write(&save_path, raw) {
                tracing::warn!("Could not export chat to {}: {error}", save_path.display());
            }
        }
    }

    // Allowed std::fs: the source is a file picked in a native open dialog on
    // this machine, not a project file; the copy into the project uses the VFS.
    #[allow(clippy::disallowed_methods)]
    pub(super) fn import_chat(&mut self, cx: &mut Context<Self>) {
        if self.is_request_in_flight {
            return;
        }
        let file = rfd::FileDialog::new()
            .add_filter("JSON Chat Files", &["json"])
            .add_filter("All Files", &["*"])
            .pick_file();

        let Some(file_path) = file else {
            return;
        };
        let raw = match std::fs::read_to_string(&file_path) {
            Ok(raw) => raw,
            Err(error) => {
                tracing::warn!("Could not read {}: {error}", file_path.display());
                return;
            }
        };
        let mut chat = match serde_json::from_str::<ChatSessionFile>(&raw) {
            Ok(chat) => chat,
            Err(error) => {
                tracing::warn!("{} is not a chat file: {error}", file_path.display());
                return;
            }
        };
        // The file's own id is untrusted (it would name the file written
        // here) and may collide with an existing chat: the copy gets a fresh
        // one.
        chat.id = format!("chat-{}", Self::now_epoch_nanos());
        let Some(save_path) = Self::ensure_chats_dir().and_then(|_| Self::chat_file_path(&chat.id))
        else {
            return;
        };
        let serialized = match serde_json::to_string_pretty(&chat) {
            Ok(serialized) => serialized,
            Err(error) => {
                tracing::warn!("Could not serialize the imported chat: {error}");
                return;
            }
        };
        if let Err(error) = write_chat_file(&vfs::provider(), &save_path, &serialized) {
            tracing::warn!("Could not import chat to {}: {error}", save_path.display());
            return;
        }
        self.update_chat_history_entry(&chat, cx);
        self.load_chat_session(&chat.id, cx);
    }
}

/// Whether `id` may name a chat file: non-empty, ASCII letters, digits, `-`
/// and `_` only. Ids are read back from file contents and from imported
/// files, so anything else (a separator, `..`, a drive letter) could make a
/// crafted chat file name a path outside the chats folder.
pub(super) fn is_valid_chat_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Write `contents` to `path` atomically, so a crash or a full disk mid-write
/// cannot leave a truncated chat behind. A remote or P2P provider has no
/// atomic replace yet and writes directly.
fn write_chat_file(
    provider: &Arc<dyn FsProvider>,
    path: &Path,
    contents: &str,
) -> anyhow::Result<()> {
    if provider.is_remote() {
        provider.write_file(path, contents.as_bytes())?;
    } else {
        provider.write_file_atomically(path, contents.as_bytes())?;
    }
    if Arc::ptr_eq(provider, &vfs::provider()) {
        engine_fs::emit(path, FsChangeKind::Modified);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_valid_chat_id;

    #[test]
    fn chat_ids_are_plain_file_names() {
        assert!(is_valid_chat_id("chat-1699999999999999999"));
        assert!(is_valid_chat_id("Chat_01"));
        for bad in [
            "",
            ".",
            "..",
            "../layout",
            "a/b",
            "a\\b",
            "C:",
            "chat.json",
            "chat 1",
            "chät",
            "chat\0",
        ] {
            assert!(!is_valid_chat_id(bad), "{bad:?} must be refused");
        }
    }
}
