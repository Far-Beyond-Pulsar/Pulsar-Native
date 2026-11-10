//! Git operations using git2.
//!
//! Blocking helpers, meant for a background executor, grouped by concern:
//! [`repository`] (open/state), [`worktree`] (stage/unstage/discard),
//! [`commit`], [`branch`], and the remote stack ([`remote_context`],
//! [`credentials`], [`remote_ops`]). Everything is re-exported here so the
//! `git_operations::*` paths callers use are unchanged.

mod branch;
mod commit;
mod credentials;
mod remote_context;
mod remote_ops;
mod repository;
mod worktree;

pub use branch::*;
pub use commit::*;
pub use credentials::*;
pub use remote_context::*;
pub use remote_ops::*;
pub use repository::*;
pub use worktree::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::*;
    use engine_fs::FsProvider as _;
    use git2::{Oid, Repository, build::CheckoutBuilder};
    use git2::{RepositoryInitOptions, Signature};
    use std::path::Path;

    fn init_repository(path: &Path) -> Repository {
        let mut options = RepositoryInitOptions::new();
        options.initial_head("main");
        let repo = Repository::init_opts(path, &options).expect("initialize repository");
        std::fs::write(path.join("tracked.txt"), "initial\n").expect("write tracked file");
        let mut index = repo.index().expect("open index");
        index
            .add_path(Path::new("tracked.txt"))
            .expect("add tracked file");
        index.write().expect("write index");
        let tree_oid = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_oid).expect("find tree");
        let signature =
            Signature::now("Pulsar Test", "pulsar@example.com").expect("create signature");
        repo.commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
            .expect("create initial commit");
        drop(tree);
        repo
    }

    fn commit_file(repo: &Repository, contents: &str, message: &str) -> Oid {
        std::fs::write(
            repo.workdir()
                .expect("repository should have a worktree")
                .join("tracked.txt"),
            contents,
        )
        .expect("write tracked file");
        let mut index = repo.index().expect("open index");
        index
            .add_path(Path::new("tracked.txt"))
            .expect("add tracked file");
        index.write().expect("write index");
        let tree_oid = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_oid).expect("find tree");
        let parent = repo
            .head()
            .expect("load HEAD")
            .peel_to_commit()
            .expect("load HEAD commit");
        let signature =
            Signature::now("Pulsar Test", "pulsar@example.com").expect("create signature");
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &[&parent],
        )
        .expect("create commit")
    }

    fn prepare_pending_fast_forward(repo: &Repository) -> (Oid, Oid) {
        let local_oid = repo
            .head()
            .expect("load HEAD")
            .target()
            .expect("HEAD target");
        let upstream_oid = commit_file(repo, "upstream\n", "upstream");
        let local = repo
            .find_object(local_oid, None)
            .expect("find original commit");
        repo.reset(&local, git2::ResetType::Hard, None)
            .expect("restore original branch state");
        drop(local);

        let upstream = repo
            .find_commit(upstream_oid)
            .expect("find upstream commit");
        let mut checkout = CheckoutBuilder::new();
        checkout.safe();
        repo.checkout_tree(upstream.as_object(), Some(&mut checkout))
            .expect("prepare fast-forward worktree");

        (local_oid, upstream_oid)
    }

    #[test]
    fn credential_retry_rejects_a_changed_branch_and_remote() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let repo = init_repository(temp.path());
        repo.remote("origin", "invalid://old.example/repository")
            .expect("create original remote");
        repo.remote("other", "invalid://new.example/repository")
            .expect("create other remote");

        let head = repo
            .head()
            .expect("load HEAD")
            .peel_to_commit()
            .expect("load HEAD commit");
        repo.branch("other", &head, false)
            .expect("create other branch");
        drop(head);
        let mut config = repo.config().expect("open config");
        config
            .set_str("branch.main.remote", "origin")
            .expect("configure main remote");
        config
            .set_str("branch.main.merge", "refs/heads/main")
            .expect("configure main merge target");
        config
            .set_str("branch.other.remote", "other")
            .expect("configure other remote");
        config
            .set_str("branch.other.merge", "refs/heads/other")
            .expect("configure other merge target");
        drop(config);

        let expected = remote_operation_context(temp.path(), &repo, RemoteOperationKind::Fetch)
            .expect("capture failed operation target");
        repo.set_head("refs/heads/other")
            .expect("switch to other branch");

        let attempt = execute_remote_operation(
            temp.path(),
            RemoteOperationKind::Fetch,
            Some(("alice".to_string(), "secret".to_string())),
            Some(expected),
        );
        let error = attempt
            .result
            .expect_err("retry must reject the changed operation target");

        assert!(error.message().contains("operation target changed"));
        assert_eq!(
            attempt.context.expect("capture current target").remote_url,
            "invalid://new.example/repository"
        );
    }

    #[test]
    fn credential_retry_rejects_changed_fetch_refspecs_for_fetch_and_pull() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let repo = init_repository(temp.path());
        repo.remote("origin", "invalid://example.test/repository")
            .expect("create remote");
        let mut config = repo.config().expect("open config");
        config
            .set_str("branch.main.remote", "origin")
            .expect("configure branch remote");
        config
            .set_str("branch.main.merge", "refs/heads/main")
            .expect("configure branch merge target");
        drop(config);

        let original_refspec = "+refs/heads/main:refs/remotes/origin/main";
        let changed_refspec = "+refs/heads/release:refs/remotes/origin/release";
        for kind in [RemoteOperationKind::Fetch, RemoteOperationKind::Pull] {
            repo.config()
                .expect("open config")
                .set_str("remote.origin.fetch", original_refspec)
                .expect("configure original fetch refspec");
            let expected = remote_operation_context(temp.path(), &repo, kind)
                .expect("capture failed operation target");
            repo.config()
                .expect("open config")
                .set_str("remote.origin.fetch", changed_refspec)
                .expect("change fetch refspec");

            let attempt = execute_remote_operation(
                temp.path(),
                kind,
                Some(("alice".to_string(), "secret".to_string())),
                Some(expected),
            );
            let error = attempt
                .result
                .expect_err("retry must reject changed fetch refspecs before network use");

            assert!(error.message().contains("operation target changed"));
            assert_eq!(
                attempt
                    .context
                    .expect("capture current target")
                    .fetch_refspecs,
                Some(vec![changed_refspec.as_bytes().to_vec()])
            );
        }
    }

    #[test]
    fn pull_commit_error_restores_old_worktree_when_reference_was_not_updated() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let repo = init_repository(temp.path());
        let (local_oid, upstream_oid) = prepare_pending_fast_forward(&repo);

        let error = finish_pull_transaction(
            &repo,
            local_oid,
            upstream_oid,
            "refs/heads/main",
            Err(git2::Error::from_str(
                "simulated transaction commit failure",
            )),
        )
        .expect_err("simulated transaction failure must be reported");

        assert!(
            error
                .message()
                .contains("simulated transaction commit failure")
        );
        assert_eq!(repo.head().expect("load HEAD").target(), Some(local_oid));
        assert_eq!(
            std::fs::read_to_string(temp.path().join("tracked.txt"))
                .expect("read tracked file")
                .trim_end(),
            "initial"
        );
    }

    #[test]
    fn pull_commit_error_keeps_new_worktree_when_reference_was_updated() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let repo = init_repository(temp.path());
        let (local_oid, upstream_oid) = prepare_pending_fast_forward(&repo);
        drop(
            repo.reference_matching(
                "refs/heads/main",
                upstream_oid,
                true,
                local_oid,
                "test: simulate partially committed transaction",
            )
            .expect("advance branch reference"),
        );

        let error = finish_pull_transaction(
            &repo,
            local_oid,
            upstream_oid,
            "refs/heads/main",
            Err(git2::Error::from_str(
                "simulated transaction commit failure",
            )),
        )
        .expect_err("simulated transaction failure must be reported");

        assert!(error.message().contains("after updating"));
        assert_eq!(repo.head().expect("load HEAD").target(), Some(upstream_oid));
        assert_eq!(
            std::fs::read_to_string(temp.path().join("tracked.txt"))
                .expect("read tracked file")
                .trim_end(),
            "upstream"
        );
    }
}
