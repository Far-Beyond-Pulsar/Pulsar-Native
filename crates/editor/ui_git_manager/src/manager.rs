//! `GitManager` behaviour: loading, selection and git operations.
//!
//! Every git call runs on the background executor; results are applied on the
//! UI thread. Calls that can finish out of order (refreshes, history pages,
//! diffs) carry a generation number so a stale answer is dropped.

use crate::history::{self, COMMIT_PAGE, DiffTarget, DiffTexts};
use crate::tree::{TreeItem, build_tree};
use crate::{
    BranchRow, ChangeRow, DetailFocus, DiffViewMode, GitLayout, GitManager, ListState, PendingAuth,
    PendingAuthOp, RepoPhase, git_operations::*, models::*,
};
use gpui::*;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

/// Height of a commit row in the graph layout / the compact layout.
pub(crate) const GRAPH_ROW_HEIGHT: f32 = 28.0;
pub(crate) const COMPACT_ROW_HEIGHT: f32 = 54.0;
/// How often the working tree is re-read while the window is open.
const POLL_INTERVAL: Duration = Duration::from_secs(4);

// ── Persisted layout choice ──────────────────────────────────────────────────

fn settings_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "Pulsar")
        .map(|dirs| dirs.config_dir().join("git_manager.json"))
}

pub(crate) fn load_layout() -> GitLayout {
    let stored = settings_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| value.get("layout").and_then(|v| v.as_str()).map(str::to_string));
    match stored.as_deref() {
        Some("desktop") => GitLayout::Desktop,
        _ => GitLayout::Tree,
    }
}

fn save_layout(layout: GitLayout) {
    let Some(path) = settings_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let name = match layout {
        GitLayout::Tree => "tree",
        GitLayout::Desktop => "desktop",
    };
    let _ = std::fs::write(path, serde_json::json!({ "layout": name }).to_string());
}

fn fingerprint(state: &RepositoryState) -> u64 {
    let mut hasher = DefaultHasher::new();
    state.current_branch.hash(&mut hasher);
    state.detached.hash(&mut hasher);
    state.has_commits.hash(&mut hasher);
    for branch in &state.branches {
        branch.name.hash(&mut hasher);
        branch.is_remote.hash(&mut hasher);
        branch.last_commit.hash(&mut hasher);
    }
    hasher.finish()
}

