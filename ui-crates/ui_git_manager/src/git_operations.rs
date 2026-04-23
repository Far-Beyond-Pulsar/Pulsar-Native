//! Git operations using git2

use crate::models::*;
use git2::{BranchType, Repository, StatusOptions};
use std::path::Path;

/// Load the complete repository state (blocking — run on background executor)
pub fn load_repository_state(path: &Path) -> Result<RepositoryState, git2::Error> {
    let repo = Repository::open(path)?;

    let head = repo.head()?;
    let current_branch = head.shorthand().unwrap_or("HEAD").to_string();

    let branches = load_branches(&repo)?;
    let commits = load_commits(&repo, 100)?;
    let (staged_files, unstaged_files, untracked_files) = load_file_changes(&repo)?;
    let (ahead, behind) = get_ahead_behind(&repo)?;

    Ok(RepositoryState {
        current_branch,
        branches,
        commits,
        staged_files,
        unstaged_files,
        untracked_files,
        ahead,
        behind,
    })
}

fn load_branches(repo: &Repository) -> Result<Vec<Branch>, git2::Error> {
    let mut branches = Vec::new();

    // Load local branches
    for branch in repo.branches(Some(BranchType::Local))? {
        let (branch, _) = branch?;
        let name = branch.name()?.unwrap_or("").to_string();
        let is_current = branch.is_head();
        let last_commit = branch
            .get()
            .peel_to_commit()
            .ok()
            .map(|c| c.id().to_string());

        branches.push(Branch {
            name,
            is_current,
            is_remote: false,
            last_commit,
        });
    }

    // Load remote branches
    for branch in repo.branches(Some(BranchType::Remote))? {
        let (branch, _) = branch?;
        let name = branch.name()?.unwrap_or("").to_string();
        let last_commit = branch
            .get()
            .peel_to_commit()
            .ok()
            .map(|c| c.id().to_string());

        branches.push(Branch {
            name,
            is_current: false,
            is_remote: true,
            last_commit,
        });
    }

    Ok(branches)
}

fn load_commits(repo: &Repository, limit: usize) -> Result<Vec<Commit>, git2::Error> {
    let mut commits = Vec::new();
    let mut revwalk = repo.revwalk()?;
    revwalk.push_head()?;
    revwalk.set_sorting(git2::Sort::TIME)?;

    for oid in revwalk.take(limit) {
        let oid = oid?;
        let commit = repo.find_commit(oid)?;

        let author = commit.author();
        let message = commit.message().unwrap_or("").to_string();
        let timestamp = chrono::DateTime::from_timestamp(commit.time().seconds(), 0)
            .unwrap_or_else(chrono::Utc::now);

        // Count files changed (simplified)
        let tree = commit.tree()?;
        let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
        let files_changed = if let Some(parent_tree) = parent_tree {
            let diff = repo.diff_tree_to_tree(Some(&parent_tree), Some(&tree), None)?;
            diff.deltas().len()
        } else {
            tree.len()
        };

        commits.push(Commit {
            hash: oid.to_string(),
            short_hash: format!("{:.7}", oid),
            author: author.name().unwrap_or("Unknown").to_string(),
            email: author.email().unwrap_or("").to_string(),
            message,
            timestamp,
            files_changed,
        });
    }

    Ok(commits)
}

fn load_file_changes(
    repo: &Repository,
) -> Result<(Vec<FileChange>, Vec<FileChange>, Vec<String>), git2::Error> {
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();

    let mut opts = StatusOptions::new();
    opts.include_untracked(true);
    opts.recurse_untracked_dirs(true);

    let statuses = repo.statuses(Some(&mut opts))?;

    for entry in statuses.iter() {
        let path = entry.path().unwrap_or("").to_string();
        let status = entry.status();

        // Staged changes
        if status.is_index_new() || status.is_index_modified() || status.is_index_deleted() {
            let change_status = if status.is_index_new() {
                ChangeStatus::Added
            } else if status.is_index_deleted() {
                ChangeStatus::Deleted
            } else if status.is_index_renamed() {
                ChangeStatus::Renamed
            } else {
                ChangeStatus::Modified
            };

            staged.push(FileChange {
                path: path.clone(),
                status: change_status,
                additions: 0, // Would need diff to calculate
                deletions: 0,
            });
        }

        // Unstaged changes
        if status.is_wt_modified() || status.is_wt_deleted() {
            let change_status = if status.is_wt_deleted() {
                ChangeStatus::Deleted
            } else {
                ChangeStatus::Modified
            };

            unstaged.push(FileChange {
                path: path.clone(),
                status: change_status,
                additions: 0,
                deletions: 0,
            });
        }

        // Untracked files
        if status.is_wt_new() {
            untracked.push(path);
        }
    }

    Ok((staged, unstaged, untracked))
}

