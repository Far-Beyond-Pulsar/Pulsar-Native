//! Git Manager Window
//!
//! A GitHub Desktop-like Git manager built with GPUI and the UI crate

mod git_operations;
mod models;
mod views;

use gpui::*;
use ui::{Root, v_flex, h_flex, TitleBar, ActiveTheme as _, input::{InputState, InputEvent, TabSize, LineHighlight}};
use std::path::PathBuf;
use parking_lot::RwLock;
use std::sync::Arc;

pub use git_operations::*;
pub use models::*;

/// Main Git Manager window
pub struct GitManager {
    project_path: PathBuf,
    repo_state: Arc<RwLock<RepositoryState>>,
    // Changes view
    selected_file: Option<String>,
    file_content: Option<FileContentResult>,        // non-text error/binary/toolong states
    file_viewer: Option<Entity<InputState>>,        // fresh entity created per-file
    pending_file_content: Option<(String, Vec<DiffLineKind>, &'static str)>, // (content, line_kinds, language) ready to apply
    commit_message_input: Entity<InputState>,
    commit_description_input: Entity<InputState>,
    // History view
    selected_commit: Option<String>,
    selected_commit_files: Vec<FileChange>,
    selected_commit_file: Option<String>,
    selected_commit_file_content: Option<FileContentResult>, // non-text error/binary/toolong states
    commit_file_viewer: Option<Entity<InputState>>,          // fresh entity created per-file
    pending_commit_file_content: Option<(String, Vec<DiffLineKind>, &'static str)>,   // (content, line_kinds, language) ready to apply
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

        // Pre-create read-only code viewers (content set lazily via pending fields + render)
        // Subscribe to Enter key events on commit message input
        cx.subscribe(&commit_message_input, |this, _input, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { secondary: false } = event {
                this.commit_changes(cx);
            }
        }).detach();

        // Load initial git state
        let path = project_path.clone();
        cx.spawn(async move |this, mut cx| {
            if let Ok(state) = cx.background_executor().spawn(async move { load_repository_state(&path) }).await {
                cx.update(|cx| {
                    this.update(cx, |git_manager, cx| {
                        *git_manager.repo_state.write() = state;
                        cx.notify();
                    }).ok();
                }).ok();
            }
        }).detach();

        Self {
            project_path,
            repo_state,
            selected_file: None,
            file_content: None,
            file_viewer: None,
            pending_file_content: None,
            commit_message_input,
            commit_description_input,
            selected_commit: None,
            selected_commit_files: Vec::new(),
            selected_commit_file: None,
            selected_commit_file_content: None,
            commit_file_viewer: None,
            pending_commit_file_content: None,
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
        cx.spawn(async move |this, mut cx| {
            if let Ok(state) = cx.background_executor().spawn(async move { load_repository_state(&path) }).await {
                cx.update(|cx| {
                    this.update(cx, |git_manager, cx| {
                        *git_manager.repo_state.write() = state;
                        cx.notify();
                    }).ok();
                }).ok();
            }
        }).detach();
    }

    fn commit_changes(&mut self, cx: &mut Context<Self>) {
        let message = self.commit_message_input.read(cx).text().to_string();
        if message.trim().is_empty() {
            return;
        }

        let path = self.project_path.clone();

        cx.spawn(async move |this, mut cx| {
            let result = cx.background_executor().spawn(async move { commit_staged_changes(&path, &message) }).await;
            if result.is_ok() {
                cx.update(|cx| {
                    this.update(cx, |git_manager, cx| {
                        git_manager.refresh_state(cx);
                    }).ok();
                }).ok();
            }
        }).detach();
    }

    fn stage_file(&mut self, file_path: String, cx: &mut Context<Self>) {
        let path = self.project_path.clone();
        cx.spawn(async move |this, mut cx| {
            let result = cx.background_executor().spawn(async move { stage_file(&path, &file_path) }).await;
            if result.is_ok() {
                cx.update(|cx| {
                    this.update(cx, |git_manager, cx| { git_manager.refresh_state(cx); }).ok();
                }).ok();
            }
        }).detach();
    }

    fn unstage_file(&mut self, file_path: String, cx: &mut Context<Self>) {
        let path = self.project_path.clone();
        cx.spawn(async move |this, mut cx| {
            let result = cx.background_executor().spawn(async move { unstage_file(&path, &file_path) }).await;
            if result.is_ok() {
                cx.update(|cx| {
                    this.update(cx, |git_manager, cx| { git_manager.refresh_state(cx); }).ok();
                }).ok();
            }
        }).detach();
    }

    fn select_commit(&mut self, commit_hash: String, cx: &mut Context<Self>) {
        self.selected_commit = Some(commit_hash.clone());
        self.selected_commit_files = Vec::new();
        self.selected_commit_file = None;
        self.selected_commit_file_content = None;
        cx.notify();

        let path = self.project_path.clone();
        cx.spawn(async move |this, mut cx| {
            if let Ok(files) = cx.background_executor().spawn(async move { get_commit_files(&path, &commit_hash) }).await {
                cx.update(|cx| {
                    this.update(cx, |git_manager, cx| {
                        git_manager.selected_commit_files = files;
                        cx.notify();
                    }).ok();
                }).ok();
            }
        }).detach();
    }

    pub fn select_commit_file(&mut self, file_path: String, cx: &mut Context<Self>) {
        let commit_hash = match &self.selected_commit {
            Some(h) => h.clone(),
            None => return,
        };
        self.selected_commit_file = Some(file_path.clone());
        self.selected_commit_file_content = None;
        self.commit_file_viewer = None;
        self.pending_commit_file_content = None;
        cx.notify();

        let repo_path = self.project_path.clone();
        cx.spawn(async move |this, mut cx| {
            let result = cx.background_executor()
                .spawn(async move { load_file_diff_at_commit(&repo_path, &commit_hash, &file_path) })
                .await;
            cx.update(|cx| {
                this.update(cx, |git_manager, cx| {
                    match result {
                        Ok(diff) => {
                            let lang = detect_language(git_manager.selected_commit_file.as_deref().unwrap_or(""));
                            git_manager.pending_commit_file_content = Some((diff.text, diff.line_kinds, lang));
                            git_manager.selected_commit_file_content = None;
                        }
                        Err(msg) => {
                            git_manager.selected_commit_file_content = Some(FileContentResult::Error(msg));
                            git_manager.pending_commit_file_content = None;
                        }
                    }
                    cx.notify();
                }).ok();
            }).ok();
        }).detach();
    }

    pub fn select_file(&mut self, file_path: String, cx: &mut Context<Self>) {
        self.selected_file = Some(file_path.clone());
        self.file_content = None;
        self.file_viewer = None;
        self.pending_file_content = None;
        cx.notify();
        let repo_path = self.project_path.clone();
        cx.spawn(async move |this, mut cx| {
            let result = cx.background_executor()
                .spawn(async move { load_file_diff_working(&repo_path, &file_path) })
                .await;
            cx.update(|cx| {
                this.update(cx, |git_manager, cx| {
                    match result {
                        Ok(diff) => {
                            let lang = detect_language(git_manager.selected_file.as_deref().unwrap_or(""));
                            git_manager.pending_file_content = Some((diff.text, diff.line_kinds, lang));
                            git_manager.file_content = None;
                        }
                        Err(msg) => {
                            git_manager.file_content = Some(FileContentResult::Error(msg));
                            git_manager.pending_file_content = None;
                        }
                    }
                    cx.notify();
                }).ok();
            }).ok();
        }).detach();
    }

    fn push(&mut self, cx: &mut Context<Self>) {
        let path = self.project_path.clone();
        cx.spawn(async move |this, mut cx| {
            let _ = cx.background_executor().spawn(async move { push_to_remote(&path) }).await;
            cx.update(|cx| {
                this.update(cx, |git_manager, cx| { git_manager.refresh_state(cx); }).ok();
            }).ok();
        }).detach();
    }

    fn pull(&mut self, cx: &mut Context<Self>) {
        let path = self.project_path.clone();
        cx.spawn(async move |this, mut cx| {
            let _ = cx.background_executor().spawn(async move { pull_from_remote(&path) }).await;
            cx.update(|cx| {
                this.update(cx, |git_manager, cx| { git_manager.refresh_state(cx); }).ok();
            }).ok();
        }).detach();
    }

    fn switch_branch(&mut self, branch_name: String, cx: &mut Context<Self>) {
        let path = self.project_path.clone();
        cx.spawn(async move |this, mut cx| {
            let result = cx.background_executor().spawn(async move { switch_branch(&path, &branch_name) }).await;
            if result.is_ok() {
                cx.update(|cx| {
                    this.update(cx, |git_manager, cx| { git_manager.refresh_state(cx); }).ok();
                }).ok();
            }
        }).detach();
    }
}

impl Focusable for GitManager {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GitManager {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Apply any pending diff content — create a fresh Entity<InputState> per file
        if let Some((content, line_kinds, lang)) = self.pending_file_content.take() {
            let highlights: Vec<LineHighlight> = line_kinds.iter().map(|k| match k {
                DiffLineKind::Added   => LineHighlight::Added,
                DiffLineKind::Removed => LineHighlight::Removed,
                _                     => LineHighlight::None,
            }).collect();
            let viewer = cx.new(|cx| {
                let mut state = InputState::new(window, cx)
                    .code_editor(lang)
                    .tab_size(TabSize { tab_size: 4, hard_tabs: false });
                state.set_line_highlights(highlights);
                state.set_value(&content, window, cx);
                state
            });
            self.file_viewer = Some(viewer);
        }
        if let Some((content, line_kinds, lang)) = self.pending_commit_file_content.take() {
            let highlights: Vec<LineHighlight> = line_kinds.iter().map(|k| match k {
                DiffLineKind::Added   => LineHighlight::Added,
                DiffLineKind::Removed => LineHighlight::Removed,
                _                     => LineHighlight::None,
            }).collect();
            let viewer = cx.new(|cx| {
                let mut state = InputState::new(window, cx)
                    .code_editor(lang)
                    .tab_size(TabSize { tab_size: 4, hard_tabs: false });
                state.set_line_highlights(highlights);
                state.set_value(&content, window, cx);
                state
            });
            self.commit_file_viewer = Some(viewer);
        }

