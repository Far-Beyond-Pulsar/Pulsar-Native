//! Git Manager Window
//!
//! A GitHub Desktop-like Git manager built with GPUI and the UI crate

mod auto_fetch;
mod avatar_loader;
mod components;
mod git_hooks;
mod git_operations;
mod handlers;
mod models;
mod utils;
mod views;

use gpui::ClipboardItem;
use gpui::*;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use ui::{
    ActiveTheme as _, TitleBar, VirtualListScrollHandle, h_flex,
    input::{InputEvent, InputState},
    scroll::ScrollbarState,
    v_flex,
};

pub use auto_fetch::{
    AutoFetchOutcome, AutoFetchSettings, AutoFetchSettingsWatcher, AutoFetchWaitOutcome,
    TrackingSnapshot, canonical_repository_path, fetch_tracking_snapshot, read_auto_fetch_settings,
    start_auto_fetch_task,
};
pub use git_hooks::{
    GitHookSyncReport, GitHookSyncSkipReason, GitHookSyncStatus, sync_configured_project_hooks,
};
pub use git_operations::*;
pub use handlers::open_git_settings_modal;
pub use models::*;

use views::AlignedRow;

// ── Diff-row constants (public for file_panel / commit_detail) ───────────────
pub const DIFF_LINE_ROW_H: f32 = 20.0;
pub const DIFF_COLLAPSE_ROW_H: f32 = 24.0;

// ── File context-menu actions ────────────────────────────────────────────────
#[derive(Clone, Debug, PartialEq, serde::Deserialize, Action)]
#[action(namespace = git_manager, no_json)]
pub struct DiscardFileChanges {
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, Action)]
#[action(namespace = git_manager, no_json)]
pub struct IgnoreFile {
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, Action)]
#[action(namespace = git_manager, no_json)]
pub struct IgnoreExtension {
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, Action)]
#[action(namespace = git_manager, no_json)]
pub struct IgnoreFolder {
    pub folder: String,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, Action)]
#[action(namespace = git_manager, no_json)]
pub struct CopyRelativePath {
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, Action)]
#[action(namespace = git_manager, no_json)]
pub struct CopyFullPath {
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, Action)]
#[action(namespace = git_manager, no_json)]
pub struct OpenInExplorer {
    pub path: String,
}
// ────────────────────────────────────────────────────────────────────────────

/// Which remote operation is awaiting credential input
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PendingAuthOp {
    Fetch,
    Push,
    Pull,
}