fn get_ahead_behind(repo: &Repository) -> Result<(usize, usize), git2::Error> {
    let head = repo.head()?;
    if !head.is_branch() {
        return Ok((0, 0));
    }

    let local_oid = head
        .target()
        .ok_or_else(|| git2::Error::from_str("No target"))?;
    let branch = repo.find_branch(head.shorthand().unwrap_or(""), BranchType::Local)?;
    let upstream = branch.upstream()?;
    let upstream_oid = upstream
        .get()
        .target()
        .ok_or_else(|| git2::Error::from_str("No upstream target"))?;

    let (ahead, behind) = repo.graph_ahead_behind(local_oid, upstream_oid)?;
    Ok((ahead, behind))
}

/// Stage a file (blocking — run on background executor)
pub fn stage_file(repo_path: &Path, file_path: &str) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let mut index = repo.index()?;
    let git_path = file_path.replace('\\', "/");
    index.add_path(Path::new(&git_path))?;
    index.write()?;
    Ok(())
}

/// Stage all modified/untracked files (blocking — run on background executor)
pub fn stage_all_files(repo_path: &Path) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let mut index = repo.index()?;
    index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
    index.write()?;
    Ok(())
}

/// Discard working-tree changes for a file (blocking — run on background executor).
/// For untracked files, deletes the file. For modified/deleted, checks out HEAD version.
pub fn discard_file_changes(repo_path: &Path, file_path: &str) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let git_path = file_path.replace('\\', "/");

    // Check status to determine how to discard
    let mut opts = StatusOptions::new();
    opts.pathspec(&git_path)
        .include_untracked(true)
        .include_ignored(false);
    let statuses = repo.statuses(Some(&mut opts))?;
    let status = statuses
        .iter()
        .next()
        .map(|s| s.status())
        .unwrap_or(git2::Status::empty());

    if status.contains(git2::Status::WT_NEW) {
        // Untracked — delete the file
        let full_path = repo_path.join(file_path);
        std::fs::remove_file(&full_path).map_err(|e| git2::Error::from_str(&e.to_string()))?;
        return Ok(());
    }

    if status.contains(git2::Status::INDEX_NEW) {
        // Staged as new (no HEAD version) — just remove from index and delete working copy
        let mut index = repo.index()?;
        index.remove_path(std::path::Path::new(&git_path))?;
        index.write()?;
        let full_path = repo_path.join(file_path);
        std::fs::remove_file(&full_path).ok();
        return Ok(());
    }

    // Modified/deleted tracked file — checkout from HEAD
    let head = repo.head()?;
    let head_commit = head.peel_to_commit()?;
    let head_tree = head_commit.tree()?;

    let mut checkout = git2::build::CheckoutBuilder::default();
    checkout
        .path(std::path::Path::new(&git_path))
        .force()
        .update_index(false);
    repo.checkout_tree(head_tree.as_object(), Some(&mut checkout))?;
    Ok(())
}

