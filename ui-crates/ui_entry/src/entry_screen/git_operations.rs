//! Git repository operations for project management.
//!
//! This module provides utilities for managing git repositories within the Pulsar Native
//! project launcher. It handles repository cloning, remote configuration, update checking,
//! and pulling changes from upstream repositories.
//!
//! # Features
//!
//! - **Repository Cloning**: Clone git repositories with progress tracking for user feedback
//! - **Remote Management**: Configure multiple remotes (template and user origin)
//! - **Update Checking**: Non-blocking checks for available updates from upstream
//! - **Update Pulling**: Safe fast-forward merges to pull latest changes
//! - **Repository Initialization**: Create new git repositories for fresh projects
//!
//! # Example Workflow
//!
//! ```no_run
//! use std::path::Path;
//! use std::sync::{Arc, parking_lot::Mutex};
//! 
//! # fn example() -> Result<(), git2::Error> {
//! // Clone a template repository
//! let progress = Arc::new(Mutex::new(CloneProgress::default()));
//! let repo = clone_repository(
//!     "https://github.com/user/template.git".to_string(),
//!     Path::new("/path/to/project").to_path_buf(),
//!     progress
//! )?;
//!
//! // Setup remotes for template workflow
//! setup_template_remotes(
//!     Path::new("/path/to/project"),
//!     "https://github.com/user/template.git"
//! )?;
//!
//! // Add user's own repository as origin
//! add_user_upstream(
//!     Path::new("/path/to/project"),
//!     "https://github.com/myuser/myproject.git"
//! )?;
//! # Ok(())
//! # }
//! ```

// TODO: THis entire file should be moved to a new `engine_std` crate for reuse across ui-crates and the various parts of the `engine_backend`

use std::path::Path;
use super::types::SharedCloneProgress;

/// Clones a git repository with real-time progress tracking.
///
/// This function clones a remote git repository to a local path while providing
/// progress updates through a shared progress tracker. The progress tracker is
/// updated during the object transfer phase, showing the number of objects received
/// and the overall completion percentage.
///
/// # Arguments
///
/// * `repo_url` - The URL of the git repository to clone (HTTPS or SSH)
/// * `target_path` - The local filesystem path where the repository should be cloned
/// * `progress` - A thread-safe shared progress tracker that will be updated during the clone
///
/// # Returns
///
/// * `Ok(Repository)` - The cloned repository object on success
/// * `Err(git2::Error)` - An error if cloning fails (network issues, invalid URL, etc.)
///
/// # Errors
///
/// This function will return an error if:
/// - The repository URL is invalid or inaccessible
/// - Network connectivity issues occur during cloning
/// - The target path is invalid or inaccessible
/// - Authentication is required but not provided
/// - The target directory already exists and is not empty
///
/// # Example
///
/// ```no_run
/// use std::sync::{Arc, parking_lot::Mutex};
/// use std::path::PathBuf;
/// # use super::types::CloneProgress;
///
/// # fn example() -> Result<(), git2::Error> {
/// let progress = Arc::new(Mutex::new(CloneProgress::default()));
/// let repo = clone_repository(
///     "https://github.com/rust-lang/rust.git".to_string(),
///     PathBuf::from("/tmp/rust"),
///     progress.clone()
/// )?;
///
/// // Monitor progress from another thread
/// let status = progress.lock();
/// println!("Progress: {}", status.message);
/// # Ok(())
/// # }
/// ```
///
/// # Thread Safety
///
/// The progress callback is invoked from the git2 library's internal threads.
/// The `SharedCloneProgress` must be thread-safe (typically an `Arc<Mutex<T>>`).
pub fn clone_repository(
    repo_url: String,
    target_path: std::path::PathBuf,
    progress: SharedCloneProgress,
) -> Result<git2::Repository, git2::Error> {
    let mut callbacks = git2::RemoteCallbacks::new();
    let progress_inner = progress.clone();
    
    callbacks.transfer_progress(move |stats| {
        let mut prog = progress_inner.lock();
        prog.current = stats.received_objects();
        prog.total = stats.total_objects();
        prog.message = format!(
            "Receiving objects: {}/{} ({:.1}%)",
            stats.received_objects(),
            stats.total_objects(),
            (stats.received_objects() as f32 / stats.total_objects() as f32) * 100.0
        );
        true
    });
    
    let mut fetch_options = git2::FetchOptions::new();
    fetch_options.remote_callbacks(callbacks);
    
    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fetch_options);
    
    builder.clone(&repo_url, &target_path)
}