impl PendingAuthOp {
    pub fn label(self) -> &'static str {
        match self {
            PendingAuthOp::Fetch => "Fetch",
            PendingAuthOp::Push => "Push",
            PendingAuthOp::Pull => "Pull",
        }
    }

    fn kind(self) -> git_operations::RemoteOperationKind {
        match self {
            PendingAuthOp::Fetch => git_operations::RemoteOperationKind::Fetch,
            PendingAuthOp::Push => git_operations::RemoteOperationKind::Push,
            PendingAuthOp::Pull => git_operations::RemoteOperationKind::Pull,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PendingAuth {
    pub(crate) op: PendingAuthOp,
    pub(crate) context: git_operations::RemoteOperationContext,
}

/// A flattened row in the changes virtual list.
#[derive(Clone)]
pub enum ChangesRow {
    /// Section header: title, file count, is_staged
    Header {
        title: String,
        count: usize,
        is_staged: bool,
    },
    /// A file entry
    File { change: FileChange, is_staged: bool },
}

/// Main Git Manager window
pub struct GitManager {
    project_path: PathBuf,
    repo_state: Arc<RwLock<RepositoryState>>,
    // Changes view
    selected_file: Option<String>,
    file_diff: Option<DiffResult>,
    file_diff_error: Option<String>,
    file_diff_expanded: HashSet<usize>,
    commit_message_input: Entity<InputState>,
    commit_description_input: Entity<InputState>,
    /// Flattened row list for the virtual changes list (rebuilt when repo_state changes).
    pub(crate) changes_rows: Vec<ChangesRow>,
    /// Scroll handle for the virtual changes list.
    pub(crate) changes_scroll: VirtualListScrollHandle,
    /// Scrollbar state for the virtual changes list.
    pub(crate) changes_scrollbar_state: ScrollbarState,
    /// Avatar image cache keyed by commit email. `None` = fetch in-flight.
    pub(crate) avatar_cache: HashMap<String, Option<Arc<gpui::RenderImage>>>,
    // ── File diff virtual list ─────────────────────────────────────────────
    pub(crate) file_diff_rows: Vec<DiffRow>,
    pub(crate) file_diff_scroll: VirtualListScrollHandle,
    pub(crate) file_diff_scrollbar: ScrollbarState,
    // ── File aligned rows (side-by-side) ──────────────────────────────────
    pub(crate) file_aligned_rows: Vec<AlignedRow>,
    pub(crate) file_align_scroll: VirtualListScrollHandle,
    pub(crate) file_align_scrollbar: ScrollbarState,
    // ── Commit diff virtual list ───────────────────────────────────────────
    pub(crate) commit_diff_rows: Vec<DiffRow>,
    pub(crate) commit_diff_scroll: VirtualListScrollHandle,
    pub(crate) commit_diff_scrollbar: ScrollbarState,
    // ── Commit aligned rows (side-by-side) ────────────────────────────────
    pub(crate) commit_aligned_rows: Vec<AlignedRow>,
    pub(crate) commit_align_scroll: VirtualListScrollHandle,
    pub(crate) commit_align_scrollbar: ScrollbarState,
    // History view
    selected_commit: Option<String>,
    selected_commit_files: Vec<FileChange>,
    selected_commit_file: Option<String>,
    commit_file_diff: Option<DiffResult>,
    commit_file_diff_error: Option<String>,
    commit_file_expanded: HashSet<usize>,
    /// Last error from a background git operation (push/pull/fetch/switch)
    pub(crate) op_error: Option<String>,
    /// When set, the toolbar shows a credential prompt bound to the failed operation target.
    pub(crate) pending_auth: Option<PendingAuth>,
    pub(crate) remote_operation_generation: u64,
    pub(crate) auth_username_input: Entity<InputState>,
    pub(crate) auth_password_input: Entity<InputState>,
    current_view: GitView,
    pub(crate) diff_view_mode: DiffViewMode,
    focus_handle: FocusHandle,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GitView {
    Changes,
    History,
    Branches,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiffViewMode {
    Unified,
    SideBySide,
}

impl GitManager {
    pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let repo_state = Arc::new(RwLock::new(RepositoryState::default()));

        // Create commit message input
        let commit_message_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_placeholder("Summary (required)", window, cx);
            input
        });

        // Create commit description input
        let commit_description_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).multi_line().auto_grow(2, 5);
            input.set_placeholder("Description (optional)", window, cx);
            input
        });

        // Subscribe to Enter key events on commit message input
        cx.subscribe(
            &commit_message_input,
            |this, _input, event: &InputEvent, cx| {
                if let InputEvent::PressEnter { secondary: false } = event {
                    this.commit_changes(cx);
                }
            },
        )
        .detach();

        // Create auth credential inputs
        let auth_username_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_placeholder("Username", window, cx);
            input
        });
        let auth_password_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_placeholder("Password / Token", window, cx);
            input.set_masked(true, window, cx);
            input
        });

        // Load initial git state. Credentials are resolved for a fixed remote
        // only after a remote-operation guard has been acquired.
        let path = project_path.clone();
        cx.spawn(async move |this, cx| {
            let load_path = path.clone();
            let state_result = cx
                .background_executor()
                .spawn(async move { load_repository_state(&load_path) })
                .await;
            cx.update(|cx| {
                this.update(cx, |git_manager, cx| {
                    match state_result {
                        Ok(s) => {
                            *git_manager.repo_state.write() = s;
                            git_manager.op_error = None;
                        }
                        Err(e) => {
                            git_manager.op_error = Some(format!(
                                "Failed to load repository state for {}: {}",
                                path.display(),
                                e
                            ));
                        }
                    }
                    git_manager.rebuild_changes_rows();
                    cx.notify();
                });
            });
        })
        .detach();

        Self {
            project_path,
            repo_state,
            selected_file: None,
            file_diff: None,
            file_diff_error: None,
            file_diff_expanded: HashSet::new(),
            commit_message_input,
            commit_description_input,
            changes_rows: Vec::new(),
            changes_scroll: VirtualListScrollHandle::new(),
            changes_scrollbar_state: ScrollbarState::default(),
            avatar_cache: HashMap::new(),
            file_diff_rows: Vec::new(),
            file_diff_scroll: VirtualListScrollHandle::new(),
            file_diff_scrollbar: ScrollbarState::default(),
            file_aligned_rows: Vec::new(),
            file_align_scroll: VirtualListScrollHandle::new(),
            file_align_scrollbar: ScrollbarState::default(),
            commit_diff_rows: Vec::new(),
            commit_diff_scroll: VirtualListScrollHandle::new(),
            commit_diff_scrollbar: ScrollbarState::default(),
            commit_aligned_rows: Vec::new(),
            commit_align_scroll: VirtualListScrollHandle::new(),
            commit_align_scrollbar: ScrollbarState::default(),
            selected_commit: None,
            selected_commit_files: Vec::new(),
            selected_commit_file: None,
            commit_file_diff: None,
            commit_file_diff_error: None,
            commit_file_expanded: HashSet::new(),
            op_error: None,
            pending_auth: None,
            remote_operation_generation: 0,
            auth_username_input,
            auth_password_input,
            current_view: GitView::Changes,
            diff_view_mode: DiffViewMode::Unified,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Rebuild the flat row list used by the virtual changes list.
    /// Called after every repo state refresh.
    pub(crate) fn rebuild_changes_rows(&mut self) {
        let state = self.repo_state.read();
        let staged = state.staged_files.clone();
        let unstaged = state.unstaged_files.clone();
        let untracked = state.untracked_files.clone();
        drop(state);

        let mut unstaged_all: Vec<FileChange> = unstaged;
        unstaged_all.extend(untracked.iter().map(|path| FileChange {
            path: path.clone(),
            status: ChangeStatus::Untracked,
            additions: 0,
            deletions: 0,
        }));

        let mut rows = Vec::new();
        if !staged.is_empty() {
            rows.push(ChangesRow::Header {
                title: "Staged".into(),
                count: staged.len(),
                is_staged: true,
            });
            for f in staged {
                rows.push(ChangesRow::File {
                    change: f,
                    is_staged: true,
                });
            }
        }
        if !unstaged_all.is_empty() {
            rows.push(ChangesRow::Header {
                title: "Changes".into(),
                count: unstaged_all.len(),
                is_staged: false,
            });
            for f in unstaged_all {
                rows.push(ChangesRow::File {
                    change: f,
                    is_staged: false,
                });
            }
        }
        self.changes_rows = rows;
    }

    /// Ensure the GitHub avatar for a commit author is loaded.
    /// Uses the commit email to derive a GitHub username; falls back gracefully.
    /// Safe to call every render — it's a no-op if the entry is already cached or in-flight.
    pub(crate) fn ensure_avatar_loaded(&mut self, email: &str, cx: &mut Context<Self>) {
        if self.avatar_cache.contains_key(email) {
            return;
        }
        let Some(username) = avatar_loader::github_username_from_email(email) else {
            // Email is not a GitHub noreply address — insert a permanent miss so we
            // never retry, and fall back to the initials avatar in the UI.
            self.avatar_cache.insert(email.to_string(), None);
            return;
        };
        let url = avatar_loader::avatar_url(&username);
        // Mark as in-flight with `None` so we don't spawn a second request.
        self.avatar_cache.insert(email.to_string(), None);

        let email_key = email.to_string();
        let (tx, rx) = smol::channel::bounded::<Option<std::sync::Arc<gpui::RenderImage>>>(1);
        std::thread::spawn(move || {
            let result = avatar_loader::fetch_avatar(&url);
            let _ = smol::block_on(tx.send(result.ok()));
        });
        cx.spawn(async move |this, cx| {
            if let Ok(maybe) = rx.recv().await {
                cx.update(|cx| {
                    this.update(cx, |gm, cx| {
                        gm.avatar_cache.insert(email_key, maybe);
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    fn switch_view(&mut self, view: GitView, cx: &mut Context<Self>) {
        self.current_view = view;
        cx.notify();
    }

    fn refresh_state(&mut self, cx: &mut Context<Self>) {
        let path = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            let load_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { load_repository_state(&load_path) })
                .await;
            cx.update(|cx| {
                this.update(cx, |git_manager, cx| {
                    match result {
                        Ok(state) => {
                            *git_manager.repo_state.write() = state;
                            git_manager.op_error = None;
                        }
                        Err(e) => {
                            git_manager.op_error =
                                Some(format!("Refresh failed for {}: {}", path.display(), e));
                        }
                    }
                    git_manager.rebuild_changes_rows();
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn commit_changes(&mut self, cx: &mut Context<Self>) {
        let message = self.commit_message_input.read(cx).text().to_string();
        if message.trim().is_empty() {
            return;
        }

        let path = self.project_path.clone();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { commit_staged_changes(&path, &message) })
                .await;
            if result.is_ok() {
                cx.update(|cx| {
                    this.update(cx, |git_manager, cx| {
                        git_manager.refresh_state(cx);
                    });
                });
            }
        })
        .detach();
    }

    fn stage_file(&mut self, file_path: String, cx: &mut Context<Self>) {
        let path = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { stage_file(&path, &file_path) })
                .await;
            cx.update(|cx| {
                this.update(cx, |gm, cx| {
                    if let Err(e) = &result {
                        gm.op_error = Some(format!("Stage failed: {}", e));
                    }
                    gm.refresh_state(cx);
                });
            });
        })
        .detach();
    }

    pub fn stage_all(&mut self, cx: &mut Context<Self>) {
        let path = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { stage_all_files(&path) })
                .await;
            cx.update(|cx| {
                this.update(cx, |gm, cx| {
                    if let Err(e) = &result {
                        gm.op_error = Some(format!("Stage all failed: {}", e));
                    }
                    gm.refresh_state(cx);
                });
            });
        })
        .detach();
    }

    pub fn unstage_all(&mut self, cx: &mut Context<Self>) {
        let path = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { unstage_all_files(&path) })
                .await;
            cx.update(|cx| {
                this.update(cx, |gm, cx| {
                    if let Err(e) = &result {
                        gm.op_error = Some(format!("Unstage all failed: {}", e));
                    }
                    gm.refresh_state(cx);
                });
            });
        })
        .detach();
    }

    fn unstage_file(&mut self, file_path: String, cx: &mut Context<Self>) {
        let path = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { unstage_file(&path, &file_path) })
                .await;
            cx.update(|cx| {
                this.update(cx, |gm, cx| {
                    if let Err(e) = &result {
                        gm.op_error = Some(format!("Unstage failed: {}", e));
                    }
                    gm.refresh_state(cx);
                });
            });
        })
        .detach();
    }

    fn select_commit(&mut self, commit_hash: String, cx: &mut Context<Self>) {
        self.selected_commit = Some(commit_hash.clone());
        self.selected_commit_files = Vec::new();
        self.selected_commit_file = None;
        self.commit_file_diff = None;
        self.commit_file_diff_error = None;
        self.commit_file_expanded.clear();
        cx.notify();

        let path = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(files) = cx
                .background_executor()
                .spawn(async move { get_commit_files(&path, &commit_hash) })
                .await
            {
                cx.update(|cx| {
                    this.update(cx, |git_manager, cx| {
                        git_manager.selected_commit_files = files;
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    pub fn select_commit_file(&mut self, file_path: String, cx: &mut Context<Self>) {
        let commit_hash = match &self.selected_commit {
            Some(h) => h.clone(),
            None => return,
        };
        self.selected_commit_file = Some(file_path.clone());
        self.commit_file_diff = None;
        self.commit_diff_rows = Vec::new();
        self.commit_file_diff_error = None;
        self.commit_file_expanded.clear();
        cx.notify();

        let repo_path = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(
                    async move { load_file_diff_at_commit(&repo_path, &commit_hash, &file_path) },
                )
                .await;
            cx.update(|cx| {
                this.update(cx, |gm, cx| {
                    match result {
                        Ok(diff) => {
                            gm.commit_file_diff = Some(diff);
                            gm.commit_file_diff_error = None;
                            gm.rebuild_commit_diff_rows();
                        }
                        Err(msg) => {
                            gm.commit_file_diff = None;
                            gm.commit_file_diff_error = Some(msg);
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub fn select_file(&mut self, file_path: String, cx: &mut Context<Self>) {
        self.selected_file = Some(file_path.clone());
        self.file_diff = None;
        self.file_diff_rows = Vec::new();
        self.file_diff_error = None;
        self.file_diff_expanded.clear();
        cx.notify();
        let repo_path = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { load_file_diff_working(&repo_path, &file_path) })
                .await;
            cx.update(|cx| {
                this.update(cx, |gm, cx| {
                    match result {
                        Ok(diff) => {
                            gm.file_diff = Some(diff);
                            gm.file_diff_error = None;
                            gm.rebuild_file_diff_rows();
                        }
                        Err(msg) => {
                            gm.file_diff = None;
                            gm.file_diff_error = Some(msg);
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub fn expand_file_diff_region(&mut self, region_idx: usize, cx: &mut Context<Self>) {
        self.file_diff_expanded.insert(region_idx);
        self.rebuild_file_diff_rows();
        cx.notify();
    }

    pub fn expand_commit_diff_region(&mut self, region_idx: usize, cx: &mut Context<Self>) {
        self.commit_file_expanded.insert(region_idx);
        self.rebuild_commit_diff_rows();
        cx.notify();
    }

    pub(crate) fn rebuild_file_diff_rows(&mut self) {
        self.file_diff_rows = self
            .file_diff
            .as_ref()
            .map(|d| flatten_diff(d, &self.file_diff_expanded))
            .unwrap_or_default();
        self.file_aligned_rows = self
            .file_diff
            .as_ref()
            .map(|d| views::compute_aligned_rows(d, &self.file_diff_expanded))
            .unwrap_or_default();
    }

    pub(crate) fn rebuild_commit_diff_rows(&mut self) {
        self.commit_diff_rows = self
            .commit_file_diff
            .as_ref()
            .map(|d| flatten_diff(d, &self.commit_file_expanded))
            .unwrap_or_default();
        self.commit_aligned_rows = self
            .commit_file_diff
            .as_ref()
            .map(|d| views::compute_aligned_rows(d, &self.commit_file_expanded))
            .unwrap_or_default();
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        self.run_remote_op(PendingAuthOp::Fetch, None, None, cx);
    }

    fn push(&mut self, cx: &mut Context<Self>) {
        self.run_remote_op(PendingAuthOp::Push, None, None, cx);
    }

    fn pull(&mut self, cx: &mut Context<Self>) {
        self.run_remote_op(PendingAuthOp::Pull, None, None, cx);
    }

    pub fn retry_with_auth(&mut self, cx: &mut Context<Self>) {
        let pending_auth = match self.pending_auth.take() {
            Some(pending_auth) => pending_auth,
            None => return,
        };
        let username = self.auth_username_input.read(cx).text().to_string();
        let password = self.auth_password_input.read(cx).text().to_string();
        self.op_error = None;
        self.run_remote_op(
            pending_auth.op,
            Some((username, password)),
            Some(pending_auth.context),
            cx,
        );
    }

    pub fn cancel_auth(&mut self, cx: &mut Context<Self>) {
        self.pending_auth = None;
        self.op_error = None;
        cx.notify();
    }

    fn run_remote_op(
        &mut self,
        op: PendingAuthOp,
        explicit_creds: Option<(String, String)>,
        expected_context: Option<git_operations::RemoteOperationContext>,
        cx: &mut Context<Self>,
    ) {
        let path = self.project_path.clone();
        self.remote_operation_generation = self.remote_operation_generation.wrapping_add(1);
        let generation = self.remote_operation_generation;
        self.pending_auth = None;
        self.op_error = None;
        cx.spawn(async move |this, cx| {
            let attempt = cx
                .background_executor()
                .spawn(async move {
                    git_operations::execute_remote_operation(
                        &path,
                        op.kind(),
                        explicit_creds,
                        expected_context,
                    )
                })
                .await;
            cx.update(|cx| {
                this.update(cx, |gm, cx| {
                    if gm.remote_operation_generation != generation {
                        return;
                    }

                    match attempt.result {
                        Ok(()) => {}
                        Err(error) if is_auth_error(&error) => {
                            if let Some(context) = attempt.context {
                                gm.pending_auth = Some(PendingAuth { op, context });
                                gm.op_error = Some("Authentication required".to_string());
                            } else {
                                gm.op_error = Some(format!("{} failed: {}", op.label(), error));
                            }
                        }
                        Err(error) => {
                            gm.op_error = Some(format!("{} failed: {}", op.label(), error));
                        }
                    }
                    gm.refresh_state(cx);
                });
            });
        })
        .detach();
    }

    fn switch_branch(&mut self, branch_name: String, cx: &mut Context<Self>) {
        let path = self.project_path.clone();
        self.remote_operation_generation = self.remote_operation_generation.wrapping_add(1);
        let generation = self.remote_operation_generation;
        self.pending_auth = None;
        self.op_error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { switch_branch(&path, &branch_name) })
                .await;
            cx.update(|cx| {
                this.update(cx, |gm, cx| {
                    if gm.remote_operation_generation != generation {
                        return;
                    }
                    if let Err(e) = &result {
                        gm.op_error = Some(format!("Switch failed: {}", e));
                    }
                    gm.refresh_state(cx);
                });
            });
        })
        .detach();
    }

    pub(crate) fn toggle_diff_view_mode(&mut self, cx: &mut Context<Self>) {
        self.diff_view_mode = match self.diff_view_mode {
            DiffViewMode::Unified => DiffViewMode::SideBySide,
            DiffViewMode::SideBySide => DiffViewMode::Unified,
        };
        cx.notify();
    }

    fn dismiss_error(&mut self, cx: &mut Context<Self>) {
        self.op_error = None;
        cx.notify();
    }

    fn discard_file_changes(&mut self, path: &str, cx: &mut Context<Self>) {
        let repo_path = self.project_path.clone();
        let file_path = path.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { discard_file_changes(&repo_path, &file_path) })
                .await;
            cx.update(|cx| {
                this.update(cx, |gm, cx| {
                    if let Err(e) = &result {
                        gm.op_error = Some(format!("Discard failed: {}", e));
                    }
                    gm.refresh_state(cx);
                });
            });
        })
        .detach();
    }

    fn append_gitignore(&mut self, line: String, cx: &mut Context<Self>) {
        let repo_path = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { append_to_gitignore(&repo_path, &line) })
                .await;
            cx.update(|cx| {
                this.update(cx, |gm, cx| {
                    if let Err(e) = &result {
                        gm.op_error = Some(format!("Gitignore failed: {}", e));
                    }
                    gm.refresh_state(cx);
                });
            });
        })
        .detach();
    }
    fn open_in_explorer(&mut self, path: &str, _cx: &mut Context<Self>) {
        let full_path = self.project_path.join(path);
        open_in_explorer(&full_path);
    }
}

impl Focusable for GitManager {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GitManager {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let project_path = self.project_path.clone();

        v_flex()
            .size_full()
            .bg(theme.background)
            .key_context("GitManager")
            .on_action(
                cx.listener(|this, action: &DiscardFileChanges, _window, cx| {
                    this.discard_file_changes(&action.path, cx);
                }),
            )
            .on_action(cx.listener(|this, action: &IgnoreFile, _window, cx| {
                let filename = std::path::Path::new(&action.path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&action.path)
                    .to_string();
                this.append_gitignore(filename, cx);
            }))
            .on_action(cx.listener(|this, action: &IgnoreExtension, _window, cx| {
                let ext = std::path::Path::new(&action.path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| format!("*.{}", e))
                    .unwrap_or_default();
                if !ext.is_empty() {
                    this.append_gitignore(ext, cx);
                }
            }))
            .on_action(cx.listener(|this, action: &IgnoreFolder, _window, cx| {
                this.append_gitignore(action.folder.clone(), cx);
            }))
            .on_action(
                cx.listener(move |_this, action: &CopyRelativePath, _window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(action.path.clone()));
                }),
            )
            .on_action(
                cx.listener(move |_this, action: &CopyFullPath, _window, cx| {
                    let full = project_path
                        .join(&action.path)
                        .to_string_lossy()
                        .to_string();
                    cx.write_to_clipboard(ClipboardItem::new_string(full));
                }),
            )
            .on_action(cx.listener(|this, action: &OpenInExplorer, _window, cx| {
                this.open_in_explorer(&action.path, cx);
            }))
            .child(TitleBar::new())
            .child(
                h_flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .w(px(280.))
                            .h_full()
                            .border_r_1()
                            .border_color(theme.border)
                            .overflow_hidden()
                            .child(match self.current_view {
                                GitView::Changes => {
                                    views::render_changes_view(self, cx).into_any_element()
                                }
                                GitView::History => {
                                    views::render_history_view(self, cx).into_any_element()
                                }
                                GitView::Branches => {
                                    views::render_branches_view(self, cx).into_any_element()
                                }
                            }),
                    )
                    .child(v_flex().flex_1().h_full().overflow_hidden().child(
                        match self.current_view {
                            GitView::History => {
                                views::render_commit_detail(self, cx).into_any_element()
                            }
                            _ => views::render_file_panel(self, cx).into_any_element(),
                        },
                    )),
            )
    }
}

/// Type alias for use in the PulsarWindow system.
pub type GitManagerWindow = GitManager;

#[window_manager::register_window]
impl window_manager::PulsarWindow for GitManager {
    type Params = ();

    fn window_name() -> &'static str {
        "GitManagerWindow"
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(1280.0, 800.0)
    }

    fn build(_: (), window: &mut gpui::Window, cx: &mut gpui::App) -> gpui::Entity<Self> {
        let path = engine_state::get_project_path()
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        cx.new(|cx| GitManager::new(path, window, cx))
    }
}
