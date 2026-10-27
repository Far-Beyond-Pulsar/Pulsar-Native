//! Commit history, commit details and diff sources.
//!
//! Everything here is blocking and meant to run on the background executor.
//! History is read in pages so the commit list can be virtualized over a
//! repository of any size: the graph builder is carried from one page to the
//! next so lanes stay stable while scrolling.

use crate::git_operations::open_repo;
use crate::models::*;
use git2::{Oid, Repository, Sort};
use std::collections::HashMap;
use std::path::Path;

/// Commits loaded per page.
pub const COMMIT_PAGE: usize = 400;
/// Largest blob shown in the diff view.
const MAX_DIFF_BYTES: usize = 3 * 1024 * 1024;

// ── Graph ────────────────────────────────────────────────────────────────────

/// Assigns commits to lanes, top to bottom.
#[derive(Debug, Clone, Default)]
pub struct GraphBuilder {
    /// For every lane, the commit it is waiting for and its colour.
    lanes: Vec<Option<(Oid, usize)>>,
    next_color: usize,
}

impl GraphBuilder {
    fn free_lane(&mut self) -> usize {
        match self.lanes.iter().position(|lane| lane.is_none()) {
            Some(index) => index,
            None => {
                self.lanes.push(None);
                self.lanes.len() - 1
            }
        }
    }

    fn new_color(&mut self) -> usize {
        let color = self.next_color;
        self.next_color = self.next_color.wrapping_add(1);
        color
    }

    pub fn push(&mut self, oid: Oid, parents: &[Oid]) -> GraphRow {
        let before = self.lanes.clone();
        let matching: Vec<usize> = before
            .iter()
            .enumerate()
            .filter(|(_, lane)| matches!(lane, Some((o, _)) if *o == oid))
            .map(|(index, _)| index)
            .collect();

        let (lane, color) = match matching.first() {
            Some(&first) => (first, before[first].map(|l| l.1).unwrap_or(0)),
            None => {
                let color = self.new_color();
                (self.free_lane(), color)
            }
        };

        let incoming: Vec<Lane> = matching
            .iter()
            .map(|&i| (i, before[i].map(|l| l.1).unwrap_or(0)))
            .collect();
        for &i in &matching {
            self.lanes[i] = None;
        }

        let mut outgoing: Vec<Lane> = Vec::new();
        for (n, parent) in parents.iter().enumerate() {
            let existing = self
                .lanes
                .iter()
                .position(|l| matches!(l, Some((o, _)) if o == parent));
            match existing {
                Some(j) => {
                    let c = self.lanes[j].map(|l| l.1).unwrap_or(0);
                    outgoing.push((j, c));
                }
                None if n == 0 => {
                    while self.lanes.len() <= lane {
                        self.lanes.push(None);
                    }
                    self.lanes[lane] = Some((*parent, color));
                    outgoing.push((lane, color));
                }
                None => {
                    let c = self.new_color();
                    let k = self.free_lane();
                    self.lanes[k] = Some((*parent, c));
                    outgoing.push((k, c));
                }
            }
        }

        let through: Vec<Lane> = before
            .iter()
            .enumerate()
            .filter(|(i, l)| l.is_some() && !matching.contains(i))
            .map(|(i, l)| (i, l.map(|l| l.1).unwrap_or(0)))
            .collect();

        while matches!(self.lanes.last(), Some(None)) {
            self.lanes.pop();
        }

        let widest = incoming
            .iter()
            .chain(through.iter())
            .chain(outgoing.iter())
            .map(|(i, _)| *i + 1)
            .max()
            .unwrap_or(0)
            .max(lane + 1);

        GraphRow {
            lane,
            color,
            incoming,
            through,
            outgoing,
            lane_count: widest,
        }
    }
}

// ── Pages ────────────────────────────────────────────────────────────────────

