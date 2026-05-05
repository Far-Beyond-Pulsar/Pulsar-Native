//! Git Manager Window
//!
//! A GitHub Desktop-like Git manager built with GPUI and the UI crate

mod git_operations;
mod models;
mod views;

use gpui::ClipboardItem;
use gpui::*;
use parking_lot::RwLock;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use ui::{
    ActiveTheme as _, TitleBar, h_flex,
    input::{InputEvent, InputState},
    v_flex,
};

pub use git_operations::*;
pub use models::*;

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
    // History view
    selected_commit: Option<String>,
    selected_commit_files: Vec<FileChange>,
    selected_commit_file: Option<String>,
    commit_file_diff: Option<DiffResult>,
    commit_file_diff_error: Option<String>,
    commit_file_expanded: HashSet<usize>,
    /// Last error from a background git operation (push/pull/fetch/switch)
    pub(crate) op_error: Option<String>,
    /// When set, the toolbar shows a credential prompt and retries this op on submit
    pub(crate) pending_auth_op: Option<PendingAuthOp>,
    pub(crate) auth_username_input: Entity<InputState>,
    pub(crate) auth_password_input: Entity<InputState>,
    /// Credentials cached in process memory after a successful auth — reused automatically.
    /// Never written to disk by us; cleared when the window closes.
    stored_creds: Option<(String, String)>,
    current_view: GitView,
    focus_handle: FocusHandle,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GitView {
    Changes,
    History,
    Branches,
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

        // Load initial git state + stored credentials from OS keychain
        let path = project_path.clone();
        cx.spawn(async move |this, cx| {
            let load_path = path.clone();
            let (state_result, stored_creds) = cx
                .background_executor()
                .spawn(async move {
                    let state = load_repository_state(&load_path);
                    let creds = load_git_credentials(&load_path);
                    (state, creds)
                })
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
                    git_manager.stored_creds = stored_creds;
                    cx.notify();
                })
                .ok();
            })
            .ok();
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
            selected_commit: None,
            selected_commit_files: Vec::new(),
            selected_commit_file: None,
            commit_file_diff: None,
            commit_file_diff_error: None,
            commit_file_expanded: HashSet::new(),
            op_error: None,
            pending_auth_op: None,
            auth_username_input,
            auth_password_input,
            stored_creds: None,
            current_view: GitView::Changes,
            focus_handle: cx.focus_handle(),
        }
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
                    cx.notify();
                })
                .ok();
            })
            .ok();
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
                    })
                    .ok();
                })
                .ok();
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
                })
                .ok();
            })
            .ok();
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
                })
                .ok();
            })
            .ok();
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
                })
                .ok();
            })
            .ok();
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
                })
                .ok();
            })
            .ok();
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
                    })
                    .ok();
                })
                .ok();
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
                        }
                        Err(msg) => {
                            gm.commit_file_diff = None;
                            gm.commit_file_diff_error = Some(msg);
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    pub fn select_file(&mut self, file_path: String, cx: &mut Context<Self>) {
        self.selected_file = Some(file_path.clone());
        self.file_diff = None;
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
                        }
                        Err(msg) => {
                            gm.file_diff = None;
                            gm.file_diff_error = Some(msg);
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    pub fn expand_file_diff_region(&mut self, region_idx: usize, cx: &mut Context<Self>) {
        self.file_diff_expanded.insert(region_idx);
        cx.notify();
    }

    pub fn expand_commit_diff_region(&mut self, region_idx: usize, cx: &mut Context<Self>) {
        self.commit_file_expanded.insert(region_idx);
        cx.notify();
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        self.run_remote_op(PendingAuthOp::Fetch, None, cx);
    }

    fn push(&mut self, cx: &mut Context<Self>) {
        self.run_remote_op(PendingAuthOp::Push, None, cx);
    }

    fn pull(&mut self, cx: &mut Context<Self>) {
        self.run_remote_op(PendingAuthOp::Pull, None, cx);
    }

    pub fn retry_with_auth(&mut self, cx: &mut Context<Self>) {
        let op = match self.pending_auth_op {
            Some(op) => op,
            None => return,
        };
        let username = self.auth_username_input.read(cx).text().to_string();
        let password = self.auth_password_input.read(cx).text().to_string();
        self.pending_auth_op = None;
        self.op_error = None;
        self.run_remote_op(op, Some((username, password)), cx);
    }

    pub fn cancel_auth(&mut self, cx: &mut Context<Self>) {
        self.pending_auth_op = None;
        self.op_error = None;
        cx.notify();
    }

    fn run_remote_op(
        &mut self,
        op: PendingAuthOp,
        explicit_creds: Option<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        let path = self.project_path.clone();
        self.op_error = None;
        let cached_creds = self.stored_creds.clone();
        let _explicit_was_provided = explicit_creds.is_some();
        cx.spawn(async move |this, cx| {
            let path_clone = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    // Priority: explicit (retry) → in-memory cache → OS keychain
                    let creds = explicit_creds
                        .or(cached_creds)
                        .or_else(|| load_git_credentials(&path));
                    let creds_used = creds.clone();
                    let res = match op {
                        PendingAuthOp::Fetch => fetch_from_remote(&path, creds),
                        PendingAuthOp::Push => push_to_remote(&path, creds),
                        PendingAuthOp::Pull => pull_from_remote(&path, creds),
                    };
                    (res, creds_used)
                })
                .await;
            let (result, creds_used) = result;
            // On success with any creds: persist to keychain and update memory cache
            if result.is_ok() {
                if let Some((ref user, ref pass)) = creds_used {
                    store_git_credentials(&path_clone, user, pass);
                }
            }
            // Single update — set stored_creds and handle errors together
            cx.update(|cx| {
                this.update(cx, |gm, cx| {
                    match &result {
                        Ok(_) => {
                            // Cache creds in memory on success
                            if let Some(c) = creds_used {
                                gm.stored_creds = Some(c);
                            }
                        }
                        Err(e) if is_auth_error(e) => {
                            // Stale creds failed — clear them and prompt
                            gm.stored_creds = None;
                            gm.pending_auth_op = Some(op);
                            gm.op_error = Some("Authentication required".to_string());
                        }
                        Err(e) => {
                            gm.op_error = Some(format!("{} failed: {}", op.label(), e));
                        }
                    }
                    gm.refresh_state(cx);
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    fn switch_branch(&mut self, branch_name: String, cx: &mut Context<Self>) {
        let path = self.project_path.clone();
        self.op_error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { switch_branch(&path, &branch_name) })
                .await;
            cx.update(|cx| {
                this.update(cx, |gm, cx| {
                    if let Err(e) = &result {
                        gm.op_error = Some(format!("Switch failed: {}", e));
                    }
                    gm.refresh_state(cx);
                })
                .ok();
            })
            .ok();
        })
        .detach();
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
                })
                .ok();
            })
            .ok();
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
                })
                .ok();
            })
            .ok();
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

impl window_manager::PulsarWindow for GitManager {
    type Params = std::path::PathBuf;

    fn window_name() -> &'static str {
        "GitManagerWindow"
    }

    fn window_options(_: &std::path::PathBuf) -> gpui::WindowOptions {
        window_manager::default_window_options(1280.0, 800.0)
    }

    fn build(
        params: std::path::PathBuf,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> gpui::Entity<Self> {
        cx.new(|cx| GitManager::new(params, window, cx))
    }
}
