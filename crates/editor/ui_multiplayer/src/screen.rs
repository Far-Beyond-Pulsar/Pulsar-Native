use gpui::prelude::FluentBuilder;
use gpui::*;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use ui::input::InputState;
use ui::{button::Button, h_flex, v_flex, ActiveTheme as _, Icon, IconName, StyledExt as _, TitleBar};

use crate::diff_viewer::{DiffFileEntry, DiffViewer};
use crate::utils::types::*;
use engine_backend::subsystems::networking::multiuser::{
    ClientMessage, MultiuserClient, PeerIdentity, PeerProfile,
};
use engine_backend::subsystems::networking::simple_sync::SyncDiff;
use engine_fs::{
    events::{FsChangeKind, FsEventSource},
    subscribe,
};
use engine_state::{EngineContext, MultiuserContext, MultiuserParticipant, MultiuserStatus};

use crate::components::{render_active_session, render_chat_tab, render_connection_form};

pub struct MultiplayerWindow {
    pub(crate) server_address_input: Entity<InputState>,
    pub(crate) session_id_input: Entity<InputState>,
    pub(crate) session_password_input: Entity<InputState>,
    pub(crate) chat_input: Entity<InputState>,
    pub(crate) connection_status: ConnectionStatus,
    pub(crate) active_session: Option<ActiveSession>,
    pub(crate) client: Option<Arc<RwLock<MultiuserClient>>>,
    pub(crate) current_peer_id: Option<String>,
    pub(crate) current_tab: SessionTab,
    pub(crate) chat_messages: Vec<ChatMessage>,
    pub(crate) file_assets: Vec<FileAssetStatus>,
    pub(crate) user_presences: Vec<UserPresence>,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) project_root: Option<PathBuf>,
    pub(crate) pending_file_sync: Option<(SyncDiff, String)>,
    pub(crate) file_sync_in_progress: bool,
    pub(crate) sync_progress_message: Option<String>,
    pub(crate) sync_progress_percent: Option<f32>,
    pub(crate) diff_viewer: Entity<DiffViewer>,
    pub(crate) pending_diff_populate: Option<SyncDiff>,
    pub(crate) pending_file_updates: Vec<(String, String)>,
    pub(crate) fs_event_forwarder: Option<gpui::Task<()>>,
}