fn collect_refs(repo: &Repository) -> HashMap<Oid, Vec<RefLabel>> {
    let mut map: HashMap<Oid, Vec<RefLabel>> = HashMap::new();
    let head_name = repo
        .head()
        .ok()
        .filter(|head| head.is_branch())
        .and_then(|head| head.name().ok().map(str::to_string));
    let head_oid = repo.head().ok().and_then(|head| head.target());

    let Ok(references) = repo.references() else {
        return map;
    };
    for reference in references.flatten() {
        let Ok(name) = reference.name() else {
            continue;
        };
        let (label, kind) = if let Some(short) = name.strip_prefix("refs/heads/") {
            let kind = if Some(name) == head_name.as_deref() {
                RefKind::Head
            } else {
                RefKind::Local
            };
            (short.to_string(), kind)
        } else if let Some(short) = name.strip_prefix("refs/remotes/") {
            if short.ends_with("/HEAD") {
                continue;
            }
            (short.to_string(), RefKind::Remote)
        } else if let Some(short) = name.strip_prefix("refs/tags/") {
            (short.to_string(), RefKind::Tag)
        } else {
            continue;
        };
        let Ok(commit) = reference.peel_to_commit() else {
            continue;
        };
        map.entry(commit.id())
            .or_default()
            .push(RefLabel { name: label, kind });
    }
    // A detached HEAD is shown as its own label.
    if head_name.is_none() {
        if let Some(oid) = head_oid {
            map.entry(oid).or_default().push(RefLabel {
                name: "HEAD".to_string(),
                kind: RefKind::Head,
            });
        }
    }
    for labels in map.values_mut() {
        labels.sort_by_key(|label| match label.kind {
            RefKind::Head => 0,
            RefKind::Local => 1,
            RefKind::Remote => 2,
            RefKind::Tag => 3,
        });
    }
    map
}

/// A page of history.
pub struct CommitPage {
    pub commits: Vec<GraphCommit>,
    pub graph: GraphBuilder,
    /// The history ended inside this page.
    pub exhausted: bool,
}

/// Load `limit` commits after the first `skip`, across every branch.
///
/// An empty repository has no history; that is an empty page, not an error.
pub fn load_commit_page(
    repo_path: &Path,
    skip: usize,
    limit: usize,
    mut graph: GraphBuilder,
) -> Result<CommitPage, git2::Error> {
    let repo = open_repo(repo_path)?;
    let mut revwalk = repo.revwalk()?;
    // Topological order keeps every commit above its parents, which the lane
    // assignment relies on, and makes pages line up between walks.
    revwalk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
    let mut pushed = false;
    let _ = revwalk.push_head().map(|_| pushed = true);
    for glob in ["refs/heads/*", "refs/remotes/*"] {
        if revwalk.push_glob(glob).is_ok() {
            pushed = true;
        }
    }
    if !pushed {
        return Ok(CommitPage {
            commits: Vec::new(),
            graph,
            exhausted: true,
        });
    }

    let refs = collect_refs(&repo);
    let mut commits = Vec::with_capacity(limit);
    let mut count = 0usize;
    for oid in revwalk.skip(skip) {
        if count == limit {
            break;
        }
        let oid = oid?;
        let Ok(commit) = repo.find_commit(oid) else {
            continue;
        };
        count += 1;
        let parent_ids: Vec<Oid> = commit.parent_ids().collect();
        let row = graph.push(oid, &parent_ids);
        let author = commit.author();
        commits.push(GraphCommit {
            hash: oid.to_string(),
            short_hash: format!("{:.7}", oid),
            author: author.name().unwrap_or("Unknown").to_string(),
            email: author.email().unwrap_or("").to_string(),
            summary: commit.summary().ok().flatten().unwrap_or("").to_string(),
            timestamp: chrono::DateTime::from_timestamp(commit.time().seconds(), 0)
                .unwrap_or_else(chrono::Utc::now),
            parents: parent_ids.iter().map(|p| p.to_string()).collect(),
            refs: refs.get(&oid).cloned().unwrap_or_default(),
            graph: row,
        });
    }
    Ok(CommitPage {
        exhausted: count < limit,
        commits,
        graph,
    })
}

