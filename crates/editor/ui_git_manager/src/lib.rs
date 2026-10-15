//! Git Manager Window
//!
//! A full Git client built with GPUI and the UI crate. It has two layouts
//! over the same repository state:
//!
//! * **Tree** — an IDE style layout: a sidebar tree of changes and branches,
//!   a virtualized commit graph, and a commit / diff detail pane.
//! * **Desktop** — a GitHub Desktop style layout: a Changes / History tab
//!   strip on the left and the diff on the right.
//!
//! Diffs are shown in read-only Mockaco editors, the same editor the code
//! editor plugin uses (see [`diff_pane`]).

mod auto_fetch;
mod avatar_loader;
mod components;
mod diff_pane;
mod git_hooks;
mod git_operations;
mod handlers;
mod history;
mod manager;
mod models;
mod tree;
mod utils;
mod views;

use gpui::ClipboardItem;
use gpui::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use ui::{
    ActiveTheme as _, TitleBar,
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
// GitManagerWindow / GitManagerParams are defined below alongside the
// PulsarWindow impl; both are public items on this module directly.
pub use models::*;

use diff_pane::DiffPane;
use history::{DiffTarget, GraphBuilder};
use tree::TreeItem;

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

    pub(crate) fn progress_label(self) -> &'static str {
        match self {
            PendingAuthOp::Fetch => "Fetching…",
            PendingAuthOp::Push => "Pushing…",
            PendingAuthOp::Pull => "Pulling…",
        }
    }

    pub(crate) fn kind(self) -> git_operations::RemoteOperationKind {
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

/// The two window layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitLayout {
    /// Sidebar tree + commit graph + detail pane.
    Tree,
    /// GitHub Desktop style Changes / History tabs + diff.
    Desktop,
}

/// Tabs of the desktop layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopTab {
    Changes,
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffViewMode {
    Unified,
    SideBySide,
}

/// What the detail pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetailFocus {
    Commit,
    Working,
}

/// Where the repository load stands.
#[derive(Debug, Clone)]
pub(crate) enum RepoPhase {
    Loading,
    /// The folder is not inside a git repository.
    NotARepository,
    Failed(String),
    Ready,
}

/// Scroll handle + scrollbar of a virtualized list.
#[derive(Clone)]
pub(crate) struct ListState {
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) bar: ScrollbarState,
}

impl Default for ListState {
    fn default() -> Self {
        Self {
            scroll: UniformListScrollHandle::new(),
            bar: ScrollbarState::default(),
        }
    }
}

/// A row of the changes list.
#[derive(Clone)]
pub(crate) enum ChangeRow {
    /// Tree layout: "Staged" / "Changes" section header.
    Section {
        staged: bool,
        count: usize,
        collapsed: bool,
    },
    Folder {
        key: String,
        name: String,
        depth: usize,
        count: usize,
        collapsed: bool,
    },
    File {
        change: FileChange,
        staged: bool,
        depth: usize,
    },
    /// Desktop layout: one row per file; the checkbox shows the staged state.
    Flat {
        change: FileChange,
        /// Some of the file is staged.
        staged: bool,
        /// Some of the file is not staged.
        unstaged: bool,
    },
}

/// A row of a branch list.
#[derive(Clone)]
pub(crate) enum BranchRow {
    Section {
        key: String,
        title: &'static str,
        count: usize,
        collapsed: bool,
    },
    Folder {
        key: String,
        name: String,
        depth: usize,
        collapsed: bool,
    },
    Branch {
        branch: Branch,
        label: String,
        depth: usize,
    },
    /// "Create branch <name>" entry of the branch menu.
    Create { name: String },
}

/// Main Git Manager window
pub struct GitManager {
    pub(crate) project_path: PathBuf,
    pub(crate) phase: RepoPhase,
    pub(crate) repo_state: RepositoryState,
    pub(crate) layout: GitLayout,
    pub(crate) desktop_tab: DesktopTab,
    pub(crate) diff_mode: DiffViewMode,
    pub(crate) refreshing: bool,
    refresh_generation: u64,

    // ── Changes ──────────────────────────────────────────────────────────
    pub(crate) changes_rows: Rc<Vec<ChangeRow>>,
    pub(crate) changes_list: ListState,
    pub(crate) collapsed_changes: HashSet<String>,
    pub(crate) changes_filter_input: Entity<InputState>,
    pub(crate) changes_filter: String,
    pub(crate) commit_message_input: Entity<InputState>,
    pub(crate) commit_description_input: Entity<InputState>,
    pub(crate) commit_busy: bool,
    clear_commit_inputs: bool,

