//! Git operations using git2

use crate::{auto_fetch::RemoteOperationGuard, models::*};
use engine_fs::{FsProvider as _, LocalFsProvider};
use git2::{BranchType, ErrorCode, Oid, Repository, StatusOptions, build::CheckoutBuilder};
use std::path::{Path, PathBuf};

pub(crate) fn open_repo(path: &Path) -> Result<Repository, git2::Error> {
    if path.as_os_str().is_empty() {
        return Err(git2::Error::from_str("repository path is empty"));
    }

    Repository::discover(path).or_else(|_| Repository::open(path))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteOperationKind {
    Fetch,
    Push,
    Pull,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteOperationContext {
    repository_path: PathBuf,
    head_reference: Option<String>,
    head_oid: Option<Oid>,
    branch_merge: Option<String>,
    remote_name: String,
    remote_url: String,
    fetch_refspecs: Option<Vec<Vec<u8>>>,
}

pub(crate) struct RemoteOperationAttempt {
    pub(crate) result: Result<(), git2::Error>,
    pub(crate) context: Option<RemoteOperationContext>,
}

/// Load the complete repository state (blocking — run on background executor)
pub fn load_repository_state(path: &Path) -> Result<RepositoryState, git2::Error> {
    let repo = open_repo(path)?;

    let current_branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().ok().map(|s| s.to_string()))
        .unwrap_or_else(|| "HEAD".to_string());

    let branches = load_branches(&repo).unwrap_or_default();
    let commits = load_commits(&repo, 100).unwrap_or_default();
    let (staged_files, unstaged_files, untracked_files) = load_file_changes(&repo)?;
    let (ahead, behind) = get_ahead_behind(&repo).unwrap_or((0, 0));

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
    let branch = repo.find_branch(head.shorthand().ok().unwrap_or(""), BranchType::Local)?;
    let upstream = match branch.upstream() {
        Ok(upstream) => upstream,
        Err(_) => return Ok((0, 0)),
    };
    let upstream_oid = upstream
        .get()
        .target()
        .ok_or_else(|| git2::Error::from_str("No upstream target"))?;

    let (ahead, behind) = repo.graph_ahead_behind(local_oid, upstream_oid)?;
    Ok((ahead, behind))
}

/// Stage a file (blocking — run on background executor)
pub fn stage_file(repo_path: &Path, file_path: &str) -> Result<(), git2::Error> {
    let repo = open_repo(repo_path)?;
    let mut index = repo.index()?;
    let git_path = file_path.replace('\\', "/");
    index.add_path(Path::new(&git_path))?;
    index.write()?;
    Ok(())
}

/// Stage all modified/untracked files (blocking — run on background executor)
pub fn stage_all_files(repo_path: &Path) -> Result<(), git2::Error> {
    let repo = open_repo(repo_path)?;
    let mut index = repo.index()?;
    index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
    index.write()?;
    Ok(())
}

/// Discard working-tree changes for a file (blocking — run on background executor).
/// For untracked files, deletes the file. For modified/deleted, checks out HEAD version.
pub fn discard_file_changes(repo_path: &Path, file_path: &str) -> Result<(), git2::Error> {
    let repo = open_repo(repo_path)?;
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
        std::fs::remove_file(&full_path);
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
    let repo = open_repo(repo_path)?;
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
    let repo = open_repo(repo_path)?;
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
    let _guard = RemoteOperationGuard::acquire(repo_path)?;
    let repo = open_repo(repo_path)?;
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
        if let Ok(branch_name) = head.shorthand() {
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
        .map(|name| name.map(|name| name.to_string()))?
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

fn load_git_credentials(remote_url: &str) -> Option<(String, String)> {
    let entry = keyring::Entry::new("pulsar-git", remote_url).ok()?;
    let secret = entry.get_password().ok()?;
    let mut parts = secret.splitn(2, '\n');
    let username = parts.next()?.to_string();
    let password = parts.next()?.to_string();
    Some((username, password))
}

fn store_git_credentials(remote_url: &str, username: &str, password: &str) {
    let Ok(entry) = keyring::Entry::new("pulsar-git", remote_url) else {
        return;
    };

    if let Err(error) = entry.set_password(&format!("{username}\n{password}")) {
        tracing::warn!(%error, "Failed to store Git credentials");
    }
}

fn remote_operation_url(
    remote: &git2::Remote<'_>,
    use_push_url: bool,
) -> Result<String, git2::Error> {
    let url = if use_push_url {
        match remote.pushurl()? {
            Some(url) => url,
            None => remote.url()?,
        }
    } else {
        remote.url()?
    };

    Ok(url.to_owned())
}

fn remote_operation_context(
    repo_path: &Path,
    repo: &Repository,
    kind: RemoteOperationKind,
) -> Result<RemoteOperationContext, git2::Error> {
    let (head_reference, head_oid, branch_name) = match repo.head() {
        Ok(head) => {
            let branch_name = if head.is_branch() {
                Some(head.shorthand()?.to_owned())
            } else {
                None
            };
            (Some(head.name()?.to_owned()), head.target(), branch_name)
        }
        Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::UnbornBranch) => {
            (None, None, None)
        }
        Err(error) => return Err(error),
    };
    let branch_merge = branch_name.as_deref().and_then(|branch_name| {
        repo.config()
            .and_then(|config| config.get_string(&format!("branch.{branch_name}.merge")))
            .ok()
    });
    let remote_name = find_remote_name(repo)?;
    let remote = repo.find_remote(&remote_name)?;
    let remote_url = remote_operation_url(&remote, kind == RemoteOperationKind::Push)?;
    let fetch_refspecs = if matches!(kind, RemoteOperationKind::Fetch | RemoteOperationKind::Pull) {
        Some(
            remote
                .fetch_refspecs()?
                .iter_bytes()
                .map(ToOwned::to_owned)
                .collect(),
        )
    } else {
        None
    };
    let common_dir = repo.commondir();
    let repository_path = LocalFsProvider::new()
        .canonicalize(common_dir)
        .map_err(|error| {
            git2::Error::from_str(&format!(
                "failed to canonicalize repository path {} for {}: {error}",
                common_dir.display(),
                repo_path.display()
            ))
        })?;

    Ok(RemoteOperationContext {
        repository_path,
        head_reference,
        head_oid,
        branch_merge,
        remote_name,
        remote_url,
        fetch_refspecs,
    })
}

fn ensure_remote_operation_context(
    expected: &RemoteOperationContext,
    current: &RemoteOperationContext,
) -> Result<(), git2::Error> {
    if expected == current {
        Ok(())
    } else {
        Err(git2::Error::from_str(
            "Git operation target changed; start the operation again",
        ))
    }
}

fn context_remote<'repo>(
    repo: &'repo Repository,
    context: &RemoteOperationContext,
    use_push_url: bool,
) -> Result<git2::Remote<'repo>, git2::Error> {
    let remote = repo.find_remote(&context.remote_name)?;
    let current_url = remote_operation_url(&remote, use_push_url)?;
    if current_url != context.remote_url {
        return Err(git2::Error::from_str(
            "Git operation target changed; start the operation again",
        ));
    }

    Ok(remote)
}