// ── Commit detail ────────────────────────────────────────────────────────────

fn delta_status(status: git2::Delta) -> ChangeStatus {
    match status {
        git2::Delta::Added => ChangeStatus::Added,
        git2::Delta::Deleted => ChangeStatus::Deleted,
        git2::Delta::Renamed => ChangeStatus::Renamed,
        git2::Delta::Copied => ChangeStatus::Copied,
        _ => ChangeStatus::Modified,
    }
}

/// Load a commit with the files it changed against its first parent.
pub fn load_commit_detail(repo_path: &Path, hash: &str) -> Result<CommitDetail, git2::Error> {
    let repo = open_repo(repo_path)?;
    let commit = repo.find_commit(Oid::from_str(hash)?)?;
    let tree = commit.tree()?;
    let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());

    let mut diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;
    let _ = diff.find_similar(None);

    let mut files = Vec::new();
    for index in 0..diff.deltas().len() {
        let Some(delta) = diff.get_delta(index) else {
            continue;
        };
        let new_path = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().replace('\\', "/"));
        let old_path = delta
            .old_file()
            .path()
            .map(|p| p.to_string_lossy().replace('\\', "/"));
        let path = new_path
            .clone()
            .or_else(|| old_path.clone())
            .unwrap_or_default();
        let status = delta_status(delta.status());
        let mut change = FileChange::new(path, status);
        if matches!(status, ChangeStatus::Renamed | ChangeStatus::Copied) {
            change.old_path = old_path;
        }
        // Line counts are only worth their cost on commits of a sane size.
        if index < 400 {
            if let Ok(Some(patch)) = git2::Patch::from_diff(&diff, index) {
                if let Ok((_, additions, deletions)) = patch.line_stats() {
                    change.additions = additions;
                    change.deletions = deletions;
                }
            }
        }
        files.push(change);
    }

    let author = commit.author();
    let committer = commit.committer();
    Ok(CommitDetail {
        hash: commit.id().to_string(),
        author: author.name().unwrap_or("Unknown").to_string(),
        email: author.email().unwrap_or("").to_string(),
        committer: committer.name().unwrap_or("Unknown").to_string(),
        timestamp: chrono::DateTime::from_timestamp(commit.time().seconds(), 0)
            .unwrap_or_else(chrono::Utc::now),
        message: commit.message().unwrap_or("").trim_end().to_string(),
        parents: commit.parent_ids().map(|p| p.to_string()).collect(),
        files,
    })
}

// ── Branches ─────────────────────────────────────────────────────────────────

/// Create a branch at HEAD and switch to it. The working tree already matches
/// HEAD's commit, so nothing needs checking out.
pub fn create_branch(repo_path: &Path, name: &str) -> Result<(), git2::Error> {
    let repo = open_repo(repo_path)?;
    let name = name.trim();
    if !git2::Branch::name_is_valid(name)? {
        return Err(git2::Error::from_str(&format!(
            "\"{}\" is not a valid branch name",
            name
        )));
    }
    let head = repo
        .head()
        .map_err(|_| git2::Error::from_str("Make a first commit before creating branches"))?;
    let commit = head.peel_to_commit()?;
    repo.branch(name, &commit, false)?;
    repo.set_head(&format!("refs/heads/{}", name))?;
    Ok(())
}

// ── Diff sources ─────────────────────────────────────────────────────────────

/// What a file diff compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffTarget {
    /// The index against HEAD.
    Staged {
        path: String,
        old_path: Option<String>,
    },
    /// The working tree against the index.
    Unstaged { path: String },
    /// A commit against its first parent.
    Commit {
        hash: String,
        path: String,
        old_path: Option<String>,
    },
}