        let theme = cx.theme();

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(TitleBar::new())
            .child(
                h_flex()
                    .flex_1()
                    .overflow_hidden()
                    // Left panel — navigation + file list (fixed width like GH Desktop)
                    .child(
                        v_flex()
                            .w(px(280.))
                            .h_full()
                            .border_r_1()
                            .border_color(theme.border)
                            .overflow_hidden()
                            .child(
                                match self.current_view {
                                    GitView::Changes => views::render_changes_view(self, cx).into_any_element(),
                                    GitView::History => views::render_history_view(self, cx).into_any_element(),
                                    GitView::Branches => views::render_branches_view(self, cx).into_any_element(),
                                }
                            )
                    )
                    // Right panel — file content viewer or commit detail
                    .child(
                        v_flex()
                            .flex_1()
                            .h_full()
                            .overflow_hidden()
                            .child(
                                match self.current_view {
                                    GitView::History => views::render_commit_detail(self, cx).into_any_element(),
                                    _ => views::render_file_panel(self, cx).into_any_element(),
                                }
                            )
                    )
            )
    }
}

/// Create a Git Manager window component
pub fn create_git_manager_component(
    window: &mut Window,
    cx: &mut App,
    project_path: PathBuf,
) -> Entity<Root> {
    let git_manager = cx.new(|cx| GitManager::new(project_path, window, cx));
    cx.new(|cx| Root::new(git_manager.into(), window, cx))
}

/// Detect syntax highlighting language from file path extension
pub fn detect_language(file_path: &str) -> &'static str {
    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext.to_lowercase().as_str() {
        "rs" => "rust",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" => "typescript",
        "py" => "python",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" => "cpp",
        "java" => "java",
        "json" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "md" | "markdown" => "markdown",
        "html" | "htm" => "html",
        "css" => "css",
        "xml" => "xml",
        "sh" | "bash" | "zsh" => "bash",
        "lua" => "lua",
        "wgsl" | "glsl" | "hlsl" => "glsl",
        _ => "text",
    }
}
