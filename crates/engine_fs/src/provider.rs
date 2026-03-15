//! Filesystem provider abstraction
//!
//! The `FsProvider` trait decouples the engine from any single filesystem
//! backend, letting the same editor code work transparently against a local
//! disk *or* a remote `pulsar-host` server.

use anyhow::Result;
use std::path::Path;

// ── Data types ─────────────────────────────────────────────────────────────────

/// A single entry returned by [`FsProvider::list_dir`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FsEntry {
    /// Filename without any parent path components.
    pub name: String,
    /// Whether this entry is a directory.
    pub is_dir: bool,
    /// Size in bytes; `0` for directories.
    pub size: u64,
    /// Last-modified time as a Unix timestamp (seconds since epoch).
    pub modified: Option<u64>,
}

/// Lightweight metadata for a single path.
#[derive(Debug, Clone)]
pub struct FsMetadata {
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<u64>,
}

/// Flat file manifest entry (for whole-project tree scans).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManifestEntry {
    /// Path relative to the project workspace root, using forward slashes.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<u64>,
}

// ── Trait ──────────────────────────────────────────────────────────────────────

/// Backend-agnostic filesystem interface.
///
/// All operations are **synchronous** so they can be called from any context
/// without requiring an async runtime. Implementations must be
/// `Send + Sync + 'static` so they can live in the
/// [`crate::virtual_fs`] global.
pub trait FsProvider: Send + Sync + 'static {
    // ── Core I/O ─────────────────────────────────────────────────────────────

    /// Read the full contents of a file.
    fn read_file(&self, path: &Path) -> Result<Vec<u8>>;

    /// Overwrite (or create) a file with `content`.
    fn write_file(&self, path: &Path, content: &[u8]) -> Result<()>;

    /// Create `path` with `content`, failing if it already exists.
    fn create_file(&self, path: &Path, content: &[u8]) -> Result<()>;

    /// Delete a file, or recursively remove a directory.
    fn delete_path(&self, path: &Path) -> Result<()>;

    /// Rename / move `from` to `to`.
    fn rename(&self, from: &Path, to: &Path) -> Result<()>;

    // ── Directory operations ──────────────────────────────────────────────────

    /// List immediate children of a directory.
    fn list_dir(&self, path: &Path) -> Result<Vec<FsEntry>>;

    /// Recursively create `path` and all missing parents.
    fn create_dir_all(&self, path: &Path) -> Result<()>;

    // ── Metadata ─────────────────────────────────────────────────────────────

    /// Return `true` if `path` exists.
    fn exists(&self, path: &Path) -> Result<bool>;

    /// Return basic metadata about `path`.
    fn metadata(&self, path: &Path) -> Result<FsMetadata>;

    // ── Full-project manifest ─────────────────────────────────────────────────

    /// Return a flat list of every file and directory under `path`.
    ///
    /// The *default* implementation recursively calls [`Self::list_dir`]; remote
    /// providers override this to issue a single network round-trip.
    fn manifest(&self, path: &Path) -> Result<Vec<ManifestEntry>> {
        let mut out = Vec::new();
        self.manifest_recursive(path, path, &mut out)?;
        Ok(out)
    }

    // ── Descriptor ───────────────────────────────────────────────────────────

    /// Whether this provider serves a remote filesystem.
    fn is_remote(&self) -> bool {
        false
    }

    /// Short human-readable label shown in the file manager toolbar.
    fn label(&self) -> &str {
        "Local"
    }

    // ── Private helper (default impl) ─────────────────────────────────────────
    #[doc(hidden)]
    fn manifest_recursive(
        &self,
        root: &Path,
        dir: &Path,
        out: &mut Vec<ManifestEntry>,
    ) -> Result<()> {
        for entry in self.list_dir(dir)? {
            let child_path = dir.join(&entry.name);
            let rel = child_path
                .strip_prefix(root)
                .unwrap_or(&child_path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push(ManifestEntry {
                path: rel,
                is_dir: entry.is_dir,
                size: entry.size,
                modified: entry.modified,
            });
            if entry.is_dir {
                self.manifest_recursive(root, &child_path, out)?;
            }
        }
        Ok(())
    }
}

// ── Local provider ─────────────────────────────────────────────────────────────

/// Standard local-disk implementation of [`FsProvider`].
pub struct LocalFsProvider;

impl FsProvider for LocalFsProvider {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        Ok(std::fs::read(path)?)
    }

    fn write_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        Ok(std::fs::write(path, content)?)
    }

    fn create_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        if path.exists() {
            anyhow::bail!("File already exists: {}", path.display());
        }
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        Ok(std::fs::write(path, content)?)
    }

    fn delete_path(&self, path: &Path) -> Result<()> {
        if path.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        Ok(std::fs::rename(from, to)?)
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<FsEntry>> {
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
        Ok(std::fs::create_dir_all(path)?)
    }

    fn exists(&self, path: &Path) -> Result<bool> {
        Ok(path.exists())
    }

    fn metadata(&self, path: &Path) -> Result<FsMetadata> {
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
