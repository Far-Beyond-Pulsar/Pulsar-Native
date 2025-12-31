//! Git-based project synchronization for multiplayer collaboration
//!
//! ## Architecture
//!
//! Since peers are not on the same LAN, we proxy git data through the WebSocket server:
//!
//! 1. **Host creates session**: Initializes git repo, commits current state
//! 2. **Joiner requests sync**: Asks host for commit hash
//! 3. **Host serializes git objects**: Converts commit, trees, and blobs to `GitObject` structs
//! 4. **Transfer via WebSocket**: Git objects sent as JSON through multiuser_server
//! 5. **Joiner reconstructs**: Receives objects and recreates them in local git repo
//! 6. **Checkout**: Joiner checks out the synced commit
//!
//! This approach gives us:
//! - **Git's diff algorithm**: Accurate change detection
//! - **Version control**: Full git history for rollback
//! - **Conflict detection**: Git's merge capabilities
//! - **No direct network access**: Everything proxied through WebSocket server
//!
//! Uses libgit2 for all git operations

use git2::{Repository, Signature, DiffOptions, ObjectType, Oid};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Represents a git-based sync state for a project
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitSyncState {
    /// The commit hash we're synced to
    pub commit_hash: String,
    /// Number of files in the commit
    pub file_count: usize,
    /// Total size of all files
    pub total_size: u64,
}

/// Diff between local and remote git state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiff {
    /// Files that need to be added/updated
    pub changed_files: Vec<ChangedFile>,
    /// Files that need to be deleted
    pub deleted_files: Vec<String>,
    /// The remote commit we're syncing to
    pub target_commit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    pub size: u64,
    pub status: FileStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum FileStatus {
    Added,
    Modified,
}

impl GitDiff {
    pub fn has_changes(&self) -> bool {
        !self.changed_files.is_empty() || !self.deleted_files.is_empty()
    }

    pub fn change_count(&self) -> usize {
        self.changed_files.len() + self.deleted_files.len()
    }

    pub fn summary(&self) -> String {
        let added = self.changed_files.iter().filter(|f| f.status == FileStatus::Added).count();
        let modified = self.changed_files.iter().filter(|f| f.status == FileStatus::Modified).count();
        let deleted = self.deleted_files.len();

        format!("{} added, {} modified, {} deleted", added, modified, deleted)
    }
}

/// Initialize or open a git repository for a project
pub fn ensure_git_repo(project_path: &Path) -> Result<Repository, git2::Error> {
    // Try to open existing repo
    match Repository::open(project_path) {
        Ok(repo) => Ok(repo),
        Err(_) => {
            // Initialize new repo
            let repo = Repository::init(project_path)?;

            // Create .gitignore if it doesn't exist
            let gitignore_path = project_path.join(".gitignore");
            if !gitignore_path.exists() {
                let default_ignore = "\
# Build outputs
target/
dist/
build/
*.exe
*.dll
*.so
*.dylib

# IDE
.vscode/
.idea/
.vs/
*.swp
*.swo

# Logs
*.log

# OS
.DS_Store
Thumbs.db
";
                std::fs::write(gitignore_path, default_ignore).ok();
            }

            Ok(repo)
        }
    }
}

/// Create a commit with all current changes
pub fn commit_current_state(repo: &Repository, message: &str) -> Result<Oid, git2::Error> {
    let mut index = repo.index()?;

    // Add all files (respects .gitignore)
    index.add_all(["."].iter(), git2::IndexAddOption::DEFAULT, None)?;
    index.write()?;

    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;

    let signature = Signature::now("Pulsar Multiplayer", "noreply@pulsar.dev")?;

    let parent_commit = match repo.head() {
        Ok(head) => Some(head.peel_to_commit()?),
        Err(_) => None,
    };

    let parents = if let Some(ref parent) = parent_commit {
        vec![parent]
    } else {
        vec![]
    };

    let commit_id = repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parents,
    )?;

    Ok(commit_id)
}

/// Get the current commit hash
pub fn get_current_commit(repo: &Repository) -> Result<String, git2::Error> {
    let head = repo.head()?;
    let commit = head.peel_to_commit()?;
    Ok(commit.id().to_string())
}

/// Get sync state for current commit
pub fn get_sync_state(repo: &Repository) -> Result<GitSyncState, git2::Error> {
    let head = repo.head()?;
    let commit = head.peel_to_commit()?;
    let tree = commit.tree()?;

    let mut file_count = 0;
    let mut total_size = 0;

    tree.walk(git2::TreeWalkMode::PreOrder, |_, entry| {
        if let Some(git2::ObjectType::Blob) = entry.kind() {
            file_count += 1;
            if let Ok(object) = entry.to_object(repo) {
                if let Some(blob) = object.as_blob() {
                    total_size += blob.size() as u64;
                }
            }
        }
        git2::TreeWalkResult::Ok
    })?;

    Ok(GitSyncState {
        commit_hash: commit.id().to_string(),
        file_count,
        total_size,
    })
}