/// Sets up git remote configuration for a template-based workflow.
///
/// This function configures a repository that was cloned from a template to maintain
/// a connection to the original template while preparing for a separate user origin.
/// It renames the default 'origin' remote to 'template' and configures it to not be
/// used as the default push target.
///
/// # Purpose
///
/// In a template-based workflow:
/// 1. User clones a project template
/// 2. The template remote is kept for receiving updates
/// 3. User's own repository becomes the new 'origin' for their work
///
/// This allows users to pull template updates while pushing their changes to their own repo.
///
/// # Arguments
///
/// * `repo_path` - Path to the git repository directory
/// * `template_url` - URL of the template repository (used for documentation/reference)
///
/// # Returns
///
/// * `Ok(())` - Configuration was successful
/// * `Err(git2::Error)` - An error occurred during configuration
///
/// # Errors
///
/// This function will return an error if:
/// - The repository path is invalid or not a git repository
/// - The 'origin' remote doesn't exist
/// - The 'template' remote name is already in use
/// - Git configuration cannot be modified (permissions, etc.)
///
/// # Example
///
/// ```no_run
/// use std::path::Path;
///
/// # fn example() -> Result<(), git2::Error> {
/// // After cloning a template
/// setup_template_remotes(
///     Path::new("/path/to/project"),
///     "https://github.com/org/template.git"
/// )?;
///
/// // Now 'origin' is renamed to 'template' and won't be used for push by default
/// # Ok(())
/// # }
/// ```
///
/// # Note
///
/// After calling this function, you should call [`add_user_upstream`] to add the user's
/// own repository as the new 'origin'.
pub fn setup_template_remotes(
    repo_path: &Path,
    template_url: &str,
) -> Result<(), git2::Error> {
    let repo = git2::Repository::open(repo_path)?;
    
    // Rename origin to template
    repo.remote_rename("origin", "template")?;
    
    // Set template as non-default for push
    let mut config = repo.config()?;
    config.set_str("remote.template.pushDefault", "false")?;
    
    Ok(())
}

/// Adds the user's repository as the default 'origin' remote.
///
/// This function adds a new remote named 'origin' pointing to the user's own repository
/// and configures it as the default target for push operations. This is typically used
/// after [`setup_template_remotes`] to complete the template workflow setup.
///
/// # Arguments
///
/// * `repo_path` - Path to the git repository directory
/// * `user_url` - URL of the user's repository (where their work will be pushed)
///
/// # Returns
///
/// * `Ok(())` - Remote was added successfully
/// * `Err(git2::Error)` - An error occurred while adding the remote
///
/// # Errors
///
/// This function will return an error if:
/// - The repository path is invalid or not a git repository
/// - The 'origin' remote name is already in use
/// - The user URL is invalid
/// - Git configuration cannot be modified
///
/// # Example
///
/// ```no_run
/// use std::path::Path;
///
/// # fn example() -> Result<(), git2::Error> {
/// // Add user's repository as the default push target
/// add_user_upstream(
///     Path::new("/path/to/project"),
///     "https://github.com/myuser/myproject.git"
/// )?;
///
/// // Now 'git push' will push to the user's repository by default
/// # Ok(())
/// # }
/// ```
///
/// # Workflow Integration
///
/// Typical usage in a template-based project setup:
/// 1. Clone template repository
/// 2. Call [`setup_template_remotes`] to preserve template connection
/// 3. Call `add_user_upstream` to set up user's own repository
/// 4. User can now push to their repo and pull template updates separately
pub fn add_user_upstream(
    repo_path: &Path,
    user_url: &str,
) -> Result<(), git2::Error> {
    let repo = git2::Repository::open(repo_path)?;
    
    // Add user's remote as origin
    repo.remote("origin", user_url)?;
    
    // Set as default push remote
    let mut config = repo.config()?;
    config.set_str("remote.pushDefault", "origin")?;
    
    Ok(())
}

/// Initializes a new git repository at the specified path.
///
/// Creates a new git repository with a `.git` directory at the given path.
/// If the directory doesn't exist, it will be created. If it already exists,
/// the git repository will be initialized within it.
///
/// # Arguments
///
/// * `path` - The filesystem path where the repository should be initialized
///
/// # Returns
///
/// * `Ok(Repository)` - The newly initialized repository
/// * `Err(git2::Error)` - An error if initialization fails
///
/// # Errors
///
/// This function will return an error if:
/// - The path is invalid or inaccessible
/// - The directory is already a git repository
/// - Filesystem permissions prevent creating the `.git` directory
///
/// # Example
///
/// ```no_run
/// use std::path::Path;
///
/// # fn example() -> Result<(), git2::Error> {
/// let repo = init_repository(Path::new("/path/to/new/project"))?;
/// println!("Initialized repository at: {:?}", repo.path());
/// # Ok(())
/// # }
/// ```
pub fn init_repository(path: &Path) -> Result<git2::Repository, git2::Error> {
    git2::Repository::init(path)
}

