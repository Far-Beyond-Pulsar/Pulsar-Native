//! Fetch, push and pull against a remote, with the pull rollback and the
//! credential-retry transaction. Entry points: `fetch_from_remote`,
//! `push_to_remote`, `pull_from_remote`.

use crate::{auto_fetch::RemoteOperationGuard, models::*};
use engine_fs::{FsProvider as _, LocalFsProvider};
use git2::{BranchType, ErrorCode, Oid, Repository, StatusOptions, build::CheckoutBuilder};
use std::path::{Path, PathBuf};
use super::remote_context::*;
use super::credentials::*;
use super::repository::open_repo;

pub(super) fn fetch_remote(
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

pub(super) fn push_remote(
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

pub(super) fn rollback_pull_worktree(
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

pub(super) fn finish_pull_transaction(
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

pub(super) fn pull_remote<F>(
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

pub(super) fn remote_operation_attempt_error(error: git2::Error) -> RemoteOperationAttempt {
    RemoteOperationAttempt {
        result: Err(error),
        context: None,
    }
}

pub(super) fn execute_remote_operation_unlocked(
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