/// Compute diff between local commit and a remote commit hash
pub fn compute_diff(repo: &Repository, remote_commit_hash: &str) -> Result<GitDiff, git2::Error> {
    let local_commit = repo.head()?.peel_to_commit()?;
    let local_tree = local_commit.tree()?;

    let remote_oid = Oid::from_str(remote_commit_hash)?;
    let remote_commit = repo.find_commit(remote_oid)?;
    let remote_tree = remote_commit.tree()?;

    let mut diff_opts = DiffOptions::new();
    let diff = repo.diff_tree_to_tree(Some(&local_tree), Some(&remote_tree), Some(&mut diff_opts))?;

    let mut changed_files = Vec::new();
    let mut deleted_files = Vec::new();

    diff.foreach(
        &mut |delta, _progress| {
            let old_file = delta.old_file();
            let new_file = delta.new_file();

            match delta.status() {
                git2::Delta::Added => {
                    if let Some(path) = new_file.path() {
                        changed_files.push(ChangedFile {
                            path: path.to_string_lossy().to_string(),
                            size: new_file.size(),
                            status: FileStatus::Added,
                        });
                    }
                }
                git2::Delta::Modified => {
                    if let Some(path) = new_file.path() {
                        changed_files.push(ChangedFile {
                            path: path.to_string_lossy().to_string(),
                            size: new_file.size(),
                            status: FileStatus::Modified,
                        });
                    }
                }
                git2::Delta::Deleted => {
                    if let Some(path) = old_file.path() {
                        deleted_files.push(path.to_string_lossy().to_string());
                    }
                }
                _ => {}
            }

            true
        },
        None,
        None,
        None,
    )?;

    Ok(GitDiff {
        changed_files,
        deleted_files,
        target_commit: remote_commit_hash.to_string(),
    })
}

/// Get file content from a specific commit
pub fn get_file_at_commit(
    repo: &Repository,
    commit_hash: &str,
    file_path: &str,
) -> Result<Vec<u8>, git2::Error> {
    let oid = Oid::from_str(commit_hash)?;
    let commit = repo.find_commit(oid)?;
    let tree = commit.tree()?;

    let entry = tree.get_path(Path::new(file_path))?;
    let object = entry.to_object(repo)?;

    if let Some(blob) = object.as_blob() {
        Ok(blob.content().to_vec())
    } else {
        Err(git2::Error::from_str("Not a blob"))
    }
}

/// Checkout files from a specific commit (sync to remote state)
pub fn checkout_commit(repo: &Repository, commit_hash: &str) -> Result<(), git2::Error> {
    let oid = Oid::from_str(commit_hash)?;
    let commit = repo.find_commit(oid)?;
    let tree = commit.tree()?;

    // Checkout the tree
    repo.checkout_tree(tree.as_object(), None)?;

    // Update HEAD to point to this commit (detached HEAD state)
    repo.set_head_detached(oid)?;

    Ok(())
}

/// Serialized git object for network transfer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitObject {
    pub oid: String,
    pub object_type: GitObjectType,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum GitObjectType {
    Commit,
    Tree,
    Blob,
}

/// Serialize a commit and all its objects for network transfer
pub fn serialize_commit(repo: &Repository, commit_hash: &str) -> Result<Vec<GitObject>, git2::Error> {
    tracing::debug!("GIT_SYNC: Serializing commit {} for network transfer", commit_hash);

    let oid = Oid::from_str(commit_hash)?;
    let commit = repo.find_commit(oid)?;

    let mut objects = Vec::new();

    // Serialize the commit object using ODB
    let odb = repo.odb()?;
    let commit_obj = odb.read(oid)?;
    objects.push(GitObject {
        oid: oid.to_string(),
        object_type: GitObjectType::Commit,
        data: commit_obj.data().to_vec(),
    });

    // Serialize the tree and all blobs
    let tree = commit.tree()?;
    serialize_tree(repo, &tree, &mut objects)?;

    let blob_count = objects.iter().filter(|o| o.object_type == GitObjectType::Blob).count();
    let tree_count = objects.iter().filter(|o| o.object_type == GitObjectType::Tree).count();
    tracing::debug!("GIT_SYNC: Serialized {} git objects total (1 commit, {} trees, {} blobs)",
        objects.len(), tree_count, blob_count);

    Ok(objects)
}