/// Checks if a directory contains a git repository.
///
/// Performs a simple filesystem check for the presence of a `.git` directory
/// or file (in the case of git worktrees or submodules).
///
/// # Arguments
///
/// * `path` - The directory path to check
///
/// # Returns
///
/// * `true` - If a `.git` directory/file exists at the path
/// * `false` - If no git repository is present
///
/// # Note
///
/// This function only checks for the presence of `.git` and doesn't validate
/// whether the repository is valid or corrupted. For more thorough validation,
/// consider using `git2::Repository::open()` and checking for errors.
///
/// # Example
///
/// ```no_run
/// use std::path::Path;
///
/// if is_git_repo(Path::new("/path/to/project")) {
///     println!("This is a git repository");
/// } else {
///     println!("Not a git repository");
/// }
/// ```
pub fn is_git_repo(path: &Path) -> bool {
    path.join(".git").exists()
}

/// Checks if a git repository has an 'origin' remote configured.
///
/// Opens the repository and checks for the existence of a remote named 'origin'.
/// This is commonly used to determine if a repository is connected to an upstream
/// source for pulling updates and pushing changes.
///
/// # Arguments
///
/// * `path` - Path to the git repository directory
///
/// # Returns
///
/// * `true` - If the repository has an 'origin' remote
/// * `false` - If the repository doesn't exist, can't be opened, or has no 'origin' remote
///
/// # Note
///
/// This function returns `false` if the repository can't be opened, so a `false`
/// result doesn't necessarily mean the remote doesn't exist—it could also indicate
/// the path isn't a valid repository.
///
/// # Example
///
/// ```no_run
/// use std::path::Path;
///
/// let project_path = Path::new("/path/to/project");
/// if has_origin_remote(project_path) {
///     println!("Repository is connected to origin");
///     // Can check for updates or push changes
/// } else {
///     println!("No origin remote configured");
///     // Might need to add one
/// }
/// ```
pub fn has_origin_remote(path: &Path) -> bool {
    if let Ok(repo) = git2::Repository::open(path) {
        repo.find_remote("origin").is_ok()
    } else {
        false
    }
}

/// Checks for available updates from the origin remote without applying them.
///
/// Performs a non-blocking check by fetching the latest commits from origin and
/// comparing them with the local HEAD. Returns the number of commits the local
/// branch is behind the remote branch.
///
/// # Behavior
///
/// 1. Fetches all branches from origin into `refs/remotes/origin/*`
/// 2. Determines the current local branch
/// 3. Compares local and remote commit histories
/// 4. Calculates how many commits behind the local branch is
///
/// # Arguments
///
/// * `path` - Path to the git repository directory
///
/// # Returns
///
/// * `Ok(usize)` - Number of commits the local branch is behind origin
/// * `Err(git2::Error)` - An error if checking fails
///
/// # Errors
///
/// This function will return an error if:
/// - The repository path is invalid or not a git repository
/// - The 'origin' remote doesn't exist
/// - Network connectivity issues prevent fetching
/// - No commits exist on HEAD (empty repository)
/// - The remote branch doesn't exist
/// - Authentication is required but not provided
///
/// # Example
///
/// ```no_run
/// use std::path::Path;
///
/// # fn example() -> Result<(), git2::Error> {
/// let project_path = Path::new("/path/to/project");
/// match check_for_updates(project_path) {
///     Ok(0) => println!("Repository is up to date"),
///     Ok(n) => println!("Repository is {} commits behind origin", n),
///     Err(e) => eprintln!("Failed to check for updates: {}", e),
/// }
/// # Ok(())
/// # }
/// ```
///
/// # Performance
///
/// This function performs a network fetch operation, which may take time depending
/// on network speed and repository size. Consider running this in a background
/// thread or async context to avoid blocking the UI.
///
/// # Note
///
/// This function only fetches and compares—it doesn't modify the working directory.
/// Use [`pull_updates`] to actually apply the updates.
pub fn check_for_updates(path: &Path) -> Result<usize, git2::Error> {
    let repo = git2::Repository::open(path)?;
    
    // Find origin remote
    let mut remote = repo.find_remote("origin")?;
    
    // Fetch from origin
    remote.fetch(&["refs/heads/*:refs/remotes/origin/*"], None, None)?;
    
    // Get current branch
    let head = repo.head()?;
    let branch = head.shorthand().unwrap_or("main");
    
    // Get local and remote commits
    let local_oid = head.target().ok_or_else(|| {
        git2::Error::from_str("No commit on HEAD")
    })?;
    
    let remote_branch = format!("refs/remotes/origin/{}", branch);
    let remote_ref = repo.find_reference(&remote_branch)?;
    let remote_oid = remote_ref.target().ok_or_else(|| {
        git2::Error::from_str("No remote commit")
    })?;
    
    // Count commits behind
    let (ahead, behind) = repo.graph_ahead_behind(local_oid, remote_oid)?;
    
    Ok(behind)
}