fn same_credential_scope(configured_url: &str, requested_url: &str) -> bool {
    if configured_url == requested_url {
        return true;
    }

    let Ok(configured) = reqwest::Url::parse(configured_url) else {
        return false;
    };
    let Ok(requested) = reqwest::Url::parse(requested_url) else {
        return false;
    };

    configured.scheme().eq_ignore_ascii_case(requested.scheme())
        && configured
            .host_str()
            .zip(requested.host_str())
            .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
        && configured.port_or_known_default() == requested.port_or_known_default()
}

fn load_stored_credentials(remote_url: &str) -> Option<(String, String)> {
    let parsed = reqwest::Url::parse(remote_url).ok()?;
    parsed.host_str()?;
    load_git_credentials(remote_url)
}

/// Build remote callbacks using credentials scoped to the selected remote URL.
fn make_callbacks(
    creds: Option<(String, String)>,
    credential_url: String,
) -> git2::RemoteCallbacks<'static> {
    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks.credentials(move |url, username, allowed_types| {
        if let Some((ref user, ref pass)) = creds {
            if same_credential_scope(&credential_url, url)
                && allowed_types.contains(git2::CredentialType::USER_PASS_PLAINTEXT)
            {
                return git2::Cred::userpass_plaintext(user, pass);
            }
        }

        if allowed_types.contains(git2::CredentialType::SSH_KEY) {
            if let Ok(credential) = git2::Cred::ssh_key_from_agent(username.unwrap_or("git")) {
                return Ok(credential);
            }
        }

        if allowed_types.contains(git2::CredentialType::DEFAULT) {
            if let Ok(credential) = git2::Cred::default() {
                return Ok(credential);
            }
        }

        Err(git2::Error::from_str("No credentials available"))
    });
    callbacks
}