/// Open a path in the OS file explorer
pub fn open_in_explorer(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        // Use /select to highlight the file, or just open the folder
        let arg = if path.is_file() {
            format!("/select,{}", path.to_string_lossy())
        } else {
            path.to_string_lossy().to_string()
        };
        let _ = std::process::Command::new("explorer").arg(arg).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        if path.is_file() {
            let _ = std::process::Command::new("open")
                .arg("-R")
                .arg(path)
                .spawn();
        } else {
            let _ = std::process::Command::new("open").arg(path).spawn();
        }
    }
    #[cfg(target_os = "linux")]
    {
        let dir = if path.is_file() {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

/// Append a line to the repo's .gitignore (creating it if needed).
pub fn append_to_gitignore(repo_path: &Path, line: &str) -> Result<(), git2::Error> {
    let gitignore = repo_path.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore).unwrap_or_default();
    // Don't duplicate
    if !existing.lines().any(|l| l.trim() == line.trim()) {
        let separator = if existing.is_empty() || existing.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::fs::write(&gitignore, format!("{}{}{}\n", existing, separator, line))
            .map_err(|e| git2::Error::from_str(&e.to_string()))?;
    }
    Ok(())
}

/// Unstage a file (blocking — run on background executor)
pub fn unstage_file(repo_path: &Path, file_path: &str) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let git_path = file_path.replace('\\', "/");
    match repo.head() {
        Ok(head) => {
            // Reset the index entry to match HEAD, which removes it from staging
            let head_commit = head.peel_to_commit()?;
            repo.reset_default(
                Some(head_commit.as_object()),
                std::iter::once(git_path.as_str()),
            )?;
        }
        Err(_) => {
            // No HEAD (initial repo) — remove from index entirely
            let mut index = repo.index()?;
            index.remove_path(Path::new(&git_path))?;
            index.write()?;
        }
    }
    Ok(())
}

/// Unstage all staged files (blocking — run on background executor)
pub fn unstage_all_files(repo_path: &Path) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    match repo.head() {
        Ok(head) => {
            let head_commit = head.peel_to_commit()?;
            repo.reset_default(Some(head_commit.as_object()), std::iter::empty::<&str>())?;
            // reset_default with empty paths resets everything staged
            // Alternatively use reset --mixed HEAD which is what we want:
            repo.reset(head_commit.as_object(), git2::ResetType::Mixed, None)?;
        }
        Err(_) => {
            // No HEAD — clear the entire index
            let mut index = repo.index()?;
            index.clear()?;
            index.write()?;
        }
    }
    Ok(())
}

/// Commit staged changes (blocking — run on background executor)
pub fn commit_staged_changes(repo_path: &Path, message: &str) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let mut index = repo.index()?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;

    let signature = repo.signature()?;
    let parent_commit = repo.head()?.peel_to_commit()?;

    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &[&parent_commit],
    )?;

    Ok(())
}

/// Resolve the remote to use: branch tracking remote → "origin" → first available
fn find_remote_name(repo: &Repository) -> Result<String, git2::Error> {
    // Try the upstream remote for the current branch via config
    if let Ok(head) = repo.head() {
        if let Some(branch_name) = head.shorthand() {
            let key = format!("branch.{}.remote", branch_name);
            if let Ok(remote) = repo.config().and_then(|c| c.get_string(&key)) {
                if !remote.is_empty() {
                    return Ok(remote);
                }
            }
        }
    }
    // Fall back to "origin" if it exists
    if repo.find_remote("origin").is_ok() {
        return Ok("origin".to_string());
    }
    // Last resort: first remote in the list
    let remotes = repo.remotes()?;
    remotes
        .get(0)
        .map(|n| n.to_string())
        .ok_or_else(|| git2::Error::from_str("No remotes configured"))
}

/// Returns true if the git2 error indicates an authentication failure.
pub fn is_auth_error(e: &git2::Error) -> bool {
    let msg = e.message().to_lowercase();
    msg.contains("authentication")
        || msg.contains("401")
        || msg.contains("credentials")
        || e.class() == git2::ErrorClass::Http
}

/// Derive a stable keyring service name from the remote URL (host only, no path).
/// Returns the remote URL for the repo — used as both the keyring service key and lookup key.
fn remote_url_for_keyring(repo_path: &Path) -> Option<String> {
    let repo = Repository::open(repo_path).ok()?;
    let remote_name = find_remote_name(&repo).ok()?;
    repo.find_remote(&remote_name)
        .ok()?
        .url()
        .map(|u| u.to_string())
}

