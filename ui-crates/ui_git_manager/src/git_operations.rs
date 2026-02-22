//! Git operations using git2

use crate::models::*;
use git2::{Repository, StatusOptions, BranchType};
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
        let last_commit = branch.get().peel_to_commit().ok().map(|c| c.id().to_string());

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
        let last_commit = branch.get().peel_to_commit().ok().map(|c| c.id().to_string());

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

fn load_file_changes(repo: &Repository) -> Result<(Vec<FileChange>, Vec<FileChange>, Vec<String>), git2::Error> {
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

    let local_oid = head.target().ok_or_else(|| git2::Error::from_str("No target"))?;
    let branch = repo.find_branch(head.shorthand().unwrap_or(""), BranchType::Local)?;
    let upstream = branch.upstream()?;
    let upstream_oid = upstream.get().target().ok_or_else(|| git2::Error::from_str("No upstream target"))?;

    let (ahead, behind) = repo.graph_ahead_behind(local_oid, upstream_oid)?;
    Ok((ahead, behind))
}

/// Stage a file (blocking — run on background executor)
pub fn stage_file(repo_path: &Path, file_path: &str) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let mut index = repo.index()?;
    index.add_path(Path::new(file_path))?;
    index.write()?;
    Ok(())
}

/// Unstage a file (blocking — run on background executor)
pub fn unstage_file(repo_path: &Path, file_path: &str) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let head = repo.head()?.peel_to_tree()?;
    repo.reset_default(Some(&head.into_object()), &[Path::new(file_path)])?;
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

/// Push to remote (blocking — run on background executor)
pub fn push_to_remote(repo_path: &Path) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let mut remote = repo.find_remote("origin")?;
    let head = repo.head()?;
    let branch_name = head.shorthand().unwrap_or("HEAD");
    remote.push(&[format!("refs/heads/{}:refs/heads/{}", branch_name, branch_name)], None)?;
    Ok(())
}

/// Pull from remote (blocking — run on background executor)
pub fn pull_from_remote(repo_path: &Path) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let mut remote = repo.find_remote("origin")?;
    remote.fetch(&["HEAD"], None, None)?;

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

/// Switch to a different branch, carrying uncommitted changes (blocking — run on background executor)
pub fn switch_branch(repo_path: &Path, branch_name: &str) -> Result<(), git2::Error> {
    let repo = Repository::open(repo_path)?;
    let (object, reference) = repo.revparse_ext(branch_name)?;
    // SAFE strategy: refuses to overwrite local modifications, carrying them to the new branch
    let mut checkout = git2::build::CheckoutBuilder::default();
    checkout.safe();
    repo.checkout_tree(&object, Some(&mut checkout))?;
    match reference {
        Some(reference) => repo.set_head(reference.name().unwrap())?,
        None => repo.set_head_detached(object.id())?,
    }
    Ok(())
}

/// Load the content of a file for display (blocking — run on background executor).
/// Returns `None` if the file is binary, or the line count exceeds `limit`.
pub fn load_file_content(repo_path: &Path, file_path: &str, line_limit: usize) -> FileContentResult {
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
