//! State management for the multiplayer window

use gpui::*;
use ui::input::InputState;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::types::*;
use super::diff_viewer::{DiffViewer, DiffFileEntry};
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
    pub(super) diff_viewer: Entity<DiffViewer>,
    /// Pending diff to populate on next render (when we have window access)
    pub(super) pending_diff_populate: Option<SyncDiff>,
    /// Pending file content updates (path, content)
    pub(super) pending_file_updates: Vec<(String, String)>,
}

impl MultiplayerWindow {
    /// Create a new multiplayer window
        pub fn new(project_path: Option<std::path::PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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

        // Use provided project path
        let project_root = project_path;

        // Create diff viewer for file sync
        let diff_viewer = cx.new(|cx| DiffViewer::new(cx));

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
            diff_viewer,
            pending_diff_populate: None,
            pending_file_updates: Vec::new(),
        }
    }

    /// Populate the file sync UI with entries from a diff
    pub(super) fn populate_file_sync_ui(&mut self, diff: &SyncDiff, window: &mut Window, cx: &mut Context<Self>) {
        use std::fs;

        let mut diff_files = Vec::new();

        // Get project root
        let project_root = match &self.project_root {
            Some(root) => root.clone(),
            None => {
                tracing::error!("No project root available for populating file sync UI");
                return;
            }
        };

        // Process added files (no local content)
        for file_path in &diff.files_to_add {
            diff_files.push(DiffFileEntry {
                path: file_path.clone(),
                before_content: String::new(), // No local file
                after_content: String::new(), // Will be populated when received
            });
        }

        // Process modified files
        for file_path in &diff.files_to_update {
            let full_path = project_root.join(file_path);
            let local_content = fs::read_to_string(&full_path).unwrap_or_default();

            diff_files.push(DiffFileEntry {
                path: file_path.clone(),
                before_content: local_content,
                after_content: String::new(), // Will be populated when received
            });
        }

        // Process deleted files
        for file_path in &diff.files_to_delete {
            let full_path = project_root.join(file_path);
            let local_content = fs::read_to_string(&full_path).unwrap_or_default();

            diff_files.push(DiffFileEntry {
                path: file_path.clone(),
                before_content: local_content,
                after_content: String::new(), // No remote file
            });
        }

        // Enter diff mode with the file list and project root
        let project_root_for_editor = project_root.clone();
        self.diff_viewer.update(cx, |viewer, cx| {
            viewer.enter_diff_mode(diff_files, project_root_for_editor, window, cx);
        });

        tracing::debug!("Populated file sync UI with {} entries",
            diff.files_to_add.len() + diff.files_to_update.len() + diff.files_to_delete.len());
    }

    /// Update a file entry with remote content when received
    pub(super) fn update_file_remote_content(&mut self, file_path: &str, content: String, window: &mut Window, cx: &mut Context<Self>) {
        self.diff_viewer.update(cx, |viewer, cx| {
            viewer.update_diff_file_after_content(file_path, content.clone(), window, cx);
            tracing::debug!("Updated remote content for {}", file_path);
        });
    }

    /// Queue a file content update (for async contexts without window access)
    pub(super) fn queue_file_content_update(&mut self, file_path: String, content: String, cx: &mut Context<Self>) {
        self.pending_file_updates.push((file_path, content));
        cx.notify(); // Trigger re-render to process pending updates
    }

    /// Queue a diff populate (for async contexts without window access)
    pub(super) fn queue_diff_populate(&mut self, diff: SyncDiff, cx: &mut Context<Self>) {
        self.pending_diff_populate = Some(diff);
        cx.notify(); // Trigger re-render to process pending updates
    }

    /// Process pending updates (called from render where we have window access)
    pub(super) fn process_pending_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Process pending diff populate
        if let Some(diff) = self.pending_diff_populate.take() {
            self.populate_file_sync_ui(&diff, window, cx);
        }

        // Process pending file content updates
        let updates = std::mem::take(&mut self.pending_file_updates);
        for (file_path, content) in updates {
            self.update_file_remote_content(&file_path, content, window, cx);
        }
    }

    /// Initialize presence for connected users
    pub(super) fn update_presence_from_participants(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = &self.active_session {
            // Create presence entries for participants who don't have one
            for peer_id in &session.connected_users {
                if !self.user_presences.iter().any(|p| &p.peer_id == peer_id) {
                    self.user_presences.push(UserPresence::new(peer_id.clone()));
                }
            }

            // Remove presence entries for participants who left
            self.user_presences.retain(|p| session.connected_users.contains(&p.peer_id));
        }
        cx.notify();
    }

    /// Get presence for a specific peer
    pub(super) fn get_presence_mut(&mut self, peer_id: &str) -> Option<&mut UserPresence> {
        self.user_presences.iter_mut().find(|p| p.peer_id == peer_id)
    }

    /// Simulate a file diff for development/testing purposes
    pub(super) fn simulate_diff_for_dev(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use std::fs;

        // Get project root from the stored path
        let project_root = match &self.project_root {
            Some(root) => root.clone(),
            None => {
                tracing::error!("No project path available - cannot simulate diff");
                return;
            }
        };

        tracing::debug!("Using project root for diff simulation: {:?}", project_root);

        // Look for common project files - try multiple patterns
        let test_file_patterns = vec![
            // Common project files
            vec!["Cargo.toml", "src/main.rs", "src/lib.rs"],
            vec!["package.json", "src/index.js", "README.md"],
            vec!["README.md", "Cargo.toml", ".gitignore"],
            // Fallback - just try to find any files
            vec!["Cargo.toml"],
        ];

        let mut test_files = Vec::new();

        // Try each pattern until we find some files
        for pattern in test_file_patterns {
            for file in &pattern {
                let full_path = project_root.join(file);
                if full_path.exists() && full_path.is_file() {
                    test_files.push(file.to_string());
                }
            }

            // If we found at least 1 file, use this pattern
            if !test_files.is_empty() {
                break;
            }
        }

        // If still no files, try to find ANY files in the project root
        if test_files.is_empty() {
            if let Ok(entries) = fs::read_dir(&project_root) {
                for entry in entries.flatten().take(3) {
                    if let Ok(metadata) = entry.metadata() {
                        if metadata.is_file() {
                            if let Some(name) = entry.file_name().to_str() {
                                test_files.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }

        if test_files.is_empty() {
            tracing::error!("No files found in project root: {:?}", project_root);
            return;
        }

        tracing::debug!("Found {} files for diff simulation: {:?}", test_files.len(), test_files);

        let mut diff_entries = Vec::new();
        let mut files_to_update = Vec::new();

        for file_path in &test_files {
            let full_path = project_root.join(file_path);
            if let Ok(content) = fs::read_to_string(&full_path) {
                // Create a modified version by adding a comment at the top
                let modified_content = format!("// SIMULATED CHANGE - This line was added for diff testing\n{}", content);

                diff_entries.push(DiffFileEntry {
                    path: file_path.to_string(),
                    before_content: content,
                    after_content: modified_content,
                });

                files_to_update.push(file_path.to_string());
            } else {
                tracing::warn!("Could not read file for diff simulation: {}", file_path);
            }
        }

        if diff_entries.is_empty() {
            tracing::error!("No files found for diff simulation");
            return;
        }

        // Enter diff mode with real file data
        self.diff_viewer.update(cx, |viewer, cx| {
            viewer.enter_diff_mode(diff_entries, project_root, window, cx);
        });

        // Create a mock diff
        let mock_diff = SyncDiff {
            files_to_add: vec![],
            files_to_update,
            files_to_delete: vec![],
        };

        let file_count = mock_diff.files_to_update.len();

        // Set pending sync with mock data
        self.pending_file_sync = Some((mock_diff, "dev-mock-peer".to_string()));
        self.current_tab = SessionTab::FileSync;

        tracing::debug!("DEV: Simulated diff for testing file sync UI with {} files", file_count);
        cx.notify();
    }

    /// Generate a consistent color for a user based on their peer ID
    pub(super) fn generate_user_color(peer_id: &str) -> Hsla {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        peer_id.hash(&mut hasher);
        let hash = hasher.finish();

        // Generate a vibrant color from the hash
        let hue = (hash % 360) as f32 / 360.0;
        let saturation = 0.7;
        let lightness = 0.6;
        let alpha = 1.0;

        hsla(hue, saturation, lightness, alpha)
    }

}
