//! Switching branches. Entry point: `switch_branch`.

use super::repository::open_repo;
use crate::{auto_fetch::RemoteOperationGuard, models::*};
use engine_fs::FsProvider as _;
use git2::{BranchType, StatusOptions, build::CheckoutBuilder};
use std::path::Path;

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