/// Save credentials to the OS keychain, keyed by the remote URL.
/// The secret value encodes both username and password as "username\npassword".
pub fn store_git_credentials(repo_path: &Path, username: &str, password: &str) {
    if let Some(url) = remote_url_for_keyring(repo_path) {
        if let Ok(entry) = keyring::Entry::new("pulsar-git", &url) {
            let _ = entry.set_password(&format!("{}\n{}", username, password));
        }
    }
}

/// Load credentials from the OS keychain, keyed by the remote URL.
/// Returns `(username, password)` or `None`.
pub fn load_git_credentials(repo_path: &Path) -> Option<(String, String)> {
    let url = remote_url_for_keyring(repo_path)?;
    let entry = keyring::Entry::new("pulsar-git", &url).ok()?;
    let secret = entry.get_password().ok()?;
    let mut parts = secret.splitn(2, '\n');
    let username = parts.next()?.to_string();
    let password = parts.next()?.to_string();
    Some((username, password))
}

/// Build remote callbacks that try SSH-agent → credential_helper → explicit creds.
fn make_callbacks(creds: Option<(String, String)>) -> git2::RemoteCallbacks<'static> {
    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks.credentials(move |url, username, allowed_types| {
        // Explicit credentials take priority (retry after auth failure)
        if let Some((ref user, ref pass)) = creds {
            return git2::Cred::userpass_plaintext(user, pass);
        }
        // SSH key from agent
        if allowed_types.contains(git2::CredentialType::SSH_KEY) {
            if let Ok(c) = git2::Cred::ssh_key_from_agent(username.unwrap_or("git")) {
                return Ok(c);
            }
        }
        // System credential helper (git credential store / keychain)
        if allowed_types.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
            if let Ok(cfg) = git2::Config::open_default() {
                if let Ok(c) = git2::Cred::credential_helper(&cfg, url, username) {
                    return Ok(c);
                }
            }
        }
        Err(git2::Error::from_str("No credentials available"))
    });
    callbacks
}

/// Fetch from remote without merging (blocking — run on background executor).
/// Pass `creds` to retry after an auth failure.
pub fn fetch_from_remote(
    repo_path: &Path,
    creds: Option<(String, String)>,
) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let remote_name = find_remote_name(&repo)?;
    let mut remote = repo.find_remote(&remote_name)?;
    let refspec = format!("+refs/heads/*:refs/remotes/{}/*", remote_name);
    let mut opts = git2::FetchOptions::new();
    opts.remote_callbacks(make_callbacks(creds));
    remote.fetch(&[refspec.as_str()], Some(&mut opts), None)?;
    Ok(())
}

/// Push to remote (blocking — run on background executor).
/// Pass `creds` to retry after an auth failure.
pub fn push_to_remote(
    repo_path: &Path,
    creds: Option<(String, String)>,
) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let remote_name = find_remote_name(&repo)?;
    let mut remote = repo.find_remote(&remote_name)?;
    let head = repo.head()?;
    let branch_name = head.shorthand().unwrap_or("HEAD");
    let mut opts = git2::PushOptions::new();
    opts.remote_callbacks(make_callbacks(creds));
    remote.push(
        &[format!(
            "refs/heads/{}:refs/heads/{}",
            branch_name, branch_name
        )],
        Some(&mut opts),
    )?;
    Ok(())
}

/// Pull from remote (blocking — run on background executor).
/// Pass `creds` to retry after an auth failure.
pub fn pull_from_remote(
    repo_path: &Path,
    creds: Option<(String, String)>,
) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let remote_name = find_remote_name(&repo)?;
    let mut remote = repo.find_remote(&remote_name)?;
    let mut opts = git2::FetchOptions::new();
    opts.remote_callbacks(make_callbacks(creds));
    remote.fetch(&["HEAD"], Some(&mut opts), None)?;

    let fetch_head = repo.find_reference("FETCH_HEAD")?;
    let fetch_commit = repo.reference_to_annotated_commit(&fetch_head)?;
    let analysis = repo.merge_analysis(&[&fetch_commit])?;

    if analysis.0.is_up_to_date() {
        Ok(())
    } else if analysis.0.is_fast_forward() {
        let refname = format!("refs/heads/{}", repo.head()?.shorthand().unwrap_or("HEAD"));
        let mut reference = repo.find_reference(&refname)?;
        reference.set_target(fetch_commit.id(), "Fast-forward")?;
        repo.set_head(&refname)?;
        repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))?;
        Ok(())
    } else {
        Err(git2::Error::from_str("Merge required"))
    }
}