impl MultiplayerWindow {
    pub fn new(
        project_path: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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

        let project_root = project_path;
        let diff_viewer = cx.new(DiffViewer::new);

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
            fs_event_forwarder: None,
        }
    }

    pub(crate) fn sync_engine_multiuser_connecting(&self, server_url: &str, session_id: &str) {
        let Some(ctx) = EngineContext::global() else {
            return;
        };

        let peer_id = self
            .current_peer_id
            .clone()
            .unwrap_or_else(|| "local".to_string());
        let host_peer_id = self
            .active_session
            .as_ref()
            .and_then(|s| s.connected_users.first())
            .cloned()
            .unwrap_or_else(|| peer_id.clone());

        let mut session = MultiuserContext::new_peer_to_peer(
            server_url.to_string(),
            session_id.to_string(),
            peer_id,
            host_peer_id,
        )
        .with_status(MultiuserStatus::Connecting);

        if let Some(active_session) = &self.active_session {
            session = session.with_join_token(active_session.join_token.clone());
        }

        ctx.set_multiuser(session);
        ctx.notify_multiuser_changed();
    }

    pub(crate) fn sync_engine_multiuser_connected(
        &self,
        server_url: &str,
        session_id: &str,
        our_peer_id: &str,
        participants: &[String],
    ) {
        let Some(ctx) = EngineContext::global() else {
            return;
        };

        let host_peer_id = participants
            .first()
            .cloned()
            .unwrap_or_else(|| our_peer_id.to_string());

        let mut session = MultiuserContext::new_peer_to_peer(
            server_url.to_string(),
            session_id.to_string(),
            our_peer_id.to_string(),
            host_peer_id,
        )
        .with_status(MultiuserStatus::Connected {
            relay_mode: None,
        })
        .with_participants(participants.to_vec());

        if let Some(active_session) = &self.active_session {
            session = session.with_join_token(active_session.join_token.clone());
        }

        ctx.set_multiuser(session);
        ctx.notify_multiuser_changed();
    }

    pub(crate) fn sync_engine_multiuser_error(&self, message: String) {
        let Some(ctx) = EngineContext::global() else {
            return;
        };

        if !ctx.update_multiuser(|mu| {
            mu.set_status(MultiuserStatus::Error(message.clone()));
        }) {
            let fallback = MultiuserContext::new_peer_to_peer(
                "unknown".to_string(),
                "unknown".to_string(),
                "local".to_string(),
                "remote".to_string(),
            )
            .with_status(MultiuserStatus::Error(message));
            ctx.set_multiuser(fallback);
        }
        ctx.notify_multiuser_changed();
    }

    pub(crate) fn sync_engine_multiuser_disconnected(&self) {
        if let Some(ctx) = EngineContext::global() {
            ctx.clear_multiuser();
            ctx.notify_multiuser_changed();
        }
    }

    pub(crate) fn sync_engine_multiuser_profiles(&self, profiles: Vec<PeerProfile>) {
        if let Some(ctx) = EngineContext::global() {
            let participants = profiles
                .into_iter()
                .map(|profile| MultiuserParticipant {
                    peer_id: profile.peer_id,
                    display_name: profile.display_name,
                    avatar_url: profile.avatar_url,
                    github_login: profile.github_login,
                    ping_ms: None,
                })
                .collect();
            ctx.set_multiuser_participant_profiles(participants);
            ctx.notify_multiuser_changed();
        }
    }

    pub(crate) fn sync_engine_multiuser_latency(&self, latency_ms: Option<u32>) {
        if let Some(ctx) = EngineContext::global() {
            ctx.set_multiuser_latency_ms(latency_ms);
            ctx.notify_multiuser_changed();
        }
    }

    pub(crate) fn start_fs_event_forwarder(
        &mut self,
        client: Arc<RwLock<MultiuserClient>>,
        session_id: String,
        peer_id: String,
        cx: &mut Context<Self>,
    ) {
        self.fs_event_forwarder = Some(cx.spawn(async move |_this, _cx| {
            let mut rx = subscribe();

            while let Ok(event) = rx.recv().await {
                if !matches!(event.source, FsEventSource::Local) {
                    continue;
                }

                let kind = match event.kind {
                    FsChangeKind::Created => "created",
                    FsChangeKind::Modified => "modified",
                    FsChangeKind::Deleted => "deleted",
                }
                .to_string();

                let path = event.path.to_string_lossy().replace('\\', "/");
                let client_guard = client.read().await;
                let _ = client_guard
                    .send(ClientMessage::FileChanged {
                        session_id: session_id.clone(),
                        peer_id: peer_id.clone(),
                        path,
                        kind,
                    })
                    .await;
            }
        }));
    }

    pub(crate) fn current_peer_identity(&self) -> Option<PeerIdentity> {
        let profile = EngineContext::global()?.auth_profile()?;
        Some(PeerIdentity {
            display_name: profile.display_name.clone().or(Some(profile.login.clone())),
            avatar_url: profile.avatar_url.clone(),
            github_login: Some(profile.login),
        })
    }

    pub(crate) fn populate_file_sync_ui(
        &mut self,
        diff: &SyncDiff,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use std::fs;

        let mut diff_files = Vec::new();

        let project_root = match &self.project_root {
            Some(root) => root.clone(),
            None => {
                tracing::error!("No project root available for populating file sync UI");
                return;
            }
        };

        for file_path in &diff.files_to_add {
            diff_files.push(DiffFileEntry {
                path: file_path.clone(),
                before_content: String::new(),
                after_content: String::new(),
            });
        }

        for file_path in &diff.files_to_update {
            let full_path = project_root.join(file_path);
            let local_content = fs::read_to_string(&full_path).unwrap_or_default();

            diff_files.push(DiffFileEntry {
                path: file_path.clone(),
                before_content: local_content,
                after_content: String::new(),
            });
        }

        for file_path in &diff.files_to_delete {
            let full_path = project_root.join(file_path);
            let local_content = fs::read_to_string(&full_path).unwrap_or_default();

            diff_files.push(DiffFileEntry {
                path: file_path.clone(),
                before_content: local_content,
                after_content: String::new(),
            });
        }

        let project_root_for_editor = project_root.clone();
        self.diff_viewer.update(cx, |viewer, cx| {
            viewer.enter_diff_mode(diff_files, project_root_for_editor, window, cx);
        });

        tracing::debug!(
            "Populated file sync UI with {} entries",
            diff.files_to_add.len() + diff.files_to_update.len() + diff.files_to_delete.len()
        );
    }

    pub(crate) fn update_file_remote_content(
        &mut self,
        file_path: &str,
        content: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.diff_viewer.update(cx, |viewer, cx| {
            viewer.update_diff_file_after_content(file_path, content.clone(), window, cx);
            tracing::debug!("Updated remote content for {}", file_path);
        });
    }

    pub(crate) fn queue_file_content_update(
        &mut self,
        file_path: String,
        content: String,
        cx: &mut Context<Self>,
    ) {
        self.pending_file_updates.push((file_path, content));
        cx.notify();
    }

    pub(crate) fn queue_diff_populate(&mut self, diff: SyncDiff, cx: &mut Context<Self>) {
        self.pending_diff_populate = Some(diff);
        cx.notify();
    }

    pub(crate) fn process_pending_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(diff) = self.pending_diff_populate.take() {
            self.populate_file_sync_ui(&diff, window, cx);
        }

        let updates = std::mem::take(&mut self.pending_file_updates);
        for (file_path, content) in updates {
            self.update_file_remote_content(&file_path, content, window, cx);
        }
    }

    pub(crate) fn update_presence_from_participants(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = &self.active_session {
            for peer_id in &session.connected_users {
                if !self.user_presences.iter().any(|p| &p.peer_id == peer_id) {
                    self.user_presences.push(UserPresence::new(peer_id.clone()));
                }
            }

            self.user_presences
                .retain(|p| session.connected_users.contains(&p.peer_id));
        }
        cx.notify();
    }

    pub(crate) fn get_presence_mut(&mut self, peer_id: &str) -> Option<&mut UserPresence> {
        self.user_presences
            .iter_mut()
            .find(|p| p.peer_id == peer_id)
    }

    pub(crate) fn simulate_diff_for_dev(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use std::fs;

        let project_root = match &self.project_root {
            Some(root) => root.clone(),
            None => {
                tracing::error!("No project path available - cannot simulate diff");
                return;
            }
        };

        tracing::debug!("Using project root for diff simulation: {:?}", project_root);

        let test_file_patterns = vec![
            vec!["Cargo.toml", "src/main.rs", "src/lib.rs"],
            vec!["package.json", "src/index.js", "README.md"],
            vec!["README.md", "Cargo.toml", ".gitignore"],
            vec!["Cargo.toml"],
        ];

        let mut test_files = Vec::new();

        for pattern in test_file_patterns {
            for file in &pattern {
                let full_path = project_root.join(file);
                if full_path.exists() && full_path.is_file() {
                    test_files.push(file.to_string());
                }
            }

            if !test_files.is_empty() {
                break;
            }
        }

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

        tracing::debug!(
            "Found {} files for diff simulation: {:?}",
            test_files.len(),
            test_files
        );

        let mut diff_entries = Vec::new();
        let mut files_to_update = Vec::new();

        for file_path in &test_files {
            let full_path = project_root.join(file_path);
            if let Ok(content) = fs::read_to_string(&full_path) {
                let modified_content = format!(
                    "// SIMULATED CHANGE - This line was added for diff testing\n{}",
                    content
                );

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

        self.diff_viewer.update(cx, |viewer, cx| {
            viewer.enter_diff_mode(diff_entries, project_root, window, cx);
        });

        let mock_diff = SyncDiff {
            files_to_add: vec![],
            files_to_update,
            files_to_delete: vec![],
        };

        let file_count = mock_diff.files_to_update.len();

        self.pending_file_sync = Some((mock_diff, "dev-mock-peer".to_string()));
        self.current_tab = SessionTab::FileSync;

        tracing::debug!(
            "DEV: Simulated diff for testing file sync UI with {} files",
            file_count
        );
        cx.notify();
    }

    pub(crate) fn generate_user_color(peer_id: &str) -> Hsla {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        peer_id.hash(&mut hasher);
        let hash = hasher.finish();

        let hue = (hash % 360) as f32 / 360.0;
        let saturation = 0.7;
        let lightness = 0.6;
        let alpha = 1.0;

        hsla(hue, saturation, lightness, alpha)
    }
}

impl Focusable for MultiplayerWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MultiplayerWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.process_pending_updates(window, cx);

        let kick_reason = engine_state::EngineContext::global().and_then(|ctx| {
            ctx.multiuser()
                .and_then(|multiuser| match multiuser.status {
                    engine_state::MultiuserStatus::Error(ref message)
                        if message.contains("Kicked from session") =>
                    {
                        Some(message.clone())
                    }
                    _ => None,
                })
        });

        if self.pending_file_sync.is_some() {
            tracing::debug!("RENDER: pending_file_sync present, FileSync tab should show it");
        }

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .child(div().text_sm().child("Multiplayer"))
                        .when_some(self.active_session.as_ref(), |this, session| {
                            this.child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .w(px(6.))
                                            .h(px(6.))
                                            .rounded(px(3.))
                                            .bg(cx.theme().success),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().foreground)
                                            .child(format!(
                                                "{} users",
                                                session.connected_users.len()
                                            )),
                                    ),
                            )
                        }),
                ),
            )
            .child(if let Some(ref session) = self.active_session {
                render_active_session(self, session, cx).into_any_element()
            } else {
                render_connection_form(self, cx).into_any_element()
            })
            .when_some(kick_reason, |this, reason| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::rgba(0x000000dd))
                        .child(
                            v_flex()
                                .w(px(520.))
                                .gap_4()
                                .p_6()
                                .rounded(px(12.))
                                .bg(cx.theme().background)
                                .border_1()
                                .border_color(cx.theme().border)
                                .shadow_lg()
                                .child(
                                    h_flex()
                                        .gap_3()
                                        .items_center()
                                        .child(
                                            Icon::new(IconName::TriangleAlert)
                                                .size(px(24.))
                                                .text_color(cx.theme().danger),
                                        )
                                        .child(
                                            div()
                                                .text_lg()
                                                .font_bold()
                                                .text_color(cx.theme().foreground)
                                                .child("Session ended"),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(reason),
                                )
                                .child(
                                    Button::new("dismiss-kick")
                                        .label("Close")
                                        .w_full()
                                        .on_click(cx.listener(|_, _, _window, cx| {
                                            if let Some(ctx) =
                                                engine_state::EngineContext::global()
                                            {
                                                ctx.clear_multiuser();
                                            }
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
            })
    }
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for MultiplayerWindow {
    type Params = ();

    fn window_name() -> &'static str {
        "MultiplayerWindow"
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(500.0, 600.0)
    }

    fn build(_: (), window: &mut gpui::Window, cx: &mut gpui::App) -> gpui::Entity<Self> {
        let path = engine_state::get_project_path().map(std::path::PathBuf::from);
        cx.new(|cx| MultiplayerWindow::new(path, window, cx))
    }
}
