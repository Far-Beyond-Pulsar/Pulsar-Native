//! State management for the multiplayer window

use gpui::*;
use ui::input::InputState;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::types::*;
use super::file_sync_ui::FileSyncUI;
use engine_backend::subsystems::networking::simple_sync::SyncDiff;
use engine_backend::subsystems::networking::multiuser::MultiuserClient;

/// Multiplayer collaboration window for connecting to multiuser servers
pub struct MultiplayerWindow {
    pub(super) server_address_input: Entity<InputState>,
    pub(super) session_id_input: Entity<InputState>,
    pub(super) session_password_input: Entity<InputState>,
    pub(super) chat_input: Entity<InputState>,
    pub(super) connection_status: ConnectionStatus,
    pub(super) active_session: Option<ActiveSession>,
    pub(super) client: Option<Arc<RwLock<MultiuserClient>>>,
    pub(super) current_peer_id: Option<String>,
    pub(super) current_tab: SessionTab,
    pub(super) chat_messages: Vec<ChatMessage>,
    pub(super) file_assets: Vec<FileAssetStatus>, // Project assets with sync status
    pub(super) user_presences: Vec<UserPresence>, // Real-time user presence data
    pub(super) focus_handle: FocusHandle,
    // File sync state
    pub(super) project_root: Option<PathBuf>,
    pub(super) pending_file_sync: Option<(SyncDiff, String)>, // (diff, host_peer_id)
    pub(super) file_sync_in_progress: bool,
    pub(super) sync_progress_message: Option<String>,
    pub(super) sync_progress_percent: Option<f32>,
    pub(super) file_sync_ui: Entity<FileSyncUI>,
}

impl MultiplayerWindow {
    /// Create a new multiplayer window
        pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let server_address_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("ws://localhost:8080", window, cx);
            state
        });

        let session_id_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("Enter session ID", window, cx);
            state
        });

        let session_password_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("Enter session password", window, cx);
            state
        });

        let chat_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("Type a message...", window, cx);
            state
        });

        // Try to get project root from current directory
        let project_root = std::env::current_dir().ok();

        // Create file sync UI
        let file_sync_ui = cx.new(|cx| FileSyncUI::new(cx));

        Self {
            server_address_input,
            session_id_input,
            session_password_input,
            chat_input,
            connection_status: ConnectionStatus::Disconnected,
            active_session: None,
            client: None,
            current_peer_id: None,
            current_tab: SessionTab::Info,
            chat_messages: Vec::new(),
            file_assets: Vec::new(),
            user_presences: Vec::new(),
            focus_handle: cx.focus_handle(),
            project_root,
            pending_file_sync: None,
            file_sync_in_progress: false,
            sync_progress_message: None,
            sync_progress_percent: None,
            file_sync_ui,
        }
    }

    /// Populate the file sync UI with entries from a diff
    pub(super) fn populate_file_sync_ui(&mut self, diff: &SyncDiff, cx: &mut Context<Self>) {
        use crate::file_sync_ui::FileSyncEntry;
        use std::fs;

        let mut entries = Vec::new();

        // Get project root
        let project_root = match &self.project_root {
            Some(root) => root.clone(),
            None => {
                tracing::error!("No project root available for populating file sync UI");
                return;
            }
        };

        // Process added files
        for file_path in &diff.files_to_add {
            // For added files, we don't have local content, will fetch remote later
            entries.push(FileSyncEntry::new_added(
                file_path.clone(),
                String::new(), // Will be populated when file content is received
            ));
        }

        // Process modified files
        for file_path in &diff.files_to_update {
            let full_path = project_root.join(file_path);
            let local_content = fs::read_to_string(&full_path).unwrap_or_default();

            entries.push(FileSyncEntry::new_modified(
                file_path.clone(),
                local_content,
                String::new(), // Will be populated when file content is received
            ));
        }

        // Process deleted files
        for file_path in &diff.files_to_delete {
            let full_path = project_root.join(file_path);
            let local_content = fs::read_to_string(&full_path).unwrap_or_default();

            entries.push(FileSyncEntry::new_deleted(
                file_path.clone(),
                local_content,
            ));
        }

        // Update the file sync UI
        self.file_sync_ui.update(cx, |ui, cx| {
            ui.set_files(entries, cx);
        });

        tracing::info!("Populated file sync UI with {} entries",
            diff.files_to_add.len() + diff.files_to_update.len() + diff.files_to_delete.len());
    }

    /// Update a file entry with remote content when received
    pub(super) fn update_file_remote_content(&mut self, file_path: &str, content: String, cx: &mut Context<Self>) {
        self.file_sync_ui.update(cx, |ui, cx| {
            // Find the file entry and update its remote content
            if let Some(entry) = ui.files.iter_mut().find(|e| e.path == file_path) {
                entry.remote_content = Some(content);
                cx.notify();
                tracing::debug!("Updated remote content for {}", file_path);
            }
        });
    }

    /// Simulate a file diff for development/testing purposes
    pub(super) fn simulate_diff_for_dev(&mut self, cx: &mut Context<Self>) {
        use crate::file_sync_ui::FileSyncEntry;

        // Create mock file entries with realistic diffs
        let mock_files = vec![
            FileSyncEntry::new_added(
                "src/new_feature.rs".to_string(),
                r#"// New feature implementation
pub struct NewFeature {
    pub name: String,
    pub enabled: bool,
}

impl NewFeature {
    pub fn new(name: String) -> Self {
        Self {
            name,
            enabled: true,
        }
    }
}
"#.to_string(),
            ),
            FileSyncEntry::new_modified(
                "src/main.rs".to_string(),
                r#"fn main() {
    println!("Hello, world!");
    let x = 42;
}
"#.to_string(),
                r#"fn main() {
    println!("Hello, Pulsar!");
    let x = 42;
    let y = 100;
    println!("Sum: {}", x + y);
}
"#.to_string(),
            ),
            FileSyncEntry::new_modified(
                "Cargo.toml".to_string(),
                r#"[package]
name = "pulsar"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = "1.0"
"#.to_string(),
                r#"[package]
name = "pulsar"
version = "0.2.0"
edition = "2021"

[dependencies]
tokio = "1.35"
serde = "1.0"
"#.to_string(),
            ),
            FileSyncEntry::new_deleted(
                "src/old_code.rs".to_string(),
                r#"// This file is being removed
pub fn old_function() {
    println!("This is old code");
}
"#.to_string(),
            ),
        ];

        // Update the file sync UI with mock data
        self.file_sync_ui.update(cx, |ui, cx| {
            ui.set_files(mock_files, cx);
        });

        // Create a mock diff
        let mock_diff = SyncDiff {
            files_to_add: vec!["src/new_feature.rs".to_string()],
            files_to_update: vec!["src/main.rs".to_string(), "Cargo.toml".to_string()],
            files_to_delete: vec!["src/old_code.rs".to_string()],
        };

        // Set pending sync with mock data
        self.pending_file_sync = Some((mock_diff, "dev-mock-peer".to_string()));
        self.current_tab = SessionTab::FileSync;

        tracing::info!("DEV: Simulated diff for testing file sync UI");
        cx.notify();
    }

}