fn fetch_remote(
    repo: &Repository,
    context: &RemoteOperationContext,
    explicit_creds: Option<(String, String)>,
) -> Result<(), git2::Error> {
    let mut remote = context_remote(repo, context, false)?;
    let credentials = explicit_creds
        .clone()
        .or_else(|| load_stored_credentials(&context.remote_url));
    let mut opts = git2::FetchOptions::new();
    opts.remote_callbacks(make_callbacks(credentials, context.remote_url.clone()));
    remote.fetch(&[] as &[&str], Some(&mut opts), None)?;

    if let Some((username, password)) = explicit_creds {
        store_git_credentials(&context.remote_url, &username, &password);
    }

    Ok(())
}

fn push_remote(
    repo: &Repository,
    context: &RemoteOperationContext,
    explicit_creds: Option<(String, String)>,
) -> Result<(), git2::Error> {
    let refname = context
        .head_reference
        .as_deref()
        .ok_or_else(|| git2::Error::from_str("cannot push an unborn branch"))?;
    let branch_name = refname
        .strip_prefix("refs/heads/")
        .ok_or_else(|| git2::Error::from_str("cannot push a detached HEAD"))?;
    let mut remote = context_remote(repo, context, true)?;
    let credentials = explicit_creds
        .clone()
        .or_else(|| load_stored_credentials(&context.remote_url));
    let mut opts = git2::PushOptions::new();
    opts.remote_callbacks(make_callbacks(credentials, context.remote_url.clone()));
    remote.push(
        &[format!("{refname}:refs/heads/{branch_name}")],
        Some(&mut opts),
    )?;

    if let Some((username, password)) = explicit_creds {
        store_git_credentials(&context.remote_url, &username, &password);
    }

    Ok(())
}

fn rollback_pull_worktree(
    repo: &Repository,
    local_oid: Oid,
    checked_out_oid: Oid,
    refname: &str,
    update_error: git2::Error,
) -> git2::Error {
    let rollback_result = (|| {
        let original = repo.find_commit(local_oid)?;
        let checked_out = repo.find_commit(checked_out_oid)?;
        let original_tree = original.tree()?;
        let checked_out_tree = checked_out.tree()?;
        let diff = repo.diff_tree_to_tree(Some(&original_tree), Some(&checked_out_tree), None)?;
        if diff.deltas().len() == 0 {
            return Ok(());
        }
        let mut rollback = CheckoutBuilder::new();
        rollback.force().disable_pathspec_match(true);
        for delta in diff.deltas() {
            if let Some(path) = delta.old_file().path() {
                rollback.path(path);
            }
            if let Some(path) = delta.new_file().path() {
                rollback.path(path);
            }
        }
        repo.checkout_tree(original.as_object(), Some(&mut rollback))
    })();

    match rollback_result {
        Ok(()) => update_error,
        Err(rollback_error) => git2::Error::from_str(&format!(
            "fast-forward reference update failed ({update_error}); restoring the working tree for {refname} also failed: {rollback_error}"
        )),
    }
}

fn finish_pull_transaction(
    repo: &Repository,
    local_oid: Oid,
    upstream_oid: Oid,
    refname: &str,
    commit_result: Result<(), git2::Error>,
) -> Result<(), git2::Error> {
    let Err(commit_error) = commit_result else {
        return Ok(());
    };

    match repo.refname_to_id(refname) {
        Ok(current_oid) if current_oid == local_oid => Err(rollback_pull_worktree(
            repo,
            local_oid,
            upstream_oid,
            refname,
            commit_error,
        )),
        Ok(current_oid) if current_oid == upstream_oid => Err(git2::Error::from_str(&format!(
            "fast-forward transaction reported an error after updating {refname} to {upstream_oid}; the reference and working tree remain at the updated commit: {commit_error}"
        ))),
        Ok(current_oid) => Err(git2::Error::from_str(&format!(
            "fast-forward transaction failed ({commit_error}), but {refname} now points to unexpected commit {current_oid}; the working tree was not rolled back"
        ))),
        Err(inspect_error) => Err(git2::Error::from_str(&format!(
            "fast-forward transaction failed ({commit_error}), and the state of {refname} could not be determined: {inspect_error}"
        ))),
    }
}

