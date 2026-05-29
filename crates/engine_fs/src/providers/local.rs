//! Local filesystem provider implementation

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::provider_trait::{FsEntry, FsMetadata, FsProvider};

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
        root.canonicalize()
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
        let path_canonical = path
            .canonicalize()
            .with_context(|| format!("Path '{}' does not exist or cannot be resolved", path.display()))?;
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
        Ok(std::fs::write(path, content)?)
    }

    fn create_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        self.check_write_allowed(path)?;
        if path.exists() {
            anyhow::bail!("File already exists: {}", path.display());
        }
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        Ok(std::fs::write(path, content)?)
    }

    fn delete_path(&self, path: &Path) -> Result<()> {
        self.check_write_allowed(path)?;
        if path.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        self.check_write_allowed(from)?;
        self.check_write_allowed(to)?;
        Ok(std::fs::rename(from, to)?)
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
        Ok(std::fs::create_dir_all(path)?)
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
}
