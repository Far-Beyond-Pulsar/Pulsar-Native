//! Local filesystem provider implementation

use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::provider_trait::{FsEntry, FsMetadata, FsProvider};
use crate::{events, FsChangeKind};

static STAGING_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Standard local-disk implementation of [`FsProvider`].
///
/// # Security
///
/// When constructed with [`LocalFsProvider::with_root`] every operation is
/// validated against the root directory. Any path that resolves (via
/// `canonicalize`) outside the root is rejected — this prevents path traversal
/// attacks.
pub struct LocalFsProvider {
    /// Optional sandbox root. When `Some`, all paths are validated against it.
    root: Option<PathBuf>,
}

impl LocalFsProvider {
    /// Create a provider with **no** sandbox root.
    ///
    /// All paths accessible to the process are valid. Only use when the
    /// provider is scoped by the caller (e.g. in a restricted process).
    pub fn new() -> Self {
        Self { root: None }
    }

    /// Create a provider that restricts all operations to `root`.
    ///
    /// The root is canonicalized immediately so that symbolic-link-based
    /// escapes are detected at construction time.
    pub fn with_root(root: PathBuf) -> Result<Self> {
        // Canonicalize eagerly so we catch a missing root early.
        let root = root
            .canonicalize()
            .context("LocalFsProvider root path does not exist")?;
        Ok(Self { root: Some(root) })
    }

    // ── helpers ───────────────────────────────────────────────────────────

    /// Validate that `path` sits inside the optional root.
    ///
    /// For *read* operations the path must exist and its canonical form must
    /// start with the canonical root.
    fn check_read_allowed(&self, path: &Path) -> Result<()> {
        let Some(root) = &self.root else {
            return Ok(());
        };
        let root_canonical = root
            .canonicalize()
            .context("Failed to canonicalize sandbox root")?;
        let path_canonical = path.canonicalize().with_context(|| {
            format!(
                "Path '{}' does not exist or cannot be resolved",
                path.display()
            )
        })?;
        if !path_canonical.starts_with(&root_canonical) {
            anyhow::bail!(
                "Path '{}' resolves outside the sandbox root '{}'",
                path.display(),
                root.display(),
            );
        }
        Ok(())
    }

    /// Validate that a *write* target sits inside the root.
    ///
    /// The path itself need not exist — we walk up the ancestor chain until
    /// we find an existing path and verify *that* is inside the root.
    fn check_write_allowed(&self, path: &Path) -> Result<()> {
        let Some(root) = &self.root else {
            return Ok(());
        };
        let root_canonical = root
            .canonicalize()
            .context("Failed to canonicalize sandbox root")?;

        // If the path itself exists, use the read check.
        if path.exists() {
            return self.check_read_allowed(path);
        }

        // Walk up until we find an ancestor that exists.
        let ancestor = path
            .ancestors()
            .skip(1) // skip self (already checked above)
            .find(|a| a.exists())
            .unwrap_or(root.as_path());

        if ancestor == root.as_path() {
            return Ok(());
        }

        let ancestor_canonical = ancestor
            .canonicalize()
            .with_context(|| format!("Cannot resolve ancestor '{}'", ancestor.display()))?;
        if !ancestor_canonical.starts_with(&root_canonical) {
            anyhow::bail!(
                "Path '{}' has ancestor '{}' outside the sandbox root '{}'",
                path.display(),
                ancestor.display(),
                root.display(),
            );
        }
        Ok(())
    }

