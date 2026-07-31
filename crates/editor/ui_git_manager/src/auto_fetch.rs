use crate::git_operations::{fetch_from_remote_unlocked, open_repo};
use engine_fs::{FsProvider as _, LocalFsProvider};
use git2::{BranchType, ErrorCode};
use gpui::{AnyWindowHandle, AppContext as _, Context, Task};
use std::{
    collections::HashSet,
    future::Future,
    path::{Path, PathBuf},
    sync::{Condvar, LazyLock, Mutex},
    time::Duration,
};
use ui::{ContextModal as _, notification::Notification};

const AUTO_FETCH_DEFAULT_INTERVAL_MINUTES: i64 = 5;
const AUTO_FETCH_MIN_INTERVAL_MINUTES: i64 = 1;
const AUTO_FETCH_MAX_INTERVAL_MINUTES: i64 = 60;
const SOURCE_CONTROL_OWNER: &str = "source_control";
const AUTO_FETCH_KEY: &str = "auto_fetch";
const AUTO_FETCH_INTERVAL_KEY: &str = "auto_fetch_interval_minutes";

struct RemoteOperationState {
    active: Mutex<HashSet<PathBuf>>,
    available: Condvar,
}

static REMOTE_OPERATIONS: LazyLock<RemoteOperationState> = LazyLock::new(|| RemoteOperationState {
    active: Mutex::new(HashSet::new()),
    available: Condvar::new(),
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoFetchSettings {
    pub enabled: bool,
    pub interval_minutes: u64,
}

impl AutoFetchSettings {
    fn interval(self) -> Duration {
        Duration::from_secs(self.interval_minutes * 60)
    }
}

impl Default for AutoFetchSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_minutes: AUTO_FETCH_DEFAULT_INTERVAL_MINUTES as u64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoFetchWaitOutcome {
    IntervalElapsed,
    SettingsChanged,
}

/// Wakes an auto-fetch timer when either relevant source-control setting changes.
///
/// Each timer owns a watcher so project windows and the launcher are notified
/// independently. The bounded channel coalesces back-to-back writes made while
/// saving both settings.
pub struct AutoFetchSettingsWatcher {
    changes: smol::channel::Receiver<()>,
    _listener: engine_state::ListenerId,
}

impl AutoFetchSettingsWatcher {
    pub fn new() -> Self {
        Self::with_config(engine_state::global_config())
    }

    fn with_config(config: &engine_state::ConfigManager) -> Self {
        let (sender, changes) = smol::channel::bounded(1);
        let listener = config.on_any_change(move |event| {
            if event.namespace == engine_state::NS_EDITOR
                && event.owner_path() == SOURCE_CONTROL_OWNER
                && matches!(event.key.as_str(), AUTO_FETCH_KEY | AUTO_FETCH_INTERVAL_KEY)
            {
                let _ = sender.try_send(());
            }
        });

        Self {
            changes,
            _listener: listener,
        }
    }

    pub async fn wait<F>(&self, timer: F) -> AutoFetchWaitOutcome
    where
        F: Future<Output = ()>,
    {
        let interval_elapsed = async move {
            timer.await;
            AutoFetchWaitOutcome::IntervalElapsed
        };
        let settings_changed = async {
            let _ = self.changes.recv().await;
            AutoFetchWaitOutcome::SettingsChanged
        };

        smol::future::FutureExt::race(interval_elapsed, settings_changed).await
    }
}

impl Default for AutoFetchSettingsWatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackingSnapshot {
    pub branch: String,
    pub upstream_oid: Option<String>,
    pub behind: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoFetchOutcome {
    Busy,
    Fetched(TrackingSnapshot),
}

/// Process-wide guard for operations that depend on a repository's remote or HEAD.
///
/// The key is the canonical Git directory (or common Git directory for a linked
/// worktree), so callers using a repository root, subdirectory, or symlink all
/// coordinate through the same entry.
#[derive(Debug)]
pub(crate) struct RemoteOperationGuard {
    repository_path: PathBuf,
}

impl RemoteOperationGuard {
    /// Try to enter the repository's remote-operation critical section.
    ///
    /// `Ok(None)` means another remote operation is already active. This is the
    /// auto-fetch path: skip the tick rather than queueing behind user work.
    pub(crate) fn try_acquire(repo_path: &Path) -> Result<Option<Self>, git2::Error> {
        let repository_path = canonical_repository_path(repo_path)?;
        let mut active = REMOTE_OPERATIONS
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !active.insert(repository_path.clone()) {
            return Ok(None);
        }

        Ok(Some(Self { repository_path }))
    }

    /// Wait until the repository is idle, then enter the critical section.
    ///
    /// User-initiated operations use this path so a periodic fetch can never
    /// make a fetch, push, pull, or branch switch fail with a transient busy
    /// error.
    pub(crate) fn acquire(repo_path: &Path) -> Result<Self, git2::Error> {
        let repository_path = canonical_repository_path(repo_path)?;
        let mut active = REMOTE_OPERATIONS
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        while active.contains(&repository_path) {
            active = REMOTE_OPERATIONS
                .available
                .wait(active)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        active.insert(repository_path.clone());

        Ok(Self { repository_path })
    }
}

impl Drop for RemoteOperationGuard {
    fn drop(&mut self) {
        let mut active = REMOTE_OPERATIONS
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        active.remove(&self.repository_path);
        drop(active);
        REMOTE_OPERATIONS.available.notify_all();
    }
}

/// Resolve a stable identity for a repository without falling back to the
/// process working directory.
pub fn canonical_repository_path(repo_path: &Path) -> Result<PathBuf, git2::Error> {
    let repo = open_repo(repo_path)?;
    let common_dir = repo.commondir();

    LocalFsProvider::new()
        .canonicalize(common_dir)
        .map_err(|error| {
            git2::Error::from_str(&format!(
                "failed to canonicalize repository path {}: {error}",
                common_dir.display()
            ))
        })
}

pub fn read_auto_fetch_settings() -> AutoFetchSettings {
    let config = engine_state::global_config();
    let enabled = config
        .get(
            engine_state::NS_EDITOR,
            SOURCE_CONTROL_OWNER,
            AUTO_FETCH_KEY,
        )
        .ok()
        .and_then(|value| value.as_bool().ok())
        .unwrap_or(true);
    let interval_minutes = config
        .get(
            engine_state::NS_EDITOR,
            SOURCE_CONTROL_OWNER,
            AUTO_FETCH_INTERVAL_KEY,
        )
        .ok()
        .and_then(|value| value.as_int().ok())
        .unwrap_or(AUTO_FETCH_DEFAULT_INTERVAL_MINUTES)
        .clamp(
            AUTO_FETCH_MIN_INTERVAL_MINUTES,
            AUTO_FETCH_MAX_INTERVAL_MINUTES,
        ) as u64;

    AutoFetchSettings {
        enabled,
        interval_minutes,
    }
}

/// Read the current branch's tracking state without contacting the remote.
pub(crate) fn load_tracking_snapshot(repo_path: &Path) -> Result<TrackingSnapshot, git2::Error> {
    let repo = open_repo(repo_path)?;
    let head = match repo.head() {
        Ok(head) => head,
        Err(error) if matches!(error.code(), ErrorCode::UnbornBranch | ErrorCode::NotFound) => {
            return Ok(TrackingSnapshot {
                branch: "HEAD".to_string(),
                upstream_oid: None,
                behind: 0,
            });
        }
        Err(error) => return Err(error),
    };

    let branch = head.shorthand().unwrap_or("HEAD").to_string();
    if !head.is_branch() {
        return Ok(TrackingSnapshot {
            branch,
            upstream_oid: None,
            behind: 0,
        });
    }

    let local_oid = head
        .target()
        .ok_or_else(|| git2::Error::from_str("current branch has no target"))?;
    let local_branch = repo.find_branch(&branch, BranchType::Local)?;
    let upstream = match local_branch.upstream() {
        Ok(upstream) => upstream,
        Err(error) if error.code() == ErrorCode::NotFound => {
            return Ok(TrackingSnapshot {
                branch,
                upstream_oid: None,
                behind: 0,
            });
        }
        Err(error) => return Err(error),
    };
    let upstream_oid = upstream
        .get()
        .target()
        .ok_or_else(|| git2::Error::from_str("upstream branch has no target"))?;
    let (_, behind) = repo.graph_ahead_behind(local_oid, upstream_oid)?;

    Ok(TrackingSnapshot {
        branch,
        upstream_oid: Some(upstream_oid.to_string()),
        behind,
    })
}

/// A notification is warranted only when a known tracking ref moves on the
/// same branch and leaves commits for the user to integrate.
fn should_notify_remote_change(
    previous: Option<&TrackingSnapshot>,
    current: &TrackingSnapshot,
) -> bool {
    let Some(previous) = previous else {
        return false;
    };

    previous.branch == current.branch
        && current.behind > 0
        && matches!(
            (&previous.upstream_oid, &current.upstream_oid),
            (Some(previous_oid), Some(current_oid)) if previous_oid != current_oid
        )
}

/// Fetch and return the resulting tracking snapshot.
///
/// This function is blocking and must run on GPUI's background executor. Busy
/// repositories are reported separately so periodic work can be skipped without
/// surfacing an error to the user.
pub fn fetch_tracking_snapshot(repo_path: &Path) -> Result<AutoFetchOutcome, git2::Error> {
    let Some(_guard) = RemoteOperationGuard::try_acquire(repo_path)? else {
        return Ok(AutoFetchOutcome::Busy);
    };

    fetch_from_remote_unlocked(repo_path, None)?;
    load_tracking_snapshot(repo_path).map(AutoFetchOutcome::Fetched)
}

struct GitAutoFetchNotification;

/// Start project-scoped automatic fetches. The caller must retain the returned
/// task for the project/window lifetime; dropping it cancels future ticks.
pub fn start_auto_fetch_task<T: 'static>(
    project_path: PathBuf,
    window_handle: AnyWindowHandle,
    cx: &Context<T>,
) -> Task<()> {
    cx.spawn(async move |owner, cx| {
        let settings_watcher = AutoFetchSettingsWatcher::new();
        let initial_path = project_path.clone();
        let mut previous_snapshot = match cx
            .background_executor()
            .spawn(async move { load_tracking_snapshot(&initial_path) })
            .await
        {
            Ok(snapshot) => Some(snapshot),
            Err(error) if error.code() == ErrorCode::NotFound => return,
            Err(error) => {
                tracing::warn!(
                    project_path = %project_path.display(),
                    %error,
                    "Failed to initialize Git auto-fetch tracking state"
                );
                None
            }
        };

        loop {
            if owner.upgrade().is_none() {
                break;
            }

            let scheduled_settings = read_auto_fetch_settings();
            if settings_watcher
                .wait(
                    cx.background_executor()
                        .timer(scheduled_settings.interval()),
                )
                .await
                == AutoFetchWaitOutcome::SettingsChanged
            {
                continue;
            }

            if owner.upgrade().is_none() {
                break;
            }

            // A changed setting starts a fresh full interval on the next loop.
            // This also prevents a fetch scheduled while enabled from running
            // after the user disables it.
            let current_settings = read_auto_fetch_settings();
            if current_settings != scheduled_settings || !current_settings.enabled {
                continue;
            }

            let fetch_path = project_path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { fetch_tracking_snapshot(&fetch_path) })
                .await;

            match result {
                Ok(AutoFetchOutcome::Busy) => {}
                Ok(AutoFetchOutcome::Fetched(snapshot)) => {
                    let notify = should_notify_remote_change(previous_snapshot.as_ref(), &snapshot);
                    previous_snapshot = Some(snapshot.clone());

                    if notify {
                        let plural = if snapshot.behind == 1 { "" } else { "s" };
                        let message = format!(
                            "{} remote commit{} available on {}",
                            snapshot.behind, plural, snapshot.branch
                        );
                        if cx
                            .update_window(window_handle, |_, window, cx| {
                                window.push_notification(
                                    Notification::info(message)
                                        .id::<GitAutoFetchNotification>()
                                        .title("Git changes available"),
                                    cx,
                                );
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
                Err(error) => tracing::warn!(
                    project_path = %project_path.display(),
                    %error,
                    "Git auto-fetch failed"
                ),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_operations::{
        fetch_from_remote, pull_from_remote, pull_from_remote_with_after_fetch,
    };
    use git2::{Oid, Repository, RepositoryInitOptions, Signature};
    use std::{cell::Cell, fs, sync::mpsc, thread, time::Duration};

    fn snapshot(branch: &str, upstream_oid: Option<&str>, behind: usize) -> TrackingSnapshot {
        TrackingSnapshot {
            branch: branch.to_string(),
            upstream_oid: upstream_oid.map(str::to_string),
            behind,
        }
    }

    fn auto_fetch_test_config() -> (engine_state::ConfigManager, engine_state::OwnerHandle) {
        let config = engine_state::ConfigManager::new();
        let schema = engine_state::NamespaceSchema::new("Source Control", "Test settings")
            .setting(
                AUTO_FETCH_KEY,
                engine_state::SchemaEntry::new("Auto fetch", true),
            )
            .setting(
                AUTO_FETCH_INTERVAL_KEY,
                engine_state::SchemaEntry::new("Auto fetch interval", 5_i64),
            )
            .setting(
                "unrelated",
                engine_state::SchemaEntry::new("Unrelated setting", false),
            );
        let handle = config
            .register(engine_state::NS_EDITOR, SOURCE_CONTROL_OWNER, schema)
            .expect("register source-control settings");
        (config, handle)
    }

    #[test]
    fn settings_watcher_interrupts_timer_only_for_auto_fetch_changes() {
        let (config, handle) = auto_fetch_test_config();
        let watcher = AutoFetchSettingsWatcher::with_config(&config);

        handle
            .set("unrelated", engine_state::ConfigValue::Bool(true))
            .expect("update unrelated setting");
        assert!(
            matches!(
                watcher.changes.try_recv(),
                Err(smol::channel::TryRecvError::Empty)
            ),
            "unrelated source-control settings must not restart auto-fetch timers"
        );

        let changed_outcome = smol::block_on(async {
            let (started_tx, started_rx) = smol::channel::bounded(1);
            let wait = async {
                started_tx.send(()).await.expect("signal timer wait");
                watcher.wait(std::future::pending()).await
            };
            let change_interval = async {
                started_rx.recv().await.expect("wait for timer to start");
                handle
                    .set(AUTO_FETCH_INTERVAL_KEY, engine_state::ConfigValue::Int(1))
                    .expect("update auto-fetch interval");
            };
            let interrupted = async {
                let (outcome, ()) = smol::future::zip(wait, change_interval).await;
                Some(outcome)
            };
            let timeout = async {
                smol::Timer::after(Duration::from_secs(1)).await;
                None
            };

            smol::future::FutureExt::race(interrupted, timeout).await
        });
        assert_eq!(changed_outcome, Some(AutoFetchWaitOutcome::SettingsChanged));
    }

    fn init_repository(path: &Path, bare: bool) -> Repository {
        let mut options = RepositoryInitOptions::new();
        options.bare(bare).initial_head("main");
        Repository::init_opts(path, &options).expect("initialize repository")
    }

    fn commit_file(repo: &Repository, contents: &str, message: &str) -> Oid {
        let workdir = repo.workdir().expect("non-bare repository");
        fs::write(workdir.join("tracked.txt"), contents).expect("write tracked file");

        let mut index = repo.index().expect("open index");
        index
            .add_path(Path::new("tracked.txt"))
            .expect("add tracked file");
        index.write().expect("write index");
        let tree_oid = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_oid).expect("find tree");
        let signature =
            Signature::now("Pulsar Test", "pulsar@example.com").expect("create signature");

        match repo.head().and_then(|head| head.peel_to_commit()) {
            Ok(parent) => repo
                .commit(
                    Some("HEAD"),
                    &signature,
                    &signature,
                    message,
                    &tree,
                    &[&parent],
                )
                .expect("commit with parent"),
            Err(_) => repo
                .commit(Some("HEAD"), &signature, &signature, message, &tree, &[])
                .expect("initial commit"),
        }
    }

    fn push_main(repo: &Repository, remote_path: &Path) {
        if repo.find_remote("origin").is_err() {
            repo.remote("origin", &remote_path.to_string_lossy())
                .expect("create origin remote");
        }
        repo.find_remote("origin")
            .expect("find origin")
            .push(&["refs/heads/main:refs/heads/main"], None)
            .expect("push main");
    }

    #[test]
    fn notifies_only_for_known_changes_on_the_same_branch() {
        let before = snapshot("main", Some("old"), 0);

        assert!(!should_notify_remote_change(None, &before));
        assert!(!should_notify_remote_change(
            Some(&snapshot("other", Some("old"), 0)),
            &snapshot("main", Some("new"), 1),
        ));
        assert!(!should_notify_remote_change(
            Some(&before),
            &snapshot("main", Some("old"), 1),
        ));
        assert!(!should_notify_remote_change(
            Some(&before),
            &snapshot("main", Some("new"), 0),
        ));
        assert!(!should_notify_remote_change(
            Some(&snapshot("main", None, 0)),
            &snapshot("main", Some("new"), 1),
        ));
        assert!(should_notify_remote_change(
            Some(&before),
            &snapshot("main", Some("new"), 1),
        ));
    }

    #[test]
    fn automatic_gate_skips_while_manual_gate_waits() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let repo_path = temp.path().join("repo");
        let remote_path = temp.path().join("remote.git");
        init_repository(&repo_path, false);
        init_repository(&remote_path, true);
        Repository::open(&repo_path)
            .expect("open repository")
            .remote("origin", &remote_path.to_string_lossy())
            .expect("create origin remote");
        let nested_path = repo_path.join("nested");
        fs::create_dir(&nested_path).expect("create nested directory");

        let guard = RemoteOperationGuard::acquire(&repo_path).expect("acquire first guard");
        let competing_path = nested_path.clone();
        let busy = thread::spawn(move || {
            RemoteOperationGuard::try_acquire(&competing_path)
                .expect("try competing guard")
                .is_none()
        })
        .join()
        .expect("join competing thread");

        assert!(busy);

        let (result_tx, result_rx) = mpsc::channel();
        let waiting_path = nested_path.clone();
        let waiter = thread::spawn(move || {
            result_tx
                .send(fetch_from_remote(&waiting_path, None))
                .expect("send manual fetch result");
        });
        assert!(
            result_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "manual fetch must wait while the repository gate is held"
        );

        drop(guard);
        result_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("manual fetch should resume after guard release")
            .expect("manual fetch should succeed");
        waiter.join().expect("join manual fetch thread");

        assert!(
            RemoteOperationGuard::try_acquire(&nested_path)
                .expect("reacquire guard")
                .is_some()
        );
    }

    #[test]
    fn non_repository_is_reported_as_not_found() {
        let temp = tempfile::tempdir().expect("create temp directory");

        assert_eq!(
            load_tracking_snapshot(temp.path())
                .expect_err("empty directory should not be a repository")
                .code(),
            ErrorCode::NotFound,
        );
    }

    #[test]
    fn fetch_from_subdirectory_returns_changed_tracking_snapshot() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let remote_path = temp.path().join("remote.git");
        let writer_path = temp.path().join("writer");
        let clone_path = temp.path().join("clone");

        init_repository(&remote_path, true);
        let writer = init_repository(&writer_path, false);
        commit_file(&writer, "first\n", "first");
        push_main(&writer, &remote_path);

        let clone = Repository::clone(&remote_path.to_string_lossy(), &clone_path)
            .expect("clone local remote");
        drop(clone);
        let nested_path = clone_path.join("nested");
        fs::create_dir(&nested_path).expect("create nested directory");
        let before = load_tracking_snapshot(&clone_path).expect("load initial snapshot");
        assert_eq!(before.behind, 0);

        commit_file(&writer, "second\n", "second");
        push_main(&writer, &remote_path);

        let after = match fetch_tracking_snapshot(&nested_path).expect("fetch local remote") {
            AutoFetchOutcome::Busy => panic!("repository should not be busy"),
            AutoFetchOutcome::Fetched(snapshot) => snapshot,
        };

        assert_eq!(after.branch, "main");
        assert_eq!(after.behind, 1);
        assert_ne!(before.upstream_oid, after.upstream_oid);
        assert!(should_notify_remote_change(Some(&before), &after));

        let unchanged = match fetch_tracking_snapshot(&nested_path).expect("fetch unchanged remote")
        {
            AutoFetchOutcome::Busy => panic!("repository should not be busy"),
            AutoFetchOutcome::Fetched(snapshot) => snapshot,
        };
        assert!(!should_notify_remote_change(Some(&after), &unchanged));
    }

    #[test]
    fn fetch_uses_the_configured_tracking_refspec() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let remote_path = temp.path().join("remote.git");
        let writer_path = temp.path().join("writer");
        let clone_path = temp.path().join("clone");

        init_repository(&remote_path, true);
        let writer = init_repository(&writer_path, false);
        let initial_oid = commit_file(&writer, "first\n", "first");
        push_main(&writer, &remote_path);

        let clone = Repository::clone(&remote_path.to_string_lossy(), &clone_path)
            .expect("clone local remote");
        clone
            .config()
            .expect("open clone config")
            .set_str(
                "remote.origin.fetch",
                "+refs/heads/main:refs/pulsar-tracking/main",
            )
            .expect("set custom fetch refspec");
        let remote_oid = commit_file(&writer, "second\n", "second");
        push_main(&writer, &remote_path);

        fetch_from_remote(&clone_path, None).expect("fetch custom tracking ref");

        assert_eq!(
            clone
                .refname_to_id("refs/pulsar-tracking/main")
                .expect("custom tracking ref should be updated"),
            remote_oid
        );
        assert_eq!(
            clone
                .refname_to_id("refs/remotes/origin/main")
                .expect("standard tracking ref should remain present"),
            initial_oid,
            "fetch must not override the configured refspec with a wildcard"
        );
    }

    #[test]
    fn fast_forward_pull_updates_head_and_worktree() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let remote_path = temp.path().join("remote.git");
        let writer_path = temp.path().join("writer");
        let clone_path = temp.path().join("clone");

        init_repository(&remote_path, true);
        let writer = init_repository(&writer_path, false);
        commit_file(&writer, "first\n", "first");
        push_main(&writer, &remote_path);

        let clone = Repository::clone(&remote_path.to_string_lossy(), &clone_path)
            .expect("clone local remote");
        let remote_oid = commit_file(&writer, "remote\n", "remote");
        push_main(&writer, &remote_path);

        pull_from_remote(&clone_path, None).expect("fast-forward pull");

        assert_eq!(clone.head().expect("load HEAD").target(), Some(remote_oid));
        assert_eq!(
            fs::read_to_string(clone_path.join("tracked.txt"))
                .expect("read tracked file")
                .trim_end(),
            "remote"
        );
    }

    #[test]
    fn pull_rejects_a_head_change_that_happens_during_fetch() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let remote_path = temp.path().join("remote.git");
        let writer_path = temp.path().join("writer");
        let clone_path = temp.path().join("clone");

        init_repository(&remote_path, true);
        let writer = init_repository(&writer_path, false);
        commit_file(&writer, "first\n", "first");
        push_main(&writer, &remote_path);

        let clone = Repository::clone(&remote_path.to_string_lossy(), &clone_path)
            .expect("clone local remote");
        commit_file(&writer, "remote\n", "remote");
        push_main(&writer, &remote_path);

        let concurrent_head = Cell::new(None);
        let error = pull_from_remote_with_after_fetch(&clone_path, None, |repo| {
            concurrent_head.set(Some(commit_file(
                repo,
                "local during fetch\n",
                "local during fetch",
            )));
            Ok(())
        })
        .expect_err("pull must reject a changed HEAD");
        let concurrent_head = concurrent_head.get().expect("record concurrent commit");

        assert!(error.message().contains("operation target changed"));
        assert_eq!(
            clone.head().expect("load HEAD").target(),
            Some(concurrent_head),
            "pull must not replace the commit created while fetch was running"
        );
        assert_eq!(
            fs::read_to_string(clone_path.join("tracked.txt")).expect("read tracked file"),
            "local during fetch\n"
        );
    }

    #[test]
    fn diverged_pull_does_not_move_head() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let remote_path = temp.path().join("remote.git");
        let writer_path = temp.path().join("writer");
        let clone_path = temp.path().join("clone");

        init_repository(&remote_path, true);
        let writer = init_repository(&writer_path, false);
        commit_file(&writer, "first\n", "first");
        push_main(&writer, &remote_path);

        let clone = Repository::clone(&remote_path.to_string_lossy(), &clone_path)
            .expect("clone local remote");
        let local_oid = commit_file(&clone, "local\n", "local");
        commit_file(&writer, "remote\n", "remote");
        push_main(&writer, &remote_path);

        let error = pull_from_remote(&clone_path, None).expect_err("diverged pull must fail");

        assert!(error.message().contains("Merge required"));
        assert_eq!(
            clone.head().expect("load HEAD").target(),
            Some(local_oid),
            "a rejected pull must leave HEAD unchanged"
        );
        assert_eq!(
            fs::read_to_string(clone_path.join("tracked.txt")).expect("read tracked file"),
            "local\n"
        );
    }

    #[test]
    fn pull_does_not_overwrite_conflicting_worktree_changes() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let remote_path = temp.path().join("remote.git");
        let writer_path = temp.path().join("writer");
        let clone_path = temp.path().join("clone");

        init_repository(&remote_path, true);
        let writer = init_repository(&writer_path, false);
        let initial_oid = commit_file(&writer, "first\n", "first");
        push_main(&writer, &remote_path);

        let clone = Repository::clone(&remote_path.to_string_lossy(), &clone_path)
            .expect("clone local remote");
        fs::write(clone_path.join("tracked.txt"), "local dirty\n").expect("write local change");
        commit_file(&writer, "remote\n", "remote");
        push_main(&writer, &remote_path);

        pull_from_remote(&clone_path, None).expect_err("conflicting dirty pull must fail");

        assert_eq!(
            clone.head().expect("load HEAD").target(),
            Some(initial_oid),
            "a failed checkout must restore the original branch target"
        );
        assert_eq!(
            fs::read_to_string(clone_path.join("tracked.txt")).expect("read tracked file"),
            "local dirty\n"
        );
    }
}
