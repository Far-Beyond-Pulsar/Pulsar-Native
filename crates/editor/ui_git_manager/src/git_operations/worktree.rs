//! Working-tree operations: stage, unstage, discard, `.gitignore` edits and
//! revealing a path in the OS file manager.

use super::repository::open_repo;
use crate::models::*;
use engine_fs::{FsProvider as _, LocalFsProvider};
use git2::{StatusOptions, build::CheckoutBuilder};
use std::path::Path;

/// Stage a file (blocking — run on background executor)
pub fn stage_file(repo_path: &Path, file_path: &str) -> Result<(), git2::Error> {
    let repo = open_repo(repo_path)?;
    let mut index = repo.index()?;
    let git_path = file_path.replace('\\', "/");
    let workdir_file = repo
        .workdir()
        .map(|dir| dir.join(&git_path))
        .unwrap_or_else(|| repo_path.join(&git_path));
    if LocalFsProvider::new()
        .exists(&workdir_file)
        .unwrap_or(false)
    {
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
        LocalFsProvider::new()
            .delete_path(&full_path)
            .map_err(|e| git2::Error::from_str(&e.to_string()))?;
        return Ok(());
    }

    if status.contains(git2::Status::INDEX_NEW) {
        // Staged as new (no HEAD version) — just remove from index and delete working copy
        let mut index = repo.index()?;
        index.remove_path(std::path::Path::new(&git_path))?;
        index.write()?;
        let full_path = repo_path.join(file_path);
        let _ = LocalFsProvider::new().delete_path(&full_path);
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
        let arg = if LocalFsProvider::new()
            .metadata(path)
            .is_ok_and(|m| !m.is_dir)
        {
            format!("/select,{}", path.to_string_lossy())
        } else {
            path.to_string_lossy().to_string()
        };
        let _ = std::process::Command::new("explorer").arg(arg).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        if LocalFsProvider::new()
            .metadata(path)
            .is_ok_and(|m| !m.is_dir)
        {
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
        let dir = if LocalFsProvider::new()
            .metadata(path)
            .is_ok_and(|m| !m.is_dir)
        {
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
    let local = LocalFsProvider::new();
    let existing = local
        .read_file(&gitignore)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default();
    // Don't duplicate
    if !existing.lines().any(|l| l.trim() == line.trim()) {
        let separator = if existing.is_empty() || existing.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        local
            .write_file(
                &gitignore,
                format!("{}{}{}\n", existing, separator, line).as_bytes(),
            )
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