    fn create_file_exclusive(&self, path: &Path, content: &[u8]) -> Result<()> {
        self.check_write_allowed(path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .with_context(|| format!("Failed to exclusively create '{}'", path.display()))?;

        if let Err(error) = file.write_all(content) {
            drop(file);
            let _ = std::fs::remove_file(path);
            return Err(error).with_context(|| {
                format!("Failed to write newly created file '{}'", path.display())
            });
        }

        drop(file);
        events::emit(path.to_path_buf(), FsChangeKind::Created);
        Ok(())
    }

    fn write_file_atomically_impl(&self, path: &Path, content: &[u8]) -> Result<()> {
        self.check_write_allowed(path)?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let file_name = path
            .file_name()
            .context("Atomic destination must include a file name")?
            .to_string_lossy();

        let (staged_path, mut staged_file) = loop {
            let sequence = STAGING_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let candidate = parent.join(format!(
                ".{file_name}.{}.{}.tmp",
                std::process::id(),
                sequence
            ));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(file) => break (candidate, file),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("Failed to create staged file '{}'", candidate.display())
                    });
                }
            }
        };

        let result = (|| -> Result<()> {
            staged_file.write_all(content).with_context(|| {
                format!("Failed to write staged file '{}'", staged_path.display())
            })?;
            staged_file.sync_all().with_context(|| {
                format!("Failed to sync staged file '{}'", staged_path.display())
            })?;
            drop(staged_file);
            replace_file(&staged_path, path)
                .with_context(|| format!("Failed to atomically replace '{}'", path.display()))
        })();

        if let Err(error) = result {
            let _ = std::fs::remove_file(&staged_path);
            return Err(error);
        }

        events::emit(path.to_path_buf(), FsChangeKind::Modified);
        Ok(())
    }

    fn create_executable_file_exclusive(&self, path: &Path, content: &[u8]) -> Result<()> {
        self.check_write_allowed(path)?;
        let parent = path
            .parent()
            .context("Executable file path must include a parent directory")?;
        std::fs::create_dir_all(parent)?;

        let (staged_path, mut staged_file) = loop {
            let sequence = STAGING_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let candidate = parent.join(format!(
                ".pulsar-executable.{}.{}.tmp",
                std::process::id(),
                sequence
            ));
            if staging_path_conflicts(&candidate, path) {
                continue;
            }
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }

            match options.open(&candidate) {
                Ok(file) => break (candidate, file),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "Failed to create staged executable file '{}'",
                            candidate.display()
                        )
                    })
                }
            }
        };

        let result = (|| -> Result<()> {
            staged_file.write_all(content).with_context(|| {
                format!(
                    "Failed to write staged executable file '{}'",
                    staged_path.display()
                )
            })?;
            staged_file.sync_all().with_context(|| {
                format!(
                    "Failed to sync staged executable file '{}'",
                    staged_path.display()
                )
            })?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                staged_file
                    .set_permissions(std::fs::Permissions::from_mode(0o755))
                    .with_context(|| {
                        format!(
                            "Failed to make staged file '{}' executable",
                            staged_path.display()
                        )
                    })?;
                staged_file.sync_all().with_context(|| {
                    format!(
                        "Failed to sync executable permissions for '{}'",
                        staged_path.display()
                    )
                })?;
            }

            drop(staged_file);
            publish_new_file(&staged_path, path).with_context(|| {
                format!(
                    "Failed to exclusively publish executable file '{}'",
                    path.display()
                )
            })
        })();

        if let Err(error) = result {
            let _ = std::fs::remove_file(&staged_path);
            return Err(error);
        }

        match std::fs::remove_file(&staged_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                path = %staged_path.display(),
                error = %error,
                "Failed to remove staged executable file after publication"
            ),
        }

        events::emit(path.to_path_buf(), FsChangeKind::Created);
        Ok(())
    }
}

#[cfg(windows)]
fn staging_path_conflicts(candidate: &Path, destination: &Path) -> bool {
    candidate
        .to_string_lossy()
        .eq_ignore_ascii_case(&destination.to_string_lossy())
}

#[cfg(not(windows))]
fn staging_path_conflicts(candidate: &Path, destination: &Path) -> bool {
    candidate == destination
}

#[cfg(unix)]
fn publish_new_file(staged_path: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::hard_link(staged_path, destination)
}

#[cfg(unix)]
fn replace_file(staged_path: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(staged_path, destination)
}

#[cfg(windows)]
const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
#[cfg(windows)]
const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    #[link_name = "MoveFileExW"]
    fn move_file_ex_w(existing_file_name: *const u16, new_file_name: *const u16, flags: u32)
        -> i32;
}

#[cfg(windows)]
fn publish_new_file(staged_path: &Path, destination: &Path) -> std::io::Result<()> {
    move_file_ex(staged_path, destination, false)
}

#[cfg(windows)]
fn replace_file(staged_path: &Path, destination: &Path) -> std::io::Result<()> {
    move_file_ex(staged_path, destination, true)
}

#[cfg(windows)]
fn move_file_ex(staged_path: &Path, destination: &Path, replace: bool) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    let staged_path = staged_path.canonicalize()?;
    let destination_parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()?;
    let destination = destination_parent.join(destination.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Destination has no file name",
        )
    })?);

    let encode_path = |path: &Path| -> std::io::Result<Vec<u16>> {
        let mut encoded: Vec<u16> = path.as_os_str().encode_wide().collect();
        if encoded.contains(&0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Windows path contains an embedded NUL",
            ));
        }
        encoded.push(0);
        Ok(encoded)
    };
    let staged_path = encode_path(&staged_path)?;
    let destination = encode_path(&destination)?;

    let flags = MOVEFILE_WRITE_THROUGH
        | if replace {
            MOVEFILE_REPLACE_EXISTING
        } else {
            0
        };
    let result = unsafe { move_file_ex_w(staged_path.as_ptr(), destination.as_ptr(), flags) };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn publish_new_file(staged_path: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::hard_link(staged_path, destination)
}