impl DiffTarget {
    pub fn path(&self) -> &str {
        match self {
            DiffTarget::Staged { path, .. }
            | DiffTarget::Unstaged { path }
            | DiffTarget::Commit { path, .. } => path,
        }
    }
}

/// The two sides of a file diff.
#[derive(Debug, Clone)]
pub enum DiffTexts {
    Text {
        old: String,
        new: String,
    },
    /// Binary or oversized; the reason is shown in place of the diff.
    Unsupported(String),
}

enum Blob {
    Missing,
    Text(String),
    Unsupported(String),
}

fn decode_blob(bytes: &[u8]) -> Blob {
    if bytes.len() > MAX_DIFF_BYTES {
        return Blob::Unsupported(format!(
            "File is too large to diff ({:.1} MB)",
            bytes.len() as f64 / (1024.0 * 1024.0)
        ));
    }
    if bytes[..bytes.len().min(8192)].contains(&0u8) {
        return Blob::Unsupported("Binary file".to_string());
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => Blob::Text(text.replace("\r\n", "\n")),
        // Latin-1 style files still deserve a diff.
        Err(_) => Blob::Text(String::from_utf8_lossy(bytes).replace("\r\n", "\n")),
    }
}

fn blob_in_tree(repo: &Repository, tree: Option<&git2::Tree>, path: &str) -> Blob {
    let Some(tree) = tree else {
        return Blob::Missing;
    };
    let normalized = path.replace('\\', "/");
    let Ok(entry) = tree.get_path(Path::new(&normalized)) else {
        return Blob::Missing;
    };
    match repo.find_blob(entry.id()) {
        Ok(blob) => decode_blob(blob.content()),
        Err(_) => Blob::Missing,
    }
}

fn blob_in_index(repo: &Repository, path: &str) -> Result<Blob, git2::Error> {
    let index = repo.index()?;
    let normalized = path.replace('\\', "/");
    Ok(match index.get_path(Path::new(&normalized), 0) {
        Some(entry) => match repo.find_blob(entry.id) {
            Ok(blob) => decode_blob(blob.content()),
            Err(_) => Blob::Missing,
        },
        None => Blob::Missing,
    })
}

fn pair(old: Blob, new: Blob) -> DiffTexts {
    match (old, new) {
        (Blob::Unsupported(reason), _) | (_, Blob::Unsupported(reason)) => {
            DiffTexts::Unsupported(reason)
        }
        (old, new) => {
            let text = |blob: Blob| match blob {
                Blob::Text(text) => text,
                _ => String::new(),
            };
            DiffTexts::Text {
                old: text(old),
                new: text(new),
            }
        }
    }
}

