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

/// Result of looking for a repository at a path.
pub enum RepositoryProbe {
    /// The path is not inside a git repository.
    NotARepository,
    Repository(RepositoryState),
}

/// Load the repository state, telling "no repository here" apart from a real
/// failure (blocking — run on background executor).
pub fn probe_repository(path: &Path) -> Result<RepositoryProbe, git2::Error> {
    if path.as_os_str().is_empty() {
        return Ok(RepositoryProbe::NotARepository);
    }
    match Repository::discover(path).or_else(|_| Repository::open(path)) {
        Ok(repo) => Ok(RepositoryProbe::Repository(read_repository_state(&repo)?)),
        Err(e) if e.code() == ErrorCode::NotFound => Ok(RepositoryProbe::NotARepository),
        Err(e) => Err(e),
    }
}

/// Initialise a repository at `path` (blocking — run on background executor).
pub fn init_repository(path: &Path) -> Result<(), git2::Error> {
    let mut options = git2::RepositoryInitOptions::new();
    options.initial_head("main");
    Repository::init_opts(path, &options).map(|_| ())
}

/// Load the complete repository state (blocking — run on background executor)
pub fn load_repository_state(path: &Path) -> Result<RepositoryState, git2::Error> {
    let repo = open_repo(path)?;
    read_repository_state(&repo)
}