#[cfg(not(any(unix, windows)))]
fn replace_file(staged_path: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(staged_path, destination)
}

impl Default for LocalFsProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl FsProvider for LocalFsProvider {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        self.check_read_allowed(path)?;
        Ok(std::fs::read(path)?)
    }

    fn write_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        self.check_write_allowed(path)?;
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let result = std::fs::write(path, content).map_err(Into::into);
        if result.is_ok() {
            events::emit(path.to_path_buf(), FsChangeKind::Modified);
        }
        result
    }

    fn write_file_atomically(&self, path: &Path, content: &[u8]) -> Result<()> {
        self.write_file_atomically_impl(path, content)
    }

    fn create_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        self.create_file_exclusive(path, content)
    }

    fn create_executable_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        self.create_executable_file_exclusive(path, content)
    }

    fn delete_path(&self, path: &Path) -> Result<()> {
        self.check_write_allowed(path)?;
        if path.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
        events::emit(path.to_path_buf(), FsChangeKind::Deleted);
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        self.check_write_allowed(from)?;
        self.check_write_allowed(to)?;
        let result = std::fs::rename(from, to).map_err(Into::into);
        if result.is_ok() {
            events::emit(from.to_path_buf(), FsChangeKind::Deleted);
            events::emit(to.to_path_buf(), FsChangeKind::Created);
        }
        result
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<FsEntry>> {
        self.check_read_allowed(path)?;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let meta = entry.metadata()?;
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            entries.push(FsEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: meta.is_dir(),
                size: meta.len(),
                modified,
            });
        }
        Ok(entries)
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        self.check_write_allowed(path)?;
        let result = std::fs::create_dir_all(path).map_err(Into::into);
        if result.is_ok() {
            events::emit(path.to_path_buf(), FsChangeKind::Created);
        }
        result
    }

    fn exists(&self, path: &Path) -> Result<bool> {
        // If path escapes the root, report it as non-existent to avoid
        // leaking information about files outside the sandbox.
        if self.check_read_allowed(path).is_err() {
            return Ok(false);
        }
        Ok(path.exists())
    }

    fn metadata(&self, path: &Path) -> Result<FsMetadata> {
        self.check_read_allowed(path)?;
        let m = std::fs::metadata(path)?;
        let modified = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        Ok(FsMetadata {
            is_dir: m.is_dir(),
            size: m.len(),
            modified,
        })
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        self.check_read_allowed(path)?;
        path.canonicalize()
            .with_context(|| format!("Failed to canonicalize '{}'", path.display()))
    }

    fn is_symlink(&self, path: &Path) -> Result<bool> {
        self.check_read_allowed(path)?;
        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("Failed to inspect link metadata for '{}'", path.display()))?;

        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt as _;

            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
            Ok(metadata.file_type().is_symlink()
                || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
        }

        #[cfg(not(windows))]
        {
            Ok(metadata.file_type().is_symlink())
        }
    }

    fn is_remote(&self) -> bool {
        false
    }

    fn permits_local_executable_writes(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_file_is_exclusive() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let provider =
            LocalFsProvider::with_root(temp.path().to_path_buf()).expect("create local provider");
        let path = temp.path().join("exclusive.txt");

        provider.create_file(&path, b"first").expect("create file");
        assert!(provider.create_file(&path, b"second").is_err());
        assert_eq!(std::fs::read(path).expect("read file"), b"first");
    }

    #[test]
    fn executable_creation_is_exclusive() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let provider =
            LocalFsProvider::with_root(temp.path().to_path_buf()).expect("create local provider");
        let path = temp.path().join("pre-commit");
        std::fs::write(&path, b"existing").expect("write existing file");

        assert!(provider
            .create_executable_file(&path, b"replacement")
            .is_err());
        assert_eq!(std::fs::read(path).expect("read file"), b"existing");
        assert_eq!(
            std::fs::read_dir(temp.path())
                .expect("read temp directory")
                .count(),
            1
        );
    }

    #[test]
    fn atomic_write_replaces_existing_file_without_staging_artifacts() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let provider =
            LocalFsProvider::with_root(temp.path().to_path_buf()).expect("create local provider");
        let path = temp.path().join("settings.toml");
        std::fs::write(&path, b"old").expect("write existing file");

        provider
            .write_file_atomically(&path, b"new")
            .expect("replace file");

        assert_eq!(std::fs::read(&path).expect("read replaced file"), b"new");
        let entries = std::fs::read_dir(temp.path())
            .expect("read temp directory")
            .map(|entry| entry.expect("read entry").file_name())
            .collect::<Vec<_>>();
        assert_eq!(
            entries,
            vec![path.file_name().expect("file name").to_os_string()]
        );
    }
}