fn pull_remote<F>(
    repo_path: &Path,
    repo: &Repository,
    context: &RemoteOperationContext,
    explicit_creds: Option<(String, String)>,
    after_fetch: F,
) -> Result<(), git2::Error>
where
    F: FnOnce(&Repository) -> Result<(), git2::Error>,
{
    let refname = context
        .head_reference
        .as_deref()
        .and_then(|name| name.strip_prefix("refs/heads/").map(|_| name))
        .ok_or_else(|| git2::Error::from_str("cannot pull into a detached or unborn HEAD"))?;
    let branch_name = refname
        .strip_prefix("refs/heads/")
        .expect("branch reference prefix was checked");
    let local_oid = context
        .head_oid
        .ok_or_else(|| git2::Error::from_str("current branch has no target"))?;

    fetch_remote(repo, context, explicit_creds)?;
    after_fetch(repo)?;

    let mut transaction = repo.transaction()?;
    transaction.lock_ref("HEAD")?;
    transaction.lock_ref(refname)?;

    let upstream = repo
        .find_branch(branch_name, BranchType::Local)?
        .upstream()?;
    let upstream_oid = upstream
        .get()
        .target()
        .ok_or_else(|| git2::Error::from_str("upstream branch has no target"))?;
    let current_context = remote_operation_context(repo_path, repo, RemoteOperationKind::Pull)?;
    ensure_remote_operation_context(context, &current_context)?;
    let (ahead, behind) = repo.graph_ahead_behind(local_oid, upstream_oid)?;

    if behind == 0 {
        return Ok(());
    }
    if ahead > 0 {
        return Err(git2::Error::from_str(
            "Merge required; pull only supports fast-forward updates",
        ));
    }

    let target = repo.find_commit(upstream_oid)?;
    let mut preflight = CheckoutBuilder::new();
    preflight.dry_run();
    repo.checkout_tree(target.as_object(), Some(&mut preflight))?;

    let mut checkout = CheckoutBuilder::new();
    checkout.safe();
    repo.checkout_tree(target.as_object(), Some(&mut checkout))?;

    if let Err(update_error) =
        transaction.set_target(refname, upstream_oid, None, "pull: fast-forward")
    {
        drop(transaction);
        return Err(rollback_pull_worktree(
            repo,
            local_oid,
            upstream_oid,
            refname,
            update_error,
        ));
    }
    let commit_result = transaction.commit();
    finish_pull_transaction(repo, local_oid, upstream_oid, refname, commit_result)
}

fn remote_operation_attempt_error(error: git2::Error) -> RemoteOperationAttempt {
    RemoteOperationAttempt {
        result: Err(error),
        context: None,
    }
}

fn execute_remote_operation_unlocked(
    repo_path: &Path,
    kind: RemoteOperationKind,
    explicit_creds: Option<(String, String)>,
    expected_context: Option<RemoteOperationContext>,
) -> RemoteOperationAttempt {
    let repo = match open_repo(repo_path) {
        Ok(repo) => repo,
        Err(error) => return remote_operation_attempt_error(error),
    };
    let context = match remote_operation_context(repo_path, &repo, kind) {
        Ok(context) => context,
        Err(error) => return remote_operation_attempt_error(error),
    };
    if let Some(expected_context) = expected_context.as_ref() {
        if let Err(error) = ensure_remote_operation_context(expected_context, &context) {
            return RemoteOperationAttempt {
                result: Err(error),
                context: Some(context),
            };
        }
    }

    let result = match kind {
        RemoteOperationKind::Fetch => fetch_remote(&repo, &context, explicit_creds),
        RemoteOperationKind::Push => push_remote(&repo, &context, explicit_creds),
        RemoteOperationKind::Pull => {
            pull_remote(repo_path, &repo, &context, explicit_creds, |_| Ok(()))
        }
    };

    RemoteOperationAttempt {
        result,
        context: Some(context),
    }
}