/// Load both sides of a file diff.
pub fn load_diff_texts(repo_path: &Path, target: &DiffTarget) -> Result<DiffTexts, git2::Error> {
    let repo = open_repo(repo_path)?;
    let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());
    match target {
        DiffTarget::Staged { path, old_path } => {
            let old = blob_in_tree(
                &repo,
                head_tree.as_ref(),
                old_path.as_deref().unwrap_or(path),
            );
            let new = blob_in_index(&repo, path)?;
            Ok(pair(old, new))
        }
        DiffTarget::Unstaged { path } => {
            let from_index = blob_in_index(&repo, path)?;
            // A file only in the working tree (untracked) has no old side.
            let old = match from_index {
                Blob::Missing => blob_in_tree(&repo, head_tree.as_ref(), path),
                other => other,
            };
            let workdir = repo
                .workdir()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| repo_path.to_path_buf());
            let new = match std::fs::read(workdir.join(path)) {
                Ok(bytes) => decode_blob(&bytes),
                Err(_) => Blob::Missing,
            };
            Ok(pair(old, new))
        }
        DiffTarget::Commit {
            hash,
            path,
            old_path,
        } => {
            let commit = repo.find_commit(Oid::from_str(hash)?)?;
            let tree = commit.tree()?;
            let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
            let old = blob_in_tree(
                &repo,
                parent_tree.as_ref(),
                old_path.as_deref().unwrap_or(path),
            );
            let new = blob_in_tree(&repo, Some(&tree), path);
            Ok(pair(old, new))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(n: u8) -> Oid {
        Oid::from_bytes(&[n; 20]).unwrap()
    }

    #[test]
    fn linear_history_stays_on_one_lane() {
        let mut graph = GraphBuilder::default();
        let a = graph.push(oid(3), &[oid(2)]);
        let b = graph.push(oid(2), &[oid(1)]);
        let c = graph.push(oid(1), &[]);
        assert_eq!((a.lane, b.lane, c.lane), (0, 0, 0));
        assert!(c.outgoing.is_empty());
        assert_eq!(b.incoming.len(), 1);
    }

    #[test]
    fn merge_opens_and_closes_a_second_lane() {
        let mut graph = GraphBuilder::default();
        // merge(4) has parents 2 (main) and 3 (feature)
        let merge = graph.push(oid(4), &[oid(2), oid(3)]);
        assert_eq!(merge.outgoing.len(), 2);
        let feature = graph.push(oid(3), &[oid(1)]);
        assert_eq!(feature.lane, 1);
        assert_eq!(feature.through.len(), 1);
        let main = graph.push(oid(2), &[oid(1)]);
        assert_eq!(main.lane, 0);
        // main joins the lane the feature branch already waits on.
        assert_eq!(main.outgoing[0].0, 1);
        let base = graph.push(oid(1), &[]);
        assert_eq!(base.lane, 1);
        assert_eq!(base.incoming.len(), 1);
    }
}

