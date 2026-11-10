//! What a fetch, push or pull is about to run against: the remote, its URL
//! and the repository state it was resolved from, so a credential retry can
//! tell the repository changed underneath it.

use crate::{auto_fetch::RemoteOperationGuard, models::*};
use engine_fs::{FsProvider as _, LocalFsProvider};
use git2::{BranchType, ErrorCode, Oid, Repository, StatusOptions, build::CheckoutBuilder};
use std::path::{Path, PathBuf};
use super::credentials::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteOperationKind {
    Fetch,
    Push,
    Pull,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteOperationContext {
    pub(super) repository_path: PathBuf,
    pub(super) head_reference: Option<String>,
    pub(super) head_oid: Option<Oid>,
    pub(super) branch_merge: Option<String>,
    pub(super) remote_name: String,
    pub(super) remote_url: String,
    pub(super) fetch_refspecs: Option<Vec<Vec<u8>>>,
}

pub(crate) struct RemoteOperationAttempt {
    pub(crate) result: Result<(), git2::Error>,
    pub(crate) context: Option<RemoteOperationContext>,
}

/// Resolve the remote to use: branch tracking remote → "origin" → first available
pub(super) fn find_remote_name(repo: &Repository) -> Result<String, git2::Error> {
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


pub(super) fn remote_operation_url(
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

pub(super) fn remote_operation_context(
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

pub(super) fn ensure_remote_operation_context(
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

pub(super) fn context_remote<'repo>(
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

pub(super) fn same_credential_scope(configured_url: &str, requested_url: &str) -> bool {
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