    // ── History ──────────────────────────────────────────────────────────
    pub(crate) commits: Vec<GraphCommit>,
    pub(crate) graph: GraphBuilder,
    pub(crate) graph_lanes: usize,
    pub(crate) commits_loading: bool,
    pub(crate) commits_exhausted: bool,
    history_generation: u64,
    history_fingerprint: u64,
    pub(crate) commit_list: ListState,
    pub(crate) commit_filter_input: Entity<InputState>,
    pub(crate) commit_filter: String,
    /// Indices into `commits` of the commits matching the filter; `None`
    /// while no filter is typed.
    pub(crate) commit_matches: Option<Rc<Vec<usize>>>,
    pub(crate) selected_commit: Option<String>,
    pub(crate) commit_detail: Option<CommitDetail>,
    pub(crate) commit_detail_error: Option<String>,
    pub(crate) commit_file_rows: Rc<Vec<TreeItem<FileChange>>>,
    pub(crate) commit_files_list: ListState,
    pub(crate) collapsed_commit_files: HashSet<String>,
    /// Avatar image cache keyed by commit email. `None` = fetch in-flight.
    pub(crate) avatar_cache: HashMap<String, Option<Arc<gpui::RenderImage>>>,

    // ── Diff ─────────────────────────────────────────────────────────────
    pub(crate) target: Option<DiffTarget>,
    pub(crate) detail_focus: DetailFocus,
    pub(crate) diff: DiffPane,
    pub(crate) diff_message: Option<String>,
    pub(crate) diff_loading: bool,
    diff_generation: u64,
    /// Modification stamp of the working-tree file being diffed.
    diff_stamp: Option<(std::time::SystemTime, u64)>,

    // ── Branches ─────────────────────────────────────────────────────────
    pub(crate) branch_rows: Rc<Vec<BranchRow>>,
    pub(crate) branch_list: ListState,
    pub(crate) collapsed_branches: HashSet<String>,
    pub(crate) branch_menu_open: bool,
    pub(crate) branch_filter_input: Entity<InputState>,
    branch_filter: String,
    pub(crate) menu_rows: Rc<Vec<BranchRow>>,
    pub(crate) menu_list: ListState,

    // ── Remote operations ────────────────────────────────────────────────
    /// Last error from a background git operation (push/pull/fetch/switch)
    pub(crate) op_error: Option<String>,
    /// When set, the top bar shows a credential prompt bound to the failed operation target.
    pub(crate) pending_auth: Option<PendingAuth>,
    pub(crate) remote_busy: Option<PendingAuthOp>,
    pub(crate) remote_operation_generation: u64,
    pub(crate) auth_username_input: Entity<InputState>,
    pub(crate) auth_password_input: Entity<InputState>,

    focus_handle: FocusHandle,
}

impl GitManager {
    pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let commit_message_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_placeholder("Summary (required)", window, cx);
            input
        });
        let commit_description_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).multi_line().auto_grow(2, 5);
            input.set_placeholder("Description (optional)", window, cx);
            input
        });
        cx.subscribe(
            &commit_message_input,
            |this, _input, event: &InputEvent, cx| {
                if let InputEvent::PressEnter { secondary: false } = event {
                    this.commit_changes(cx);
                }
            },
        )
        .detach();

        let branch_filter_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_placeholder("Filter or create a branch", window, cx);
            input
        });
        cx.subscribe(
            &branch_filter_input,
            |this, input, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    this.branch_filter = input.read(cx).text().to_string();
                    this.rebuild_branch_rows();
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.submit_branch_filter(cx),
                _ => {}
            },
        )
        .detach();

        let changes_filter_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_placeholder("Filter changed files", window, cx);
            input
        });
        cx.subscribe(
            &changes_filter_input,
            |this, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.changes_filter = input.read(cx).text().to_string();
                    this.rebuild_changes_rows();
                    cx.notify();
                }
            },
        )
        .detach();

        let commit_filter_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_placeholder("Filter commits by message, author, hash or branch", window, cx);
            input
        });
        cx.subscribe(
            &commit_filter_input,
            |this, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let text = input.read(cx).text().to_string();
                    this.set_commit_filter(text, cx);
                }
            },
        )
        .detach();

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

        let diff = DiffPane::new(cx);
        diff.link_scrolling(cx);

        let mut manager = Self {
            project_path,
            phase: RepoPhase::Loading,
            repo_state: RepositoryState::default(),
            layout: manager::load_layout(),
            desktop_tab: DesktopTab::Changes,
            diff_mode: DiffViewMode::SideBySide,
            refreshing: false,
            refresh_generation: 0,
            changes_rows: Rc::new(Vec::new()),
            changes_list: ListState::default(),
            collapsed_changes: HashSet::new(),
            changes_filter_input,
            changes_filter: String::new(),
            commit_message_input,
            commit_description_input,
            commit_busy: false,
            clear_commit_inputs: false,
            commits: Vec::new(),
            graph: GraphBuilder::default(),
            graph_lanes: 1,
            commits_loading: false,
            commits_exhausted: false,
            history_generation: 0,
            history_fingerprint: 0,
            commit_list: ListState::default(),
            commit_filter_input,
            commit_filter: String::new(),
            commit_matches: None,
            selected_commit: None,
            commit_detail: None,
            commit_detail_error: None,
            commit_file_rows: Rc::new(Vec::new()),
            commit_files_list: ListState::default(),
            collapsed_commit_files: HashSet::new(),
            avatar_cache: HashMap::new(),
            target: None,
            detail_focus: DetailFocus::Working,
            diff,
            diff_message: None,
            diff_loading: false,
            diff_generation: 0,
            diff_stamp: None,
            branch_rows: Rc::new(Vec::new()),
            branch_list: ListState::default(),
            collapsed_branches: HashSet::new(),
            branch_menu_open: false,
            branch_filter_input,
            branch_filter: String::new(),
            menu_rows: Rc::new(Vec::new()),
            menu_list: ListState::default(),
            op_error: None,
            pending_auth: None,
            remote_busy: None,
            remote_operation_generation: 0,
            auth_username_input,
            auth_password_input,
            focus_handle: cx.focus_handle(),
        };
        manager.refresh(true, cx);
        manager.start_polling(cx);
        manager
    }
}