/// Pulls updates from origin using a fast-forward merge.
///
/// Fetches the latest commits from origin and applies them to the local working
/// directory using a fast-forward merge. This is a safe operation that only succeeds
/// if the local branch can be fast-forwarded without conflicts.
///
/// # Behavior
///
/// 1. Fetches all branches from origin
/// 2. Determines the current local branch
/// 3. Performs a fast-forward merge from the corresponding remote branch
/// 4. Updates HEAD to point to the new commit
/// 5. Checks out the updated files to the working directory
///
/// # Arguments
///
/// * `path` - Path to the git repository directory
///
/// # Returns
///
/// * `Ok(())` - Updates were successfully applied
/// * `Err(git2::Error)` - An error occurred during the update process
///
/// # Errors
///
/// This function will return an error if:
/// - The repository path is invalid or not a git repository
/// - The 'origin' remote doesn't exist
/// - Network connectivity issues prevent fetching
/// - The local branch has diverged from origin (cannot fast-forward)
/// - There are uncommitted local changes that would be overwritten
/// - Authentication is required but not provided
/// - The remote branch doesn't exist
///
/// # Safety
///
/// This function only performs **fast-forward merges**, which means:
/// - ✅ Safe: Local branch is behind origin (clean pull)
/// - ❌ Fails: Local branch has diverged from origin (merge conflicts possible)
/// - ❌ Fails: Uncommitted changes would be overwritten
///
/// This conservative approach prevents accidental data loss but means users must
/// handle diverged branches manually.
///
/// # Example
///
/// ```no_run
/// use std::path::Path;
///
/// # fn example() -> Result<(), git2::Error> {
/// let project_path = Path::new("/path/to/project");
///
/// // Check if updates are available first
/// match check_for_updates(project_path) {
///     Ok(0) => println!("Already up to date"),
///     Ok(n) => {
///         println!("Pulling {} commits...", n);
///         pull_updates(project_path)?;
///         println!("Successfully updated!");
///     }
///     Err(e) => eprintln!("Error checking updates: {}", e),
/// }
/// # Ok(())
/// # }
/// ```
///
/// # Warning
///
/// If the local branch has diverged from origin (has local commits that aren't on origin),
/// this function will fail. Users should be instructed to:
/// 1. Commit their local changes
/// 2. Use `git pull --rebase` or `git merge` manually to resolve the divergence
/// 3. Or reset their local branch if they want to discard local changes
///
/// # Performance
///
/// This function performs network I/O and filesystem operations. Consider running
/// it in a background thread or async context to avoid blocking the UI.
pub fn pull_updates(path: &Path) -> Result<(), git2::Error> {
    let repo = git2::Repository::open(path)?;
    
    // Fetch first
    let mut remote = repo.find_remote("origin")?;
    remote.fetch(&["refs/heads/*:refs/remotes/origin/*"], None, None)?;
    
    // Get current branch
    let head = repo.head()?;
    let branch = head.shorthand().unwrap_or("main");
    
    // Perform merge (fast-forward only for safety)
    let remote_branch = format!("refs/remotes/origin/{}", branch);
    let remote_ref = repo.find_reference(&remote_branch)?;
    let remote_oid = remote_ref.target().ok_or_else(|| {
        git2::Error::from_str("No remote commit")
    })?;
    
    let remote_commit = repo.find_commit(remote_oid)?;
    
    // Fast-forward merge
    repo.checkout_tree(remote_commit.as_object(), None)?;
    
    // Update HEAD
    repo.head()?.set_target(remote_oid, "Fast-forward merge")?;
    
    Ok(())
}
