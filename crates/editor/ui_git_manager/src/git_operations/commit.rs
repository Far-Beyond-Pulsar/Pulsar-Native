//! Committing the staged changes. Entry point: `commit_staged_changes`.

use crate::{auto_fetch::RemoteOperationGuard, models::*};
use engine_fs::{FsProvider as _, LocalFsProvider};
use git2::{BranchType, ErrorCode, Oid, Repository, StatusOptions, build::CheckoutBuilder};
use std::path::{Path, PathBuf};
use super::repository::open_repo;

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

    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parent_refs,
    )?;
    if repo.state() == git2::RepositoryState::Merge {
        repo.cleanup_state()?;
    }

    Ok(())
}