#[cfg(test)]
mod repository_tests {
    use super::*;
    use crate::git_operations::{
        RepositoryProbe, commit_staged_changes, init_repository, probe_repository, stage_file,
        switch_branch,
    };

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::write(dir.join(name), text).expect("write file");
    }

    fn state(dir: &Path) -> RepositoryState {
        match probe_repository(dir).expect("probe") {
            RepositoryProbe::Repository(state) => state,
            RepositoryProbe::NotARepository => panic!("expected a repository"),
        }
    }

    #[test]
    fn folder_without_a_repository_is_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let probe = probe_repository(dir.path()).expect("probe");
        assert!(matches!(probe, RepositoryProbe::NotARepository));
    }

    #[test]
    fn initialized_repository_can_make_its_first_commit() {
        let dir = tempfile::tempdir().expect("tempdir");
        init_repository(dir.path()).expect("init");
        let fresh = state(dir.path());
        assert!(!fresh.has_commits);
        assert_eq!(fresh.current_branch, "main");

        write(dir.path(), "a.txt", "one\n");
        stage_file(dir.path(), "a.txt").expect("stage");
        commit_staged_changes(dir.path(), "first").expect("first commit has no parent");

        let page =
            load_commit_page(dir.path(), 0, COMMIT_PAGE, GraphBuilder::default()).expect("history");
        assert_eq!(page.commits.len(), 1);
        assert_eq!(page.commits[0].summary, "first");
        assert!(page.exhausted);
        assert!(state(dir.path()).has_commits);
    }

    #[test]
    fn deleted_file_can_be_staged() {
        let dir = tempfile::tempdir().expect("tempdir");
        init_repository(dir.path()).expect("init");
        write(dir.path(), "gone.txt", "bye\n");
        stage_file(dir.path(), "gone.txt").expect("stage");
        commit_staged_changes(dir.path(), "add").expect("commit");

        std::fs::remove_file(dir.path().join("gone.txt")).expect("delete");
        assert_eq!(state(dir.path()).unstaged_files.len(), 1);
        stage_file(dir.path(), "gone.txt").expect("staging a deletion");
        let after = state(dir.path());
        assert!(after.unstaged_files.is_empty());
        assert_eq!(after.staged_files[0].status, ChangeStatus::Deleted);
    }

    #[test]
    fn empty_commit_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        init_repository(dir.path()).expect("init");
        write(dir.path(), "a.txt", "one\n");
        stage_file(dir.path(), "a.txt").expect("stage");
        commit_staged_changes(dir.path(), "first").expect("commit");
        assert!(commit_staged_changes(dir.path(), "again").is_err());
    }

    #[test]
    fn history_pages_continue_where_the_last_one_stopped() {
        let dir = tempfile::tempdir().expect("tempdir");
        init_repository(dir.path()).expect("init");
        for n in 0..5 {
            write(dir.path(), "a.txt", &format!("{}\n", n));
            stage_file(dir.path(), "a.txt").expect("stage");
            commit_staged_changes(dir.path(), &format!("commit {}", n)).expect("commit");
        }
        let first = load_commit_page(dir.path(), 0, 3, GraphBuilder::default()).expect("page");
        assert_eq!(first.commits.len(), 3);
        assert!(!first.exhausted);
        let second = load_commit_page(dir.path(), 3, 3, first.graph).expect("page");
        assert_eq!(second.commits.len(), 2);
        assert!(second.exhausted);
        assert!(second.commits.iter().all(|c| c.graph.lane == 0));
    }

    #[test]
    fn diff_sources_pick_the_right_side() {
        let dir = tempfile::tempdir().expect("tempdir");
        init_repository(dir.path()).expect("init");
        write(dir.path(), "a.txt", "one\n");
        stage_file(dir.path(), "a.txt").expect("stage");
        commit_staged_changes(dir.path(), "first").expect("commit");

        write(dir.path(), "a.txt", "two\n");
        stage_file(dir.path(), "a.txt").expect("stage");
        write(dir.path(), "a.txt", "three\n");

        let side = |target: DiffTarget| match load_diff_texts(dir.path(), &target).expect("diff") {
            DiffTexts::Text { old, new } => (old, new),
            DiffTexts::Unsupported(reason) => panic!("unsupported: {}", reason),
        };
        let staged = side(DiffTarget::Staged {
            path: "a.txt".into(),
            old_path: None,
        });
        assert_eq!(staged, ("one\n".to_string(), "two\n".to_string()));
        let unstaged = side(DiffTarget::Unstaged {
            path: "a.txt".into(),
        });
        assert_eq!(unstaged, ("two\n".to_string(), "three\n".to_string()));

        let head = load_commit_page(dir.path(), 0, 1, GraphBuilder::default())
            .expect("page")
            .commits
            .remove(0);
        let detail = load_commit_detail(dir.path(), &head.hash).expect("detail");
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.files[0].additions, 1);
        let committed = side(DiffTarget::Commit {
            hash: head.hash,
            path: "a.txt".into(),
            old_path: None,
        });
        assert_eq!(committed, ("".to_string(), "one\n".to_string()));
    }

    #[test]
    fn binary_files_are_not_diffed_as_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        init_repository(dir.path()).expect("init");
        std::fs::write(dir.path().join("blob.bin"), [0u8, 1, 2, 3]).expect("write");
        let result = load_diff_texts(
            dir.path(),
            &DiffTarget::Unstaged {
                path: "blob.bin".into(),
            },
        )
        .expect("diff");
        assert!(matches!(result, DiffTexts::Unsupported(_)));
    }

    #[test]
    fn branches_can_be_created_and_switched() {
        let dir = tempfile::tempdir().expect("tempdir");
        init_repository(dir.path()).expect("init");
        write(dir.path(), "a.txt", "one\n");
        stage_file(dir.path(), "a.txt").expect("stage");
        commit_staged_changes(dir.path(), "first").expect("commit");

        create_branch(dir.path(), "feature/x").expect("create");
        assert_eq!(state(dir.path()).current_branch, "feature/x");
        assert!(create_branch(dir.path(), "bad name").is_err());
        switch_branch(dir.path(), "main").expect("switch");
        assert_eq!(state(dir.path()).current_branch, "main");
    }
}