impl Focusable for GitManager {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GitManager {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.clear_commit_inputs {
            self.clear_commit_inputs = false;
            self.commit_message_input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.commit_description_input
                .update(cx, |input, cx| input.set_value("", window, cx));
        }

        let background = cx.theme().background;
        let project_path = self.project_path.clone();

        let body: AnyElement = match self.phase.clone() {
            RepoPhase::Loading => views::render_loading(cx).into_any_element(),
            RepoPhase::NotARepository => views::render_not_a_repository(self, cx).into_any_element(),
            RepoPhase::Failed(message) => views::render_failed(self, message, cx).into_any_element(),
            RepoPhase::Ready => match self.layout {
                GitLayout::Tree => views::render_tree_layout(self, cx).into_any_element(),
                GitLayout::Desktop => views::render_desktop_layout(self, cx).into_any_element(),
            },
        };
        let top_bar = views::render_top_bar(self, cx);
        let banners = views::render_banners(self, cx);
        let menu = if self.branch_menu_open && matches!(self.phase, RepoPhase::Ready) {
            Some(views::render_branch_menu(self, cx))
        } else {
            None
        };

        v_flex()
            .size_full()
            .relative()
            .bg(background)
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
            .child(top_bar)
            .child(banners)
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
            .children(menu)
    }
}

/// Type alias for use in the PulsarWindow system.
pub type GitManagerWindow = GitManager;

/// Parameters for opening a [`GitManagerWindow`].
///
/// `project_path: None` falls back to the globally-loaded project from
/// `EngineContext` (matching the historical zero-param behaviour).
#[derive(Debug, Clone, Default)]
pub struct GitManagerParams {
    pub project_path: Option<PathBuf>,
}

impl GitManagerParams {
    pub fn scoped(project_path: impl Into<PathBuf>) -> Self {
        Self {
            project_path: Some(project_path.into()),
        }
    }
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for GitManagerWindow {
    type Params = GitManagerParams;

    fn window_name() -> &'static str {
        "GitManagerWindow"
    }

    fn window_options(_: &GitManagerParams) -> gpui::WindowOptions {
        window_manager::default_window_options(1360.0, 840.0)
    }

    fn build(
        params: GitManagerParams,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> gpui::Entity<Self> {
        let path = params
            .project_path
            .or_else(|| engine_state::get_project_path().map(std::path::PathBuf::from))
            .unwrap_or_default();
        cx.new(|cx| GitManager::new(path, window, cx))
    }
}

impl GitManagerWindow {
    /// Register the `"GitManagerWindow"` opener in the global
    /// [`window_manager::WindowRegistry`].
    ///
    /// Call once during app startup (parameterised windows are not
    /// auto-registered by `#[register_window]`).
    pub fn init(cx: &mut gpui::App) {
        use ui_common::PulsarWindowExt;
        <Self as PulsarWindowExt>::register(cx);
    }

    /// Open a git manager window rooted at a specific project.
    ///
    /// Focuses the existing window if one is already open.
    pub fn open_scoped(project_path: Option<PathBuf>, cx: &mut gpui::App) {
        use ui_common::PulsarWindowExt;
        Self::open(GitManagerParams { project_path }, cx);
    }
}