/// Switch to a different branch, carrying uncommitted changes via auto-stash (blocking — run on background executor)
pub fn switch_branch(repo_path: &Path, branch_name: &str) -> Result<(), git2::Error> {
    let mut repo = Repository::open(repo_path)?;

    // Auto-stash any dirty working tree so we can carry changes across branches
    let has_changes = {
        let mut opts = StatusOptions::new();
        opts.include_untracked(true);
        repo.statuses(Some(&mut opts))?.iter().any(|s| {
            s.status().intersects(
                git2::Status::INDEX_NEW
                    | git2::Status::INDEX_MODIFIED
                    | git2::Status::INDEX_DELETED
                    | git2::Status::WT_MODIFIED
                    | git2::Status::WT_DELETED
                    | git2::Status::WT_NEW,
            )
        })
    };

    let stashed = if has_changes {
        let sig = repo
            .signature()
            .or_else(|_| git2::Signature::now("Pulsar", "pulsar@local"))?;
        match repo.stash_save(
            &sig,
            "pulsar: auto-stash before branch switch",
            Some(git2::StashFlags::INCLUDE_UNTRACKED),
        ) {
            Ok(_) => true,
            Err(_) => false,
        }
    } else {
        false
    };

    // Resolve branch ref — handles both local names and remote tracking names.
    // We drop object + reference BEFORE stash_pop to release the immutable borrow on repo.
    {
        let (object, reference) = repo.revparse_ext(branch_name)?;
        let mut checkout = git2::build::CheckoutBuilder::default();
        checkout.safe();
        repo.checkout_tree(&object, Some(&mut checkout))?;
        match reference {
            Some(gref) => repo.set_head(gref.name().unwrap_or(branch_name))?,
            None => repo.set_head_detached(object.id())?,
        }
    } // object + reference dropped here — immutable borrow released

    // Re-apply stashed changes on the new branch
    if stashed {
        let mut stash_opts = git2::StashApplyOptions::new();
        stash_opts.reinstantiate_index();
        // Best-effort — if it conflicts the user can resolve manually
        let _ = repo.stash_pop(0, Some(&mut stash_opts));
    }

    Ok(())
}

/// Load the content of a file for display (blocking — run on background executor).
/// Returns `None` if the file is binary, or the line count exceeds `limit`.
pub fn load_file_content(
    repo_path: &Path,
    file_path: &str,
    line_limit: usize,
) -> FileContentResult {
    let full_path = repo_path.join(file_path);
    let bytes = match std::fs::read(&full_path) {
        Ok(b) => b,
        Err(e) => return FileContentResult::Error(e.to_string()),
    };
    // Heuristic: if any null bytes appear in the first 8 KiB, treat as binary
    let probe = &bytes[..bytes.len().min(8192)];
    if probe.contains(&0u8) {
        return FileContentResult::Binary;
    }
    match String::from_utf8(bytes) {
        Err(_) => FileContentResult::Binary,
        Ok(text) => {
            let line_count = text.lines().count();
            if line_count > line_limit {
                FileContentResult::TooLong(line_count)
            } else {
                FileContentResult::Text(text)
            }
        }
    }
}

/// Result of attempting to load a file for display
#[derive(Debug, Clone)]
pub enum FileContentResult {
    Text(String),
    Binary,
    TooLong(usize),
    Error(String),
}

/// Kind of diff line
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Added,
    Removed,
    Context,
}

/// A single line in a diff view
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
    /// Line number in the new file (None for removed lines)
    pub new_line_num: Option<usize>,
    /// Line number in the old file (None for added lines)
    pub old_line_num: Option<usize>,
}

/// A segment in a diff view — either visible lines or a collapsible unchanged region
#[derive(Debug, Clone)]
pub enum DiffSegment {
    /// Lines that should always be shown (changed + their context)
    Hunk(Vec<DiffLine>),
    /// Unchanged lines that are collapsed by default; user can expand them
    Collapsed {
        lines: Vec<DiffLine>,
        region_idx: usize,
    },
}

