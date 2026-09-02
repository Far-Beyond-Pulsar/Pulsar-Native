use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

#[derive(Clone, Debug)]
pub struct CommitInfo {
    pub hash: String,
    pub short_hash: String,
    pub subject: String,
    pub date: String,
}

#[derive(Clone, Debug)]
pub struct DeletedFileEntry {
    pub path: PathBuf,
    pub commit_hash: String,
}

/// Run `git` in the given repo directory and return stdout.
fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|e| format!("failed to execute git: {e}"))
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).map_err(|e| format!("invalid utf-8: {e}"))
            } else {
                let stderr = String::from_utf8_lossy(&o.stderr);
                Err(format!("git error: {stderr}"))
            }
        })
}

/// Return up to `count` recent commits.
pub fn recent_commits(repo: &Path, count: usize) -> Vec<CommitInfo> {
    let fmt = "%h|%H|%s|%ai";
    let out = match git(
        repo,
        &[
            "log",
            &format!("-{count}"),
            &format!("--pretty=format:{fmt}"),
        ],
    ) {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!("recent_commits: {e}");
            return vec![];
        }
    };
    out.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(4, '|').collect();
            if parts.len() < 4 {
                return None;
            }
            Some(CommitInfo {
                hash: parts[1].to_string(),
                short_hash: parts[0].to_string(),
                subject: parts[2].to_string(),
                date: parts[3].to_string(),
            })
        })
        .collect()
}

/// Get list of files deleted in a commit (files that existed in parent but not in this commit).
pub fn deleted_files_in_commit(repo: &Path, commit: &str) -> Vec<DeletedFileEntry> {
    let out = match git(
        repo,
        &[
            "log",
            "-1",
            "--diff-filter=D",
            "--name-only",
            "--pretty=format:",
            commit,
        ],
    ) {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!("deleted_files_in_commit({commit}): {e}");
            return vec![];
        }
    };
    out.lines()
        .filter(|l| !l.is_empty())
        .map(|l| DeletedFileEntry {
            path: PathBuf::from(l),
            commit_hash: commit.to_string(),
        })
        .collect()
}

/// Read a file's content from a specific commit.
pub fn read_file_from_commit(repo: &Path, commit: &str, path: &Path) -> Result<Vec<u8>, String> {
    let rel = path.strip_prefix(repo).unwrap_or(path);
    let spec = format!("{commit}:{}", rel.display());
    let out = Command::new("git")
        .args(["show", &spec])
        .current_dir(repo)
        .output()
        .map_err(|e| format!("failed to run git show: {e}"))?;

    if out.status.success() {
        Ok(out.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(format!("git show error: {stderr}"))
    }
}