pub(crate) fn execute_remote_operation(
    repo_path: &Path,
    kind: RemoteOperationKind,
    explicit_creds: Option<(String, String)>,
    expected_context: Option<RemoteOperationContext>,
) -> RemoteOperationAttempt {
    let _guard = match RemoteOperationGuard::acquire(repo_path) {
        Ok(guard) => guard,
        Err(error) => return remote_operation_attempt_error(error),
    };
    execute_remote_operation_unlocked(repo_path, kind, explicit_creds, expected_context)
}

/// Fetch from remote without merging (blocking — run on background executor).
/// Pass `creds` to retry after an auth failure.
pub fn fetch_from_remote(
    repo_path: &Path,
    explicit_creds: Option<(String, String)>,
) -> Result<(), git2::Error> {
    execute_remote_operation(repo_path, RemoteOperationKind::Fetch, explicit_creds, None).result
}

pub(crate) fn fetch_from_remote_unlocked(
    repo_path: &Path,
    explicit_creds: Option<(String, String)>,
) -> Result<(), git2::Error> {
    execute_remote_operation_unlocked(repo_path, RemoteOperationKind::Fetch, explicit_creds, None)
        .result
}

/// Push to remote (blocking — run on background executor).
/// Pass `creds` to retry after an auth failure.
pub fn push_to_remote(
    repo_path: &Path,
    explicit_creds: Option<(String, String)>,
) -> Result<(), git2::Error> {
    execute_remote_operation(repo_path, RemoteOperationKind::Push, explicit_creds, None).result
}

/// Pull from remote (blocking — run on background executor).
/// Pass `creds` to retry after an auth failure.
pub fn pull_from_remote(
    repo_path: &Path,
    explicit_creds: Option<(String, String)>,
) -> Result<(), git2::Error> {
    execute_remote_operation(repo_path, RemoteOperationKind::Pull, explicit_creds, None).result
}

#[cfg(test)]
pub(crate) fn pull_from_remote_with_after_fetch<F>(
    repo_path: &Path,
    explicit_creds: Option<(String, String)>,
    after_fetch: F,
) -> Result<(), git2::Error>
where
    F: FnOnce(&Repository) -> Result<(), git2::Error>,
{
    let _guard = RemoteOperationGuard::acquire(repo_path)?;
    let repo = open_repo(repo_path)?;
    let context = remote_operation_context(repo_path, &repo, RemoteOperationKind::Pull)?;
    pull_remote(repo_path, &repo, &context, explicit_creds, after_fetch)
}

/// Switch to a different branch, carrying uncommitted changes via auto-stash (blocking — run on background executor)
pub fn switch_branch(repo_path: &Path, branch_name: &str) -> Result<(), git2::Error> {
    let _guard = RemoteOperationGuard::acquire(repo_path)?;
    let mut repo = open_repo(repo_path)?;

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
    /// Full old-text lines indexed by line number (1-based: lines[0] = line 1).
    pub old_lines: Vec<String>,
    /// Full new-text lines indexed by line number (1-based: lines[0] = line 1).
    pub new_lines: Vec<String>,
}

/// A single flat row used by the virtualized diff viewer.
#[derive(Debug, Clone)]
pub enum DiffRow {
    /// A source code line (added / removed / context).
    Line {
        kind: DiffLineKind,
        content: String,
        /// Pre-formatted line number string, e.g. `"  42 "`.
        line_num_str: String,
    },
    /// A collapsed-region button row.
    Collapse { region_idx: usize, count: usize },
}

/// Flatten a `DiffResult` + expanded-region set into a flat `Vec<DiffRow>` that the
/// virtual diff list can render directly.
pub fn flatten_diff(
    diff: &DiffResult,
    expanded: &std::collections::HashSet<usize>,
) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for segment in &diff.segments {
        match segment {
            DiffSegment::Hunk(lines) => {
                for line in lines {
                    let line_num_str = line
                        .new_line_num
                        .or(line.old_line_num)
                        .map(|n| format!("{:>5}", n))
                        .unwrap_or_else(|| "     ".to_string());
                    rows.push(DiffRow::Line {
                        kind: line.kind,
                        content: line.content.clone(),
                        line_num_str,
                    });
                }
            }
            DiffSegment::Collapsed { lines, region_idx } => {
                if expanded.contains(region_idx) {
                    for line in lines {
                        let line_num_str = line
                            .new_line_num
                            .or(line.old_line_num)
                            .map(|n| format!("{:>5}", n))
                            .unwrap_or_else(|| "     ".to_string());
                        rows.push(DiffRow::Line {
                            kind: DiffLineKind::Context,
                            content: line.content.clone(),
                            line_num_str,
                        });
                    }
                } else {
                    rows.push(DiffRow::Collapse {
                        region_idx: *region_idx,
                        count: lines.len(),
                    });
                }
            }
        }
    }
    rows
}

