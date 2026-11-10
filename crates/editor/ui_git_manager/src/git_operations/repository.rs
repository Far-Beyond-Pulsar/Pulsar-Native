//! Opening a repository and reading its state: branches, file changes and
//! ahead/behind counts. Entry points: `open_repo`, `probe_repository`,
//! `load_repository_state`.

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

pub(super) fn read_repository_state(repo: &Repository) -> Result<RepositoryState, git2::Error> {
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

pub(super) fn load_branches(repo: &Repository) -> Result<Vec<Branch>, git2::Error> {
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

pub(super) fn load_file_changes(
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

pub(super) fn get_ahead_behind(repo: &Repository) -> Result<(usize, usize), git2::Error> {
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