/// Full diff result — ready for direct rendering
#[derive(Debug, Clone)]
pub struct DiffResult {
    pub segments: Vec<DiffSegment>,
}

/// Get the list of files changed in a specific commit (blocking — run on background executor)
pub fn get_commit_files(
    repo_path: &Path,
    commit_hash: &str,
) -> Result<Vec<FileChange>, git2::Error> {
    let repo = Repository::open(repo_path)?;
    let oid = git2::Oid::from_str(commit_hash)?;
    let commit = repo.find_commit(oid)?;

    let tree = commit.tree()?;
    let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());

    let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;

    let mut files = Vec::new();
    for delta in diff.deltas() {
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .and_then(|p| p.to_str())
            .unwrap_or("")
            .to_string();

        let status = match delta.status() {
            git2::Delta::Added => ChangeStatus::Added,
            git2::Delta::Deleted => ChangeStatus::Deleted,
            git2::Delta::Renamed => ChangeStatus::Renamed,
            git2::Delta::Copied => ChangeStatus::Copied,
            _ => ChangeStatus::Modified,
        };

        files.push(FileChange {
            path,
            status,
            additions: 0,
            deletions: 0,
        });
    }
    Ok(files)
}

/// Load file content at a specific commit (blocking — run on background executor)
pub fn load_file_at_commit(
    repo_path: &Path,
    commit_hash: &str,
    file_path: &str,
    line_limit: usize,
) -> FileContentResult {
    let repo = match Repository::open(repo_path) {
        Ok(r) => r,
        Err(e) => return FileContentResult::Error(e.to_string()),
    };
    let oid = match git2::Oid::from_str(commit_hash) {
        Ok(o) => o,
        Err(e) => return FileContentResult::Error(e.to_string()),
    };
    let commit = match repo.find_commit(oid) {
        Ok(c) => c,
        Err(e) => return FileContentResult::Error(e.to_string()),
    };
    let tree = match commit.tree() {
        Ok(t) => t,
        Err(e) => return FileContentResult::Error(e.to_string()),
    };
    let entry = match tree.get_path(Path::new(file_path)) {
        Ok(e) => e,
        Err(e) => return FileContentResult::Error(format!("File not found in commit: {}", e)),
    };
    let blob = match repo.find_blob(entry.id()) {
        Ok(b) => b,
        Err(e) => return FileContentResult::Error(e.to_string()),
    };
    let bytes = blob.content();
    let probe = &bytes[..bytes.len().min(8192)];
    if probe.contains(&0u8) {
        return FileContentResult::Binary;
    }
    match String::from_utf8(bytes.to_vec()) {
        Err(_) => FileContentResult::Binary,
        Ok(text) => {
            let line_count = text.lines().count();
            if line_count > line_limit {
                FileContentResult::TooLong(line_count)
            } else {
                FileContentResult::Text(text)
            }
        }
    }
}

// ── Diff helpers ─────────────────────────────────────────────────────────────

const CONTEXT_LINES: usize = 3;