/// Get the list of files changed in a specific commit (blocking — run on background executor)
pub fn get_commit_files(
    repo_path: &Path,
    commit_hash: &str,
) -> Result<Vec<FileChange>, git2::Error> {
    let repo = open_repo(repo_path)?;
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
    let repo = match open_repo(repo_path) {
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
    let old_lines: Vec<String> = old_text.lines().map(String::from).collect();
    let new_lines: Vec<String> = new_text.lines().map(String::from).collect();
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
            old_lines,
            new_lines,
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

    DiffResult {
        segments,
        old_lines,
        new_lines,
    }
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
    let repo = open_repo(repo_path).map_err(|e| e.message().to_string())?;
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
    let repo = open_repo(repo_path).map_err(|e| e.message().to_string())?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{RepositoryInitOptions, Signature};

    fn init_repository(path: &Path) -> Repository {
        let mut options = RepositoryInitOptions::new();
        options.initial_head("main");
        let repo = Repository::init_opts(path, &options).expect("initialize repository");
        std::fs::write(path.join("tracked.txt"), "initial\n").expect("write tracked file");
        let mut index = repo.index().expect("open index");
        index
            .add_path(Path::new("tracked.txt"))
            .expect("add tracked file");
        index.write().expect("write index");
        let tree_oid = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_oid).expect("find tree");
        let signature =
            Signature::now("Pulsar Test", "pulsar@example.com").expect("create signature");
        repo.commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
            .expect("create initial commit");
        drop(tree);
        repo
    }

    fn commit_file(repo: &Repository, contents: &str, message: &str) -> Oid {
        std::fs::write(
            repo.workdir()
                .expect("repository should have a worktree")
                .join("tracked.txt"),
            contents,
        )
        .expect("write tracked file");
        let mut index = repo.index().expect("open index");
        index
            .add_path(Path::new("tracked.txt"))
            .expect("add tracked file");
        index.write().expect("write index");
        let tree_oid = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_oid).expect("find tree");
        let parent = repo
            .head()
            .expect("load HEAD")
            .peel_to_commit()
            .expect("load HEAD commit");
        let signature =
            Signature::now("Pulsar Test", "pulsar@example.com").expect("create signature");
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &[&parent],
        )
        .expect("create commit")
    }

    fn prepare_pending_fast_forward(repo: &Repository) -> (Oid, Oid) {
        let local_oid = repo
            .head()
            .expect("load HEAD")
            .target()
            .expect("HEAD target");
        let upstream_oid = commit_file(repo, "upstream\n", "upstream");
        let local = repo
            .find_object(local_oid, None)
            .expect("find original commit");
        repo.reset(&local, git2::ResetType::Hard, None)
            .expect("restore original branch state");
        drop(local);

        let upstream = repo
            .find_commit(upstream_oid)
            .expect("find upstream commit");
        let mut checkout = CheckoutBuilder::new();
        checkout.safe();
        repo.checkout_tree(upstream.as_object(), Some(&mut checkout))
            .expect("prepare fast-forward worktree");

        (local_oid, upstream_oid)
    }

    #[test]
    fn credential_retry_rejects_a_changed_branch_and_remote() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let repo = init_repository(temp.path());
        repo.remote("origin", "invalid://old.example/repository")
            .expect("create original remote");
        repo.remote("other", "invalid://new.example/repository")
            .expect("create other remote");

        let head = repo
            .head()
            .expect("load HEAD")
            .peel_to_commit()
            .expect("load HEAD commit");
        repo.branch("other", &head, false)
            .expect("create other branch");
        drop(head);
        let mut config = repo.config().expect("open config");
        config
            .set_str("branch.main.remote", "origin")
            .expect("configure main remote");
        config
            .set_str("branch.main.merge", "refs/heads/main")
            .expect("configure main merge target");
        config
            .set_str("branch.other.remote", "other")
            .expect("configure other remote");
        config
            .set_str("branch.other.merge", "refs/heads/other")
            .expect("configure other merge target");
        drop(config);

        let expected = remote_operation_context(temp.path(), &repo, RemoteOperationKind::Fetch)
            .expect("capture failed operation target");
        repo.set_head("refs/heads/other")
            .expect("switch to other branch");

        let attempt = execute_remote_operation(
            temp.path(),
            RemoteOperationKind::Fetch,
            Some(("alice".to_string(), "secret".to_string())),
            Some(expected),
        );
        let error = attempt
            .result
            .expect_err("retry must reject the changed operation target");

        assert!(error.message().contains("operation target changed"));
        assert_eq!(
            attempt.context.expect("capture current target").remote_url,
            "invalid://new.example/repository"
        );
    }

    #[test]
    fn credential_retry_rejects_changed_fetch_refspecs_for_fetch_and_pull() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let repo = init_repository(temp.path());
        repo.remote("origin", "invalid://example.test/repository")
            .expect("create remote");
        let mut config = repo.config().expect("open config");
        config
            .set_str("branch.main.remote", "origin")
            .expect("configure branch remote");
        config
            .set_str("branch.main.merge", "refs/heads/main")
            .expect("configure branch merge target");
        drop(config);

        let original_refspec = "+refs/heads/main:refs/remotes/origin/main";
        let changed_refspec = "+refs/heads/release:refs/remotes/origin/release";
        for kind in [RemoteOperationKind::Fetch, RemoteOperationKind::Pull] {
            repo.config()
                .expect("open config")
                .set_str("remote.origin.fetch", original_refspec)
                .expect("configure original fetch refspec");
            let expected = remote_operation_context(temp.path(), &repo, kind)
                .expect("capture failed operation target");
            repo.config()
                .expect("open config")
                .set_str("remote.origin.fetch", changed_refspec)
                .expect("change fetch refspec");

            let attempt = execute_remote_operation(
                temp.path(),
                kind,
                Some(("alice".to_string(), "secret".to_string())),
                Some(expected),
            );
            let error = attempt
                .result
                .expect_err("retry must reject changed fetch refspecs before network use");

            assert!(error.message().contains("operation target changed"));
            assert_eq!(
                attempt
                    .context
                    .expect("capture current target")
                    .fetch_refspecs,
                Some(vec![changed_refspec.as_bytes().to_vec()])
            );
        }
    }

    #[test]
    fn pull_commit_error_restores_old_worktree_when_reference_was_not_updated() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let repo = init_repository(temp.path());
        let (local_oid, upstream_oid) = prepare_pending_fast_forward(&repo);

        let error = finish_pull_transaction(
            &repo,
            local_oid,
            upstream_oid,
            "refs/heads/main",
            Err(git2::Error::from_str(
                "simulated transaction commit failure",
            )),
        )
        .expect_err("simulated transaction failure must be reported");

        assert!(
            error
                .message()
                .contains("simulated transaction commit failure")
        );
        assert_eq!(repo.head().expect("load HEAD").target(), Some(local_oid));
        assert_eq!(
            std::fs::read_to_string(temp.path().join("tracked.txt"))
                .expect("read tracked file")
                .trim_end(),
            "initial"
        );
    }

    #[test]
    fn pull_commit_error_keeps_new_worktree_when_reference_was_updated() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let repo = init_repository(temp.path());
        let (local_oid, upstream_oid) = prepare_pending_fast_forward(&repo);
        drop(
            repo.reference_matching(
                "refs/heads/main",
                upstream_oid,
                true,
                local_oid,
                "test: simulate partially committed transaction",
            )
            .expect("advance branch reference"),
        );

        let error = finish_pull_transaction(
            &repo,
            local_oid,
            upstream_oid,
            "refs/heads/main",
            Err(git2::Error::from_str(
                "simulated transaction commit failure",
            )),
        )
        .expect_err("simulated transaction failure must be reported");

        assert!(error.message().contains("after updating"));
        assert_eq!(repo.head().expect("load HEAD").target(), Some(upstream_oid));
        assert_eq!(
            std::fs::read_to_string(temp.path().join("tracked.txt"))
                .expect("read tracked file")
                .trim_end(),
            "upstream"
        );
    }
}