fn file_stamp(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

impl GitManager {
    // ── Loading ──────────────────────────────────────────────────────────

    pub(crate) fn switch_repository(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.project_path == path {
            return;
        }

        self.project_path = path;
        self.refresh_generation = self.refresh_generation.wrapping_add(1);
        self.history_generation = self.history_generation.wrapping_add(1);
        self.diff_generation = self.diff_generation.wrapping_add(1);
        self.remote_operation_generation = self.remote_operation_generation.wrapping_add(1);

        self.phase = RepoPhase::Loading;
        self.repo_state = RepositoryState::default();
        self.refreshing = false;
        self.history_fingerprint = 0;
        self.changes_rows = Rc::new(Vec::new());
        self.changes_list = ListState::default();
        self.collapsed_changes.clear();
        self.commits.clear();
        self.graph = history::GraphBuilder::default();
        self.graph_lanes = 1;
        self.commits_loading = false;
        self.commits_exhausted = false;
        self.commit_list = ListState::default();
        self.commit_matches = None;
        self.selected_commit = None;
        self.commit_detail = None;
        self.commit_detail_error = None;
        self.commit_file_rows = Rc::new(Vec::new());
        self.commit_files_list = ListState::default();
        self.collapsed_commit_files.clear();
        self.avatar_cache.clear();
        self.target = None;
        self.detail_focus = DetailFocus::Working;
        self.diff_loading = false;
        self.diff_stamp = None;
        self.diff_message = None;
        self.diff.clear(cx);
        self.branch_rows = Rc::new(Vec::new());
        self.branch_list = ListState::default();
        self.collapsed_branches.clear();
        self.branch_menu_open = false;
        self.menu_rows = Rc::new(Vec::new());
        self.menu_list = ListState::default();
        self.op_error = None;
        self.pending_auth = None;
        self.remote_busy = None;
        self.commit_busy = false;
        self.clear_commit_inputs = true;

        self.refresh(true, cx);
        cx.notify();
    }

    /// Re-read the repository. `force_diff` also reloads the open diff.
    pub(crate) fn refresh(&mut self, force_diff: bool, cx: &mut Context<Self>) {
        self.refresh_generation = self.refresh_generation.wrapping_add(1);
        let generation = self.refresh_generation;
        self.refreshing = true;
        let path = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            let probe = cx
                .background_executor()
                .spawn(async move { probe_repository(&path) })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |gm, cx| {
                    if gm.refresh_generation != generation {
                        return;
                    }
                    gm.refreshing = false;
                    if gm.apply_probe(probe, force_diff, cx) {
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    pub(crate) fn refresh_state(&mut self, cx: &mut Context<Self>) {
        self.refresh(true, cx);
    }

    fn apply_probe(
        &mut self,
        probe: Result<RepositoryProbe, git2::Error>,
        force_diff: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        match probe {
            Ok(RepositoryProbe::Repository(state)) => {
                let first_load = !matches!(self.phase, RepoPhase::Ready);
                // The timer re-reads the repository; when nothing changed the
                // rows built last time are still right, so nothing is rebuilt.
                if !first_load && state == self.repo_state {
                    self.retarget_selection(force_diff, cx);
                    return false;
                }
                let print = fingerprint(&state);
                let history_changed = print != self.history_fingerprint;
                self.phase = RepoPhase::Ready;
                self.history_fingerprint = print;
                self.repo_state = state;
                self.rebuild_changes_rows();
                self.rebuild_branch_rows();
                if first_load || history_changed {
                    self.reload_history(cx);
                }
                self.retarget_selection(force_diff, cx);
                true
            }
            Ok(RepositoryProbe::NotARepository) => {
                self.phase = RepoPhase::NotARepository;
                self.repo_state = RepositoryState::default();
                self.commits.clear();
                self.changes_rows = Rc::new(Vec::new());
                self.branch_rows = Rc::new(Vec::new());
                true
            }
            Err(error) => {
                let message = format!(
                    "Could not read the repository at {}: {}",
                    self.project_path.display(),
                    error.message()
                );
                if matches!(self.phase, RepoPhase::Ready) {
                    self.op_error = Some(message);
                } else {
                    self.phase = RepoPhase::Failed(message);
                }
                true
            }
        }
    }

    /// Re-read the working tree on a timer so edits made elsewhere show up.
    pub(crate) fn start_polling(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                let alive = cx.update(|cx| {
                    this.update(cx, |gm, cx| {
                        let idle = !gm.refreshing && gm.remote_busy.is_none() && !gm.commit_busy;
                        if idle && !matches!(gm.phase, RepoPhase::Loading) {
                            gm.refresh(false, cx);
                        }
                    })
                    .is_ok()
                });
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn initialize_repository(&mut self, cx: &mut Context<Self>) {
        let path = self.project_path.clone();
        self.op_error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { init_repository(&path) })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |gm, cx| {
                    if let Err(error) = result {
                        gm.op_error = Some(format!("Initialize failed: {}", error.message()));
                    }
                    gm.refresh(true, cx);
                });
            });
        })
        .detach();
    }

    // ── History ──────────────────────────────────────────────────────────

    pub(crate) fn reload_history(&mut self, cx: &mut Context<Self>) {
        self.history_generation = self.history_generation.wrapping_add(1);
        self.commits.clear();
        self.graph = history::GraphBuilder::default();
        self.graph_lanes = 1;
        self.commit_matches = self.commit_matches.as_ref().map(|_| Rc::new(Vec::new()));
        self.commits_loading = false;
        self.commits_exhausted = false;
        self.load_more_commits(cx);
    }

    /// Load the next page of history; a no-op while one is in flight.
    pub(crate) fn load_more_commits(&mut self, cx: &mut Context<Self>) {
        if self.commits_loading || self.commits_exhausted {
            return;
        }
        self.commits_loading = true;
        let generation = self.history_generation;
        let skip = self.commits.len();
        let graph = self.graph.clone();
        let path = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { history::load_commit_page(&path, skip, COMMIT_PAGE, graph) })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |gm, cx| {
                    if gm.history_generation != generation {
                        return;
                    }
                    gm.commits_loading = false;
                    match result {
                        Ok(page) => {
                            gm.append_commits(page.commits);
                            gm.graph = page.graph;
                            gm.commits_exhausted = page.exhausted;
                            if gm.commit_matches.is_some() {
                                gm.load_more_commits(cx);
                            }
                        }
                        Err(error) => {
                            gm.commits_exhausted = true;
                            gm.op_error = Some(format!("Could not load history: {}", error.message()));
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn append_commits(&mut self, commits: Vec<GraphCommit>) {
        for commit in &commits {
            self.graph_lanes = self.graph_lanes.max(commit.graph.lane_count);
        }
        self.commits.extend(commits);
        self.rebuild_commit_matches();
    }

    /// Commits matching every word of the filter, as indices into `commits`.
    pub(crate) fn rebuild_commit_matches(&mut self) {
        let words: Vec<String> = self
            .commit_filter
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        if words.is_empty() {
            self.commit_matches = None;
            return;
        }
        let matches = self
            .commits
            .iter()
            .enumerate()
            .filter(|(_, commit)| {
                let haystack = format!(
                    "{} {} {} {} {}",
                    commit.summary,
                    commit.author,
                    commit.email,
                    commit.hash,
                    commit.refs.iter().map(|r| r.name.as_str()).collect::<Vec<_>>().join(" ")
                )
                .to_lowercase();
                words.iter().all(|word| haystack.contains(word))
            })
            .map(|(index, _)| index)
            .collect();
        self.commit_matches = Some(Rc::new(matches));
    }

    pub(crate) fn set_commit_filter(&mut self, text: String, cx: &mut Context<Self>) {
        if self.commit_filter == text {
            return;
        }
        self.commit_filter = text;
        self.rebuild_commit_matches();
        // A filter searches the whole history, not only the pages read so far.
        if self.commit_matches.is_some() {
            self.load_more_commits(cx);
        }
        cx.notify();
    }

    /// Ensure the GitHub avatar for a commit author is loaded.
    /// Uses the commit email to derive a GitHub username; falls back gracefully.
    /// Safe to call every render — it's a no-op if the entry is already cached or in-flight.
    pub(crate) fn ensure_avatar_loaded(&mut self, email: &str, cx: &mut Context<Self>) {
        if self.avatar_cache.contains_key(email) {
            return;
        }
        let Some(username) = crate::avatar_loader::github_username_from_email(email) else {
            // Not a GitHub noreply address — remember the miss so it is never retried.
            self.avatar_cache.insert(email.to_string(), None);
            return;
        };
        let url = crate::avatar_loader::avatar_url(&username);
        self.avatar_cache.insert(email.to_string(), None);

        let email_key = email.to_string();
        let (tx, rx) = smol::channel::bounded::<Option<std::sync::Arc<gpui::RenderImage>>>(1);
        std::thread::spawn(move || {
            let result = crate::avatar_loader::fetch_avatar(&url);
            let _ = smol::block_on(tx.send(result.ok()));
        });
        cx.spawn(async move |this, cx| {
            if let Ok(maybe) = rx.recv().await {
                cx.update(|cx| {
                    let _ = this.update(cx, |gm, cx| {
                        gm.avatar_cache.insert(email_key, maybe);
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    // ── Rows ─────────────────────────────────────────────────────────────

    pub(crate) fn rebuild_changes_rows(&mut self) {
        let state = &self.repo_state;
        let mut unstaged: Vec<FileChange> = state.unstaged_files.clone();
        unstaged.extend(
            state
                .untracked_files
                .iter()
                .map(|path| FileChange::new(path.clone(), ChangeStatus::Untracked)),
        );
        let mut staged = state.staged_files.clone();

        // The filter narrows both lists by path, case-insensitively.
        let words: Vec<String> = self
            .changes_filter
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        if !words.is_empty() {
            let keep = |change: &FileChange| {
                let path = change.path.to_lowercase();
                words.iter().all(|word| path.contains(word))
            };
            unstaged.retain(keep);
            staged.retain(keep);
        }

        if self.layout == GitLayout::Desktop {
            // One row per file; the checkbox shows whether it is staged.
            let mut merged: Vec<(String, (FileChange, bool, bool))> = Vec::new();
            let mut index: std::collections::HashMap<String, usize> = Default::default();
            for (list, is_staged) in [(&unstaged, false), (&staged, true)] {
                for change in list {
                    match index.get(&change.path) {
                        Some(&i) => {
                            if is_staged {
                                merged[i].1.1 = true;
                            } else {
                                merged[i].1.2 = true;
                            }
                        }
                        None => {
                            index.insert(change.path.clone(), merged.len());
                            merged.push((
                                change.path.clone(),
                                (change.clone(), is_staged, !is_staged),
                            ));
                        }
                    }
                }
            }
            merged.sort_by_key(|entry| entry.0.to_lowercase());
            self.changes_rows = Rc::new(
                merged
                    .into_iter()
                    .map(|(_, (change, staged, unstaged))| ChangeRow::Flat {
                        change,
                        staged,
                        unstaged,
                    })
                    .collect(),
            );
            return;
        }

        let mut rows = Vec::new();
        for (files, is_staged) in [(staged, true), (unstaged, false)] {
            if files.is_empty() {
                continue;
            }
            let section_key = if is_staged { "section:staged" } else { "section:changes" };
            let collapsed = self.collapsed_changes.contains(section_key);
            rows.push(ChangeRow::Section {
                staged: is_staged,
                count: files.len(),
                collapsed,
            });
            if collapsed {
                continue;
            }
            let items: Vec<(String, FileChange)> = files
                .into_iter()
                .map(|change| (change.path.clone(), change))
                .collect();
            let prefix = if is_staged { "staged" } else { "changes" };
            for item in build_tree(&items, prefix, &self.collapsed_changes, true) {
                rows.push(match item {
                    TreeItem::Folder {
                        key,
                        name,
                        depth,
                        count,
                        collapsed,
                    } => ChangeRow::Folder {
                        key,
                        name,
                        depth,
                        count,
                        collapsed,
                    },
                    TreeItem::Leaf { item, depth } => ChangeRow::File {
                        change: item,
                        staged: is_staged,
                        depth,
                    },
                });
            }
        }
        self.changes_rows = Rc::new(rows);
    }

    pub(crate) fn rebuild_branch_rows(&mut self) {
        let branches = &self.repo_state.branches;

        // Sidebar: local / remote sections as a tree of `feature/…` folders.
        let mut rows = Vec::new();
        for (remote, title, key) in [(false, "Local", "branches:local"), (true, "Remote", "branches:remote")] {
            let items: Vec<(String, Branch)> = branches
                .iter()
                .filter(|b| b.is_remote == remote && !b.name.ends_with("/HEAD"))
                .map(|b| (b.name.clone(), b.clone()))
                .collect();
            if items.is_empty() {
                continue;
            }
            let collapsed = self.collapsed_branches.contains(key);
            rows.push(BranchRow::Section {
                key: key.to_string(),
                title,
                count: items.len(),
                collapsed,
            });
            if collapsed {
                continue;
            }
            for item in build_tree(&items, key, &self.collapsed_branches, true) {
                rows.push(match item {
                    TreeItem::Folder {
                        key,
                        name,
                        depth,
                        collapsed,
                        ..
                    } => BranchRow::Folder {
                        key,
                        name,
                        depth,
                        collapsed,
                    },
                    TreeItem::Leaf { item, depth } => BranchRow::Branch {
                        label: item.name.rsplit('/').next().unwrap_or(&item.name).to_string(),
                        branch: item,
                        depth,
                    },
                });
            }
        }
        self.branch_rows = Rc::new(rows);

        // Menu: filtered, flat, with a "create" entry for a new name.
        let filter = self.branch_filter.trim().to_lowercase();
        let mut menu = Vec::new();
        let typed = self.branch_filter.trim().to_string();
        let exists = branches.iter().any(|b| !b.is_remote && b.name == typed);
        if !typed.is_empty() && !exists && self.repo_state.has_commits {
            menu.push(BranchRow::Create { name: typed });
        }
        for (remote, title, key) in [(false, "Local", "menu:local"), (true, "Remote", "menu:remote")] {
            let mut matching: Vec<&Branch> = branches
                .iter()
                .filter(|b| b.is_remote == remote && !b.name.ends_with("/HEAD"))
                .filter(|b| filter.is_empty() || b.name.to_lowercase().contains(&filter))
                .collect();
            if matching.is_empty() {
                continue;
            }
            matching.sort_by(|a, b| {
                b.is_current
                    .cmp(&a.is_current)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            menu.push(BranchRow::Section {
                key: key.to_string(),
                title,
                count: matching.len(),
                collapsed: false,
            });
            for branch in matching {
                menu.push(BranchRow::Branch {
                    label: branch.name.clone(),
                    branch: branch.clone(),
                    depth: 0,
                });
            }
        }
        self.menu_rows = Rc::new(menu);
    }

    pub(crate) fn rebuild_commit_file_rows(&mut self) {
        let Some(detail) = &self.commit_detail else {
            self.commit_file_rows = Rc::new(Vec::new());
            return;
        };
        let items: Vec<(String, FileChange)> = detail
            .files
            .iter()
            .map(|file| (file.path.clone(), file.clone()))
            .collect();
        self.commit_file_rows = Rc::new(build_tree(
            &items,
            "commit",
            &self.collapsed_commit_files,
            self.layout == GitLayout::Tree,
        ));
    }

    pub(crate) fn toggle_collapsed(&mut self, key: &str, cx: &mut Context<Self>) {
        let set = if key.starts_with("branches:") {
            &mut self.collapsed_branches
        } else if key.starts_with("commit:") {
            &mut self.collapsed_commit_files
        } else {
            &mut self.collapsed_changes
        };
        if !set.remove(key) {
            set.insert(key.to_string());
        }
        self.rebuild_changes_rows();
        self.rebuild_branch_rows();
        self.rebuild_commit_file_rows();
        cx.notify();
    }

    // ── Layout ───────────────────────────────────────────────────────────

    pub(crate) fn set_layout(&mut self, layout: GitLayout, cx: &mut Context<Self>) {
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        save_layout(layout);
        self.rebuild_changes_rows();
        self.rebuild_commit_file_rows();
        cx.notify();
    }

    pub(crate) fn set_desktop_tab(&mut self, tab: crate::DesktopTab, cx: &mut Context<Self>) {
        self.desktop_tab = tab;
        match tab {
            crate::DesktopTab::Changes => self.detail_focus = DetailFocus::Working,
            crate::DesktopTab::History => {
                self.detail_focus = DetailFocus::Commit;
                if self.selected_commit.is_none() {
                    if let Some(first) = self.commits.first().map(|c| c.hash.clone()) {
                        self.select_commit(first, cx);
                        return;
                    }
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn toggle_diff_view_mode(&mut self, cx: &mut Context<Self>) {
        self.diff_mode = match self.diff_mode {
            DiffViewMode::Unified => DiffViewMode::SideBySide,
            DiffViewMode::SideBySide => DiffViewMode::Unified,
        };
        cx.notify();
    }

    // ── Selection and diffs ──────────────────────────────────────────────

    /// Show the diff of a working-tree file.
    pub(crate) fn select_working_file(&mut self, change: &FileChange, staged: bool, cx: &mut Context<Self>) {
        self.detail_focus = DetailFocus::Working;
        let target = if staged {
            DiffTarget::Staged {
                path: change.path.clone(),
                old_path: change.old_path.clone(),
            }
        } else {
            DiffTarget::Unstaged {
                path: change.path.clone(),
            }
        };
        self.load_diff(target, cx);
    }

    pub(crate) fn select_commit(&mut self, hash: String, cx: &mut Context<Self>) {
        self.detail_focus = DetailFocus::Commit;
        self.selected_commit = Some(hash.clone());
        self.commit_detail = None;
        self.commit_detail_error = None;
        self.commit_file_rows = Rc::new(Vec::new());
        self.collapsed_commit_files.clear();
        self.target = None;
        self.diff_message = None;
        self.diff.clear(cx);
        cx.notify();

        let path = self.project_path.clone();
        let wanted = hash.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { history::load_commit_detail(&path, &wanted) })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |gm, cx| {
                    if gm.selected_commit.as_deref() != Some(hash.as_str()) {
                        return;
                    }
                    match result {
                        Ok(detail) => {
                            let first = detail.files.first().cloned();
                            gm.commit_detail = Some(detail);
                            gm.rebuild_commit_file_rows();
                            if let Some(file) = first {
                                gm.select_commit_file(&file, cx);
                            }
                        }
                        Err(error) => gm.commit_detail_error = Some(error.message().to_string()),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(crate) fn select_commit_file(&mut self, file: &FileChange, cx: &mut Context<Self>) {
        let Some(hash) = self.selected_commit.clone() else {
            return;
        };
        self.detail_focus = DetailFocus::Commit;
        self.load_diff(
            DiffTarget::Commit {
                hash,
                path: file.path.clone(),
                old_path: file.old_path.clone(),
            },
            cx,
        );
    }

    fn working_path(&self, relative: &str) -> PathBuf {
        self.project_path.join(relative)
    }

    pub(crate) fn load_diff(&mut self, target: DiffTarget, cx: &mut Context<Self>) {
        self.diff_generation = self.diff_generation.wrapping_add(1);
        let generation = self.diff_generation;
        self.diff_loading = true;
        self.diff_message = None;
        self.diff_stamp = match &target {
            DiffTarget::Unstaged { path } | DiffTarget::Staged { path, .. } => {
                file_stamp(&self.working_path(path))
            }
            DiffTarget::Commit { .. } => None,
        };
        self.target = Some(target.clone());
        cx.notify();

        let repo_path = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            let wanted = target.clone();
            let result = cx
                .background_executor()
                .spawn(async move { history::load_diff_texts(&repo_path, &wanted) })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |gm, cx| {
                    if gm.diff_generation != generation {
                        return;
                    }
                    gm.diff_loading = false;
                    match result {
                        Ok(DiffTexts::Text { old, new }) => {
                            gm.diff_message = None;
                            gm.diff.show(target.path(), &old, &new, cx);
                        }
                        Ok(DiffTexts::Unsupported(reason)) => {
                            gm.diff_message = Some(reason);
                            gm.diff.clear(cx);
                        }
                        Err(error) => {
                            gm.diff_message = Some(error.message().to_string());
                            gm.diff.clear(cx);
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(crate) fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.diff_generation = self.diff_generation.wrapping_add(1);
        self.target = None;
        self.diff_loading = false;
        self.diff_message = None;
        self.diff.clear(cx);
    }

    /// After the repository changed: keep the selected working file pointing
    /// at the side (staged / unstaged) it now lives on, or drop it.
    fn retarget_selection(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(current) = self.target.clone() else {
            return;
        };
        let (path, was_staged) = match &current {
            DiffTarget::Staged { path, .. } => (path.clone(), true),
            DiffTarget::Unstaged { path } => (path.clone(), false),
            DiffTarget::Commit { .. } => return,
        };
        let state = &self.repo_state;
        let staged = state.staged_files.iter().find(|f| f.path == path).cloned();
        let unstaged = state.unstaged_files.iter().any(|f| f.path == path)
            || state.untracked_files.contains(&path);
        let make_staged = |f: FileChange| DiffTarget::Staged {
            path: f.path,
            old_path: f.old_path,
        };
        let make_unstaged = || DiffTarget::Unstaged { path: path.clone() };
        let next = if was_staged {
            staged.clone().map(make_staged).or_else(|| unstaged.then(make_unstaged))
        } else if unstaged {
            Some(make_unstaged())
        } else {
            staged.map(make_staged)
        };
        match next {
            None => self.clear_selection(cx),
            Some(next) => {
                let changed_on_disk = file_stamp(&self.working_path(&path)) != self.diff_stamp;
                if force || next != current || changed_on_disk {
                    self.load_diff(next, cx);
                }
            }
        }
    }

    // ── Staging, discarding, committing ──────────────────────────────────

    /// Run a blocking git call, report a failure, then refresh.
    fn run_blocking<F>(&mut self, what: &'static str, work: F, cx: &mut Context<Self>)
    where
        F: FnOnce(&Path) -> Result<(), git2::Error> + Send + 'static,
    {
        let path = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { work(&path) })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |gm, cx| {
                    if let Err(error) = &result {
                        gm.op_error = Some(format!("{} failed: {}", what, error.message()));
                    }
                    gm.refresh(true, cx);
                });
            });
        })
        .detach();
    }

    pub(crate) fn stage_file(&mut self, file_path: String, cx: &mut Context<Self>) {
        self.run_blocking("Stage", move |repo| stage_file(repo, &file_path), cx);
    }

    pub(crate) fn unstage_file(&mut self, file_path: String, cx: &mut Context<Self>) {
        self.run_blocking("Unstage", move |repo| unstage_file(repo, &file_path), cx);
    }

    pub(crate) fn stage_all(&mut self, cx: &mut Context<Self>) {
        self.run_blocking("Stage all", stage_all_files, cx);
    }

    pub(crate) fn unstage_all(&mut self, cx: &mut Context<Self>) {
        self.run_blocking("Unstage all", unstage_all_files, cx);
    }

    pub(crate) fn discard_file_changes(&mut self, path: &str, cx: &mut Context<Self>) {
        let file_path = path.to_string();
        self.run_blocking("Discard", move |repo| discard_file_changes(repo, &file_path), cx);
    }

    pub(crate) fn append_gitignore(&mut self, line: String, cx: &mut Context<Self>) {
        self.run_blocking("Ignore", move |repo| append_to_gitignore(repo, &line), cx);
    }

    pub(crate) fn open_in_explorer(&mut self, path: &str, _cx: &mut Context<Self>) {
        open_in_explorer(&self.project_path.join(path));
    }

    pub(crate) fn commit_changes(&mut self, cx: &mut Context<Self>) {
        if self.commit_busy {
            return;
        }
        let summary = self.commit_message_input.read(cx).text().to_string();
        let summary = summary.trim().to_string();
        if summary.is_empty() {
            self.op_error = Some("Enter a commit summary first".to_string());
            cx.notify();
            return;
        }
        if self.repo_state.staged_files.is_empty() && self.repo_state.operation.is_none() {
            self.op_error = Some("Stage at least one file to commit".to_string());
            cx.notify();
            return;
        }
        let description = self.commit_description_input.read(cx).text().to_string();
        let message = if description.trim().is_empty() {
            summary
        } else {
            format!("{}\n\n{}", summary, description.trim())
        };

        self.commit_busy = true;
        self.op_error = None;
        let path = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { commit_staged_changes(&path, &message) })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |gm, cx| {
                    gm.commit_busy = false;
                    match result {
                        Ok(()) => gm.clear_commit_inputs = true,
                        Err(error) => {
                            gm.op_error = Some(format!("Commit failed: {}", error.message()));
                        }
                    }
                    gm.refresh(true, cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    // ── Branches ─────────────────────────────────────────────────────────

    pub(crate) fn toggle_branch_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.branch_menu_open = !self.branch_menu_open;
        if self.branch_menu_open {
            self.branch_filter.clear();
            self.branch_filter_input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.rebuild_branch_rows();
            let handle = self.branch_filter_input.focus_handle(cx);
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    pub(crate) fn close_branch_menu(&mut self, cx: &mut Context<Self>) {
        if self.branch_menu_open {
            self.branch_menu_open = false;
            cx.notify();
        }
    }

    fn submit_branch_filter_target(&self) -> Option<BranchRow> {
        self.menu_rows
            .iter()
            .find(|row| matches!(row, BranchRow::Create { .. } | BranchRow::Branch { .. }))
            .cloned()
    }

    /// Enter in the branch menu: create the typed branch, or switch to the
    /// first match.
    pub(crate) fn submit_branch_filter(&mut self, cx: &mut Context<Self>) {
        match self.submit_branch_filter_target() {
            Some(BranchRow::Create { name }) => self.create_branch(name, cx),
            Some(BranchRow::Branch { branch, .. }) => self.switch_branch(branch.name, cx),
            _ => {}
        }
    }

    pub(crate) fn switch_branch(&mut self, branch_name: String, cx: &mut Context<Self>) {
        self.branch_menu_open = false;
        self.remote_operation_generation = self.remote_operation_generation.wrapping_add(1);
        let generation = self.remote_operation_generation;
        self.pending_auth = None;
        self.op_error = None;
        let path = self.project_path.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { switch_branch(&path, &branch_name) })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |gm, cx| {
                    if gm.remote_operation_generation != generation {
                        return;
                    }
                    if let Err(error) = &result {
                        gm.op_error = Some(format!("Switch failed: {}", error.message()));
                    }
                    gm.refresh(true, cx);
                });
            });
        })
        .detach();
    }

    pub(crate) fn create_branch(&mut self, name: String, cx: &mut Context<Self>) {
        self.branch_menu_open = false;
        cx.notify();
        self.run_blocking("Create branch", move |repo| history::create_branch(repo, &name), cx);
    }

    // ── Remote operations ────────────────────────────────────────────────

    pub(crate) fn fetch(&mut self, cx: &mut Context<Self>) {
        self.run_remote_op(PendingAuthOp::Fetch, None, None, cx);
    }

    pub(crate) fn push(&mut self, cx: &mut Context<Self>) {
        self.run_remote_op(PendingAuthOp::Push, None, None, cx);
    }

    pub(crate) fn pull(&mut self, cx: &mut Context<Self>) {
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
        expected_context: Option<RemoteOperationContext>,
        cx: &mut Context<Self>,
    ) {
        let path = self.project_path.clone();
        self.remote_operation_generation = self.remote_operation_generation.wrapping_add(1);
        let generation = self.remote_operation_generation;
        self.pending_auth = None;
        self.op_error = None;
        self.remote_busy = Some(op);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let attempt = cx
                .background_executor()
                .spawn(async move {
                    execute_remote_operation(&path, op.kind(), explicit_creds, expected_context)
                })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |gm, cx| {
                    if gm.remote_operation_generation != generation {
                        return;
                    }
                    gm.remote_busy = None;
                    match attempt.result {
                        Ok(()) => {}
                        Err(error) if is_auth_error(&error) => {
                            if let Some(context) = attempt.context {
                                gm.pending_auth = Some(PendingAuth { op, context });
                                gm.op_error = Some("Authentication required".to_string());
                            } else {
                                gm.op_error = Some(format!("{} failed: {}", op.label(), error.message()));
                            }
                        }
                        Err(error) => {
                            gm.op_error = Some(format!("{} failed: {}", op.label(), error.message()));
                        }
                    }
                    gm.refresh(true, cx);
                });
            });
        })
        .detach();
    }

    pub(crate) fn dismiss_error(&mut self, cx: &mut Context<Self>) {
        self.op_error = None;
        cx.notify();
    }
}