/// Run a Myers line-level diff and produce collapsible segments (Monaco/GitHub style).
fn diff_lines(old_text: &str, new_text: &str) -> DiffResult {
    use similar::{ChangeTag, TextDiff};

    let diff = TextDiff::from_lines(old_text, new_text);
    let mut all_lines: Vec<DiffLine> = Vec::new();

    for change in diff.iter_all_changes() {
        let content = change.value().trim_end_matches('\n').to_string();
        let (kind, new_num, old_num) = match change.tag() {
            ChangeTag::Insert => (DiffLineKind::Added, change.new_index().map(|i| i + 1), None),
            ChangeTag::Delete => (
                DiffLineKind::Removed,
                None,
                change.old_index().map(|i| i + 1),
            ),
            ChangeTag::Equal => (
                DiffLineKind::Context,
                change.new_index().map(|i| i + 1),
                change.old_index().map(|i| i + 1),
            ),
        };
        all_lines.push(DiffLine {
            kind,
            content,
            new_line_num: new_num,
            old_line_num: old_num,
        });
    }

    let n = all_lines.len();
    let has_changes = all_lines.iter().any(|l| l.kind != DiffLineKind::Context);

    // No changes — return whole file as a single hunk (no collapse bars)
    if !has_changes || n == 0 {
        return DiffResult {
            segments: vec![DiffSegment::Hunk(all_lines)],
        };
    }

    // Mark lines within CONTEXT_LINES of any change as visible
    let mut visible = vec![false; n];
    for i in 0..n {
        if all_lines[i].kind != DiffLineKind::Context {
            let lo = i.saturating_sub(CONTEXT_LINES);
            let hi = (i + CONTEXT_LINES + 1).min(n);
            for j in lo..hi {
                visible[j] = true;
            }
        }
    }

    // Build segments
    let mut segments: Vec<DiffSegment> = Vec::new();
    let mut region_idx = 0usize;
    let mut i = 0;
    while i < n {
        if visible[i] {
            let start = i;
            while i < n && visible[i] {
                i += 1;
            }
            segments.push(DiffSegment::Hunk(all_lines[start..i].to_vec()));
        } else {
            let start = i;
            while i < n && !visible[i] {
                i += 1;
            }
            segments.push(DiffSegment::Collapsed {
                lines: all_lines[start..i].to_vec(),
                region_idx,
            });
            region_idx += 1;
        }
    }

    DiffResult { segments }
}

/// Load old blob content for a file from HEAD (empty string for new files).
fn load_blob_from_head(repo: &Repository, file_path: &str) -> String {
    let normalized = file_path.replace('\\', "/");
    repo.head()
        .ok()
        .and_then(|h| h.peel_to_tree().ok())
        .and_then(|t| t.get_path(Path::new(&normalized)).ok())
        .and_then(|e| repo.find_blob(e.id()).ok())
        .and_then(|b| String::from_utf8(b.content().to_vec()).ok())
        .unwrap_or_default()
}

/// Load old blob content from a commit's parent (empty string for added files).
fn load_blob_from_parent(repo: &Repository, commit: &git2::Commit, file_path: &str) -> String {
    let normalized = file_path.replace('\\', "/");
    commit
        .parent(0)
        .ok()
        .and_then(|p| p.tree().ok())
        .and_then(|t| t.get_path(Path::new(&normalized)).ok())
        .and_then(|e| repo.find_blob(e.id()).ok())
        .and_then(|b| String::from_utf8(b.content().to_vec()).ok())
        .unwrap_or_default()
}

/// Compute the working-tree diff for a single file vs HEAD (blocking).
pub fn load_file_diff_working(repo_path: &Path, file_path: &str) -> Result<DiffResult, String> {
    let repo = Repository::open(repo_path).map_err(|e| e.message().to_string())?;
    let new_text = std::fs::read_to_string(repo_path.join(file_path)).map_err(|e| e.to_string())?;
    if new_text.contains('\0') {
        return Err("Binary file".to_string());
    }
    let old_text = load_blob_from_head(&repo, file_path);
    Ok(diff_lines(&old_text, &new_text))
}

/// Compute the diff for a single file in a commit vs its parent (blocking).
pub fn load_file_diff_at_commit(
    repo_path: &Path,
    commit_hash: &str,
    file_path: &str,
) -> Result<DiffResult, String> {
    let repo = Repository::open(repo_path).map_err(|e| e.message().to_string())?;
    let oid = git2::Oid::from_str(commit_hash).map_err(|e| e.message().to_string())?;
    let commit = repo.find_commit(oid).map_err(|e| e.message().to_string())?;
    let normalized = file_path.replace('\\', "/");
    let new_text = commit
        .tree()
        .ok()
        .and_then(|t| t.get_path(Path::new(&normalized)).ok())
        .and_then(|e| repo.find_blob(e.id()).ok())
        .and_then(|b| String::from_utf8(b.content().to_vec()).ok())
        .ok_or_else(|| "File not found or binary in commit".to_string())?;
    let old_text = load_blob_from_parent(&repo, &commit, file_path);
    Ok(diff_lines(&old_text, &new_text))
}
