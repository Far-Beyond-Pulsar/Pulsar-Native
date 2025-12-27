use std::path::Path;
use super::types::SharedCloneProgress;

/// Clone a git repository with progress tracking
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

/// Setup git upstream configuration after cloning template
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

/// Add user's upstream as default origin
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

/// Initialize a new git repository
pub fn init_repository(path: &Path) -> Result<git2::Repository, git2::Error> {
    git2::Repository::init(path)
}

/// Check if a directory is a git repository
pub fn is_git_repo(path: &Path) -> bool {
    path.join(".git").exists()
}

/// Check if repository has an origin remote
pub fn has_origin_remote(path: &Path) -> bool {
    if let Ok(repo) = git2::Repository::open(path) {
        repo.find_remote("origin").is_ok()
    } else {
        false
    }
}

/// Fetch updates from origin (non-blocking check)
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

/// Pull updates from origin
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
