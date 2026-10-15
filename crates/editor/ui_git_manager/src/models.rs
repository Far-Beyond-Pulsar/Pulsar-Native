//! Git data models

use chrono::{DateTime, Utc};

/// Repository state snapshot (everything except the commit history, which is
/// loaded in pages — see [`crate::history`]).
#[derive(Debug, Clone, Default)]
pub struct RepositoryState {
    pub current_branch: String,
    /// HEAD points at a commit instead of a branch.
    pub detached: bool,
    /// The repository has at least one commit.
    pub has_commits: bool,
    pub branches: Vec<Branch>,
    pub staged_files: Vec<FileChange>,
    pub unstaged_files: Vec<FileChange>,
    pub untracked_files: Vec<String>,
    pub ahead: usize,
    pub behind: usize,
    /// A merge / rebase / cherry-pick is in progress.
    pub operation: Option<&'static str>,
}

/// Branch information
#[derive(Debug, Clone)]
pub struct Branch {
    pub name: String,
    pub is_current: bool,
    pub is_remote: bool,
    pub last_commit: Option<String>,
}

/// File change information
#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: String,
    /// The path before a rename or copy.
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    pub additions: usize,
    pub deletions: usize,
}

impl FileChange {
    pub fn new(path: impl Into<String>, status: ChangeStatus) -> Self {
        Self {
            path: path.into(),
            old_path: None,
            status,
            additions: 0,
            deletions: 0,
        }
    }

    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    pub fn directory(&self) -> &str {
        match self.path.rfind('/') {
            Some(index) => &self.path[..index],
            None => "",
        }
    }
}

/// Change status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// What a ref label on a commit points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Head,
    Local,
    Remote,
    Tag,
}

#[derive(Debug, Clone)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
}

/// One drawn lane: `(lane index, colour index)`.
pub type Lane = (usize, usize);

/// How a commit row is wired into the history graph.
#[derive(Debug, Clone, Default)]
pub struct GraphRow {
    /// Lane the commit's node sits on.
    pub lane: usize,
    pub color: usize,
    /// Lanes that arrive at the node from above.
    pub incoming: Vec<Lane>,
    /// Lanes that run straight through the row.
    pub through: Vec<Lane>,
    /// Lanes that leave the node downwards (its parents).
    pub outgoing: Vec<Lane>,
    /// Number of lanes occupied anywhere on this row.
    pub lane_count: usize,
}

/// A commit in the history list.
#[derive(Debug, Clone)]
pub struct GraphCommit {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub email: String,
    pub summary: String,
    pub timestamp: DateTime<Utc>,
    pub parents: Vec<String>,
    pub refs: Vec<RefLabel>,
    pub graph: GraphRow,
}

/// Everything about a single commit.
#[derive(Debug, Clone)]
pub struct CommitDetail {
    pub hash: String,
    pub author: String,
    pub email: String,
    pub committer: String,
    pub timestamp: DateTime<Utc>,
    pub message: String,
    pub parents: Vec<String>,
    pub files: Vec<FileChange>,
}