/// Recursively serialize a tree and its contents
fn serialize_tree(repo: &Repository, tree: &git2::Tree, objects: &mut Vec<GitObject>) -> Result<(), git2::Error> {
    // Serialize tree object itself using ODB
    let odb = repo.odb()?;
    let tree_obj = odb.read(tree.id())?;
    objects.push(GitObject {
        oid: tree.id().to_string(),
        object_type: GitObjectType::Tree,
        data: tree_obj.data().to_vec(),
    });

    // Walk tree entries
    tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
        if let Some(git2::ObjectType::Blob) = entry.kind() {
            // Serialize blob
            if let Ok(object) = entry.to_object(repo) {
                if let Some(blob) = object.as_blob() {
                    objects.push(GitObject {
                        oid: blob.id().to_string(),
                        object_type: GitObjectType::Blob,
                        data: blob.content().to_vec(),
                    });
                }
            }
        } else if let Some(git2::ObjectType::Tree) = entry.kind() {
            // Recurse into subdirectory
            if let Ok(subtree) = repo.find_tree(entry.id()) {
                serialize_tree(repo, &subtree, objects).ok();
            }
        }
        git2::TreeWalkResult::Ok
    })?;

    Ok(())
}

/// Reconstruct git objects in local repository from serialized data
pub fn reconstruct_objects(repo: &Repository, objects: Vec<GitObject>) -> Result<String, git2::Error> {
    tracing::debug!("GIT_SYNC: Reconstructing {} git objects in repository", objects.len());
    let mut commit_oid = None;
    let odb = repo.odb()?;

    // Write all objects to ODB
    for obj in objects {
        let git_type = match obj.object_type {
            GitObjectType::Blob => ObjectType::Blob,
            GitObjectType::Tree => ObjectType::Tree,
            GitObjectType::Commit => {
                commit_oid = Some(obj.oid.clone());
                ObjectType::Commit
            }
        };

        // Write raw object data to ODB
        let written_oid = odb.write(git_type, &obj.data)?;
        tracing::debug!("GIT_SYNC: Wrote {:?} object {}", git_type, written_oid);

        // Verify OID matches
        let expected_oid = Oid::from_str(&obj.oid)?;
        if written_oid != expected_oid {
            tracing::warn!("GIT_SYNC: OID mismatch! Expected {}, got {}", obj.oid, written_oid);
        }
    }

    tracing::debug!("GIT_SYNC: Successfully reconstructed all git objects");
    commit_oid.ok_or_else(|| git2::Error::from_str("No commit found in objects"))
}

/// Extract files with their paths from a commit's tree
pub fn extract_files_from_commit(
    repo: &Repository,
    commit_hash: &str,
) -> Result<Vec<(PathBuf, Vec<u8>)>, git2::Error> {
    tracing::debug!("GIT_SYNC: Extracting files from commit {}", commit_hash);

    let oid = Oid::from_str(commit_hash)?;
    let commit = repo.find_commit(oid)?;
    let tree = commit.tree()?;

    tracing::debug!("GIT_SYNC: Found commit tree, walking entries...");

    let mut files = Vec::new();
    extract_files_from_tree(repo, &tree, PathBuf::new(), &mut files)?;

    tracing::debug!("GIT_SYNC: Extracted {} files from commit", files.len());
    for (path, data) in &files {
        tracing::debug!("GIT_SYNC:   - {} ({} bytes)", path.display(), data.len());
    }

    Ok(files)
}

/// Recursively extract files from a tree
fn extract_files_from_tree(
    repo: &Repository,
    tree: &git2::Tree,
    base_path: PathBuf,
    files: &mut Vec<(PathBuf, Vec<u8>)>,
) -> Result<(), git2::Error> {
    tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
        let entry_path = if root.is_empty() {
            PathBuf::from(entry.name().unwrap_or(""))
        } else {
            PathBuf::from(root).join(entry.name().unwrap_or(""))
        };

        match entry.kind() {
            Some(git2::ObjectType::Blob) => {
                if let Ok(object) = entry.to_object(repo) {
                    if let Some(blob) = object.as_blob() {
                        files.push((entry_path, blob.content().to_vec()));
                    }
                }
                git2::TreeWalkResult::Ok
            }
            Some(git2::ObjectType::Tree) => {
                // TreeWalk handles recursion automatically
                git2::TreeWalkResult::Ok
            }
            _ => git2::TreeWalkResult::Skip,
        }
    })?;

    Ok(())
}

/// Create a commit from received files (simplified approach)
pub fn create_commit_from_files(
    repo: &Repository,
    files: Vec<(PathBuf, Vec<u8>)>,
    message: &str,
) -> Result<Oid, git2::Error> {
    // Add files to working directory
    for (path, content) in &files {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(path, content).ok();
    }

    // Stage and commit
    let mut index = repo.index()?;
    index.add_all(["."].iter(), git2::IndexAddOption::DEFAULT, None)?;
    index.write()?;

    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;

    let signature = Signature::now("Pulsar Multiplayer", "noreply@pulsar.dev")?;

    let parent_commit = match repo.head() {
        Ok(head) => Some(head.peel_to_commit()?),
        Err(_) => None,
    };

    let parents = if let Some(ref parent) = parent_commit {
        vec![parent]
    } else {
        vec![]
    };

    let commit_id = repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parents,
    )?;

    Ok(commit_id)
}