fn read_repository_state(repo: &Repository) -> Result<RepositoryState, git2::Error> {
    let (current_branch, detached, has_commits) = match repo.head() {
        Ok(head) => {
            if head.is_branch() {
                (head.shorthand().unwrap_or("HEAD").to_string(), false, true)
            } else {
                let short = head
                    .target()
                    .map(|oid| format!("{:.7}", oid))
                    .unwrap_or_else(|| "HEAD".to_string());
                (short, true, true)
            }
        }
        // Unborn branch: HEAD names a branch with no commits yet.
        Err(_) => {
            let name = repo
                .find_reference("HEAD")
                .ok()
                .and_then(|r| r.symbolic_target().ok().flatten().map(str::to_string))
                .map(|t| t.trim_start_matches("refs/heads/").to_string())
                .unwrap_or_else(|| "main".to_string());
            (name, false, false)
        }
    };

    let branches = load_branches(repo).unwrap_or_default();
    let (staged_files, unstaged_files, untracked_files) = load_file_changes(repo)?;
    let (ahead, behind) = get_ahead_behind(repo).unwrap_or((0, 0));
    let operation = match repo.state() {
        git2::RepositoryState::Clean => None,
        git2::RepositoryState::Merge => Some("Merge in progress"),
        git2::RepositoryState::Revert | git2::RepositoryState::RevertSequence => {
            Some("Revert in progress")
        }
        git2::RepositoryState::CherryPick | git2::RepositoryState::CherryPickSequence => {
            Some("Cherry-pick in progress")
        }
        git2::RepositoryState::Rebase
        | git2::RepositoryState::RebaseInteractive
        | git2::RepositoryState::RebaseMerge => Some("Rebase in progress"),
        _ => Some("Operation in progress"),
    };

    Ok(RepositoryState {
        current_branch,
        detached,
        has_commits,
        branches,
        staged_files,
        unstaged_files,
        untracked_files,
        ahead,
        behind,
        operation,
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

fn load_file_changes(
    repo: &Repository,
) -> Result<(Vec<FileChange>, Vec<FileChange>, Vec<String>), git2::Error> {
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();

    let mut opts = StatusOptions::new();
    opts.include_untracked(true);
    opts.recurse_untracked_dirs(true);
    opts.renames_head_to_index(true);

    let statuses = repo.statuses(Some(&mut opts))?;

    for entry in statuses.iter() {
        let path = entry.path().unwrap_or("").to_string();
        if path.is_empty() {
            continue;
        }
        let status = entry.status();

        // Staged changes
        if status.is_index_new()
            || status.is_index_modified()
            || status.is_index_deleted()
            || status.is_index_renamed()
            || status.is_index_typechange()
        {
            let change_status = if status.is_index_new() {
                ChangeStatus::Added
            } else if status.is_index_deleted() {
                ChangeStatus::Deleted
            } else if status.is_index_renamed() {
                ChangeStatus::Renamed
            } else {
                ChangeStatus::Modified
            };

            let mut change = FileChange::new(path.clone(), change_status);
            if status.is_index_renamed() {
                change.old_path = entry.head_to_index().and_then(|d| {
                    d.old_file()
                        .path()
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                });
            }
            staged.push(change);
        }

        // Unstaged changes
        if status.is_wt_modified() || status.is_wt_deleted() || status.is_wt_typechange() {
            let change_status = if status.is_wt_deleted() {
                ChangeStatus::Deleted
            } else {
                ChangeStatus::Modified
            };

            unstaged.push(FileChange::new(path.clone(), change_status));
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
    let workdir_file = repo
        .workdir()
        .map(|dir| dir.join(&git_path))
        .unwrap_or_else(|| repo_path.join(&git_path));
    if workdir_file.exists() {
        index.add_path(Path::new(&git_path))?;
    } else {
        // A deleted file is staged by removing it from the index.
        index.remove_path(Path::new(&git_path))?;
    }
    index.write()?;
    Ok(())
}

/// Stage all modified/untracked/deleted files (blocking — run on background executor)
pub fn stage_all_files(repo_path: &Path) -> Result<(), git2::Error> {
    let repo = open_repo(repo_path)?;
    let mut index = repo.index()?;
    index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
    index.update_all(["*"].iter(), None)?;
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
        let _ = std::fs::remove_file(&full_path);
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
    let mut repo = open_repo(repo_path)?;
    let mut merge_heads: Vec<Oid> = Vec::new();
    if repo.state() == git2::RepositoryState::Merge {
        let _ = repo.mergehead_foreach(|oid| {
            merge_heads.push(*oid);
            true
        });
    }
    let mut index = repo.index()?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;

    let signature = repo
        .signature()
        .or_else(|_| git2::Signature::now("Pulsar", "pulsar@local"))?;

    // The first commit of a repository has no parent; a merge in progress adds
    // MERGE_HEAD as a second one.
    let mut parents: Vec<git2::Commit> = Vec::new();
    match repo.head() {
        Ok(head) => parents.push(head.peel_to_commit()?),
        Err(e) if e.code() == ErrorCode::UnbornBranch || e.code() == ErrorCode::NotFound => {}
        Err(e) => return Err(e),
    }
    if let Some(parent) = parents.first() {
        if parent.tree_id() == tree_id && repo.state() == git2::RepositoryState::Clean {
            return Err(git2::Error::from_str("Nothing staged to commit"));
        }
    }
    for oid in merge_heads {
        parents.push(repo.find_commit(oid)?);
    }
    let parent_refs: Vec<&git2::Commit> = parents.iter().collect();

    repo.commit(Some("HEAD"), &signature, &signature, message, &tree, &parent_refs)?;
    if repo.state() == git2::RepositoryState::Merge {
        repo.cleanup_state()?;
    }

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
    // Checking out a remote branch ("origin/feature") lands on a local branch
    // that tracks it instead of a detached HEAD.
    let local_target: Option<String> = match repo.find_branch(branch_name, BranchType::Remote) {
        Ok(remote_branch) => {
            let short = branch_name
                .split_once('/')
                .map(|(_, rest)| rest.to_string())
                .unwrap_or_else(|| branch_name.to_string());
            if repo.find_branch(&short, BranchType::Local).is_err() {
                let commit = remote_branch.get().peel_to_commit()?;
                let mut local = repo.branch(&short, &commit, false)?;
                let _ = local.set_upstream(Some(branch_name));
            }
            Some(short)
        }
        Err(_) => None,
    };
    let branch_name = local_target.as_deref().unwrap_or(branch_name);
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
