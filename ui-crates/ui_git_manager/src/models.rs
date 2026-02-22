//! Git data models

use chrono::{DateTime, Utc};

/// Repository state snapshot
#[derive(Debug, Clone, Default)]
pub struct RepositoryState {
    pub current_branch: String,
    pub branches: Vec<Branch>,
    pub commits: Vec<Commit>,
    pub staged_files: Vec<FileChange>,
    pub unstaged_files: Vec<FileChange>,
    pub untracked_files: Vec<String>,
    pub ahead: usize,
    pub behind: usize,
}

/// Branch information
#[derive(Debug, Clone)]
pub struct Branch {
    pub name: String,
    pub is_current: bool,
    pub is_remote: bool,
    pub last_commit: Option<String>,
}

/// Commit information
#[derive(Debug, Clone)]
pub struct Commit {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub email: String,
    pub message: String,
    pub timestamp: DateTime<Utc>,
    pub files_changed: usize,
}

/// File change information
#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: String,
    pub status: ChangeStatus,
    pub additions: usize,
    pub deletions: usize,
}

/// Change status
#[derive(Debug, Clone, PartialEq)]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Untracked,
}

impl ChangeStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChangeStatus::Added => "Added",
            ChangeStatus::Modified => "Modified",
            ChangeStatus::Deleted => "Deleted",
            ChangeStatus::Renamed => "Renamed",
            ChangeStatus::Copied => "Copied",
            ChangeStatus::Untracked => "Untracked",
        }
    }

    pub fn short_str(&self) -> &'static str {
        match self {
            ChangeStatus::Added => "A",
            ChangeStatus::Modified => "M",
            ChangeStatus::Deleted => "D",
            ChangeStatus::Renamed => "R",
            ChangeStatus::Copied => "C",
            ChangeStatus::Untracked => "U",
        }
    }
}
