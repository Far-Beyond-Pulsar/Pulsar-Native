//! Filesystem provider abstraction
//!
//! The `FsProvider` trait decouples the engine from any single filesystem
//! backend, letting the same editor code work transparently against a local
//! disk *or* a remote `pulsar-studio` server.

use anyhow::Result;
use std::path::{Path, PathBuf};

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

/// A readable, seekable stream over one file ([`FsProvider::open_read`]).
pub trait ReadSeek: std::io::Read + std::io::Seek + Send {}

impl<T: std::io::Read + std::io::Seek + Send> ReadSeek for T {}

/// A stream that writes one file ([`crate::virtual_fs::open_write`]). What
/// was written becomes the file when [`finish`](Self::finish) returns `Ok`; a
/// writer dropped without it may leave nothing (a buffering provider) or a
/// partial file (a local one), so callers finish every writer they open.
pub struct FsWriter {
    inner: WriterInner,
}

enum WriterInner {
    /// Writes straight to an open file.
    Direct(Box<dyn std::io::Write + Send>),
    /// Collects the bytes and hands them to `commit` at the end.
    Buffered {
        bytes: Vec<u8>,
        commit: Box<dyn FnOnce(Vec<u8>) -> Result<()> + Send>,
    },
}

impl FsWriter {
    /// A writer over an open stream (a local file).
    pub fn direct(stream: Box<dyn std::io::Write + Send>) -> Self {
        Self {
            inner: WriterInner::Direct(stream),
        }
    }

    /// A writer that collects the bytes and stores them with `commit`.
    pub fn buffered(commit: impl FnOnce(Vec<u8>) -> Result<()> + Send + 'static) -> Self {
        Self {
            inner: WriterInner::Buffered {
                bytes: Vec::new(),
                commit: Box::new(commit),
            },
        }
    }

    /// Flush and store the file.
    pub fn finish(self) -> Result<()> {
        match self.inner {
            WriterInner::Direct(mut stream) => Ok(stream.flush()?),
            WriterInner::Buffered { bytes, commit } => commit(bytes),
        }
    }
}

impl std::io::Write for FsWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match &mut self.inner {
            WriterInner::Direct(stream) => stream.write(buf),
            WriterInner::Buffered { bytes, .. } => {
                bytes.extend_from_slice(buf);
                Ok(buf.len())
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match &mut self.inner {
            WriterInner::Direct(stream) => stream.flush(),
            WriterInner::Buffered { .. } => Ok(()),
        }
    }
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

    /// Read up to `len` bytes from the beginning of a file.
    ///
    /// Providers with ranged reads should override this. The fallback keeps
    /// older providers compatible, while local providers avoid loading large
    /// payloads when an asset header is sufficient.
    fn read_file_prefix(&self, path: &Path, len: usize) -> Result<Vec<u8>> {
        let mut bytes = self.read_file(path)?;
        bytes.truncate(len);
        Ok(bytes)
    }

    /// Overwrite (or create) a file with `content`.
    fn write_file(&self, path: &Path, content: &[u8]) -> Result<()>;

    /// Atomically replace `path` with `content`.
    ///
    /// Providers that cannot guarantee same-filesystem atomic replacement must
    /// reject this operation instead of falling back to a partial write.
    fn write_file_atomically(&self, path: &Path, _content: &[u8]) -> Result<()> {
        anyhow::bail!(
            "Filesystem provider '{}' does not support atomic writes at '{}'",
            self.label(),
            path.display()
        )
    }

    /// Create `path` with `content`, failing if it already exists.
    fn create_file(&self, path: &Path, content: &[u8]) -> Result<()>;

    /// Create an executable file, failing if it already exists.
    ///
    /// The destination must not become executable until the complete content
    /// has been written successfully.
    ///
    /// Providers that cannot preserve executable permissions must reject this
    /// operation instead of silently creating a non-executable file.
    fn create_executable_file(&self, path: &Path, _content: &[u8]) -> Result<()> {
        anyhow::bail!(
            "Filesystem provider '{}' does not support executable files at '{}'",
            self.label(),
            path.display()
        )
    }

    /// Add `content` to the end of a file, creating it if needed.
    ///
    /// The fallback reads and rewrites the whole file; local providers append
    /// in place.
    fn append_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        let mut bytes = if self.exists(path)? {
            self.read_file(path)?
        } else {
            Vec::new()
        };
        bytes.extend_from_slice(content);
        self.write_file(path, &bytes)
    }

    /// Copy the file `from` to `to`, replacing `to`.
    fn copy(&self, from: &Path, to: &Path) -> Result<()> {
        let bytes = self.read_file(from)?;
        self.write_file(to, &bytes)
    }

    /// Open a file for streamed reading (large assets, archives).
    ///
    /// The fallback reads the whole file into memory.
    fn open_read(&self, path: &Path) -> Result<Box<dyn ReadSeek>> {
        Ok(Box::new(std::io::Cursor::new(self.read_file(path)?)))
    }

    /// Open `path` (created or truncated) as a stream when the provider can
    /// write in place, else `None`, and [`crate::virtual_fs::open_write`]
    /// buffers and writes the file when it is finished.
    fn open_write_stream(&self, _path: &Path) -> Result<Option<Box<dyn std::io::Write + Send>>> {
        Ok(None)
    }

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

    /// Resolve `path` to its canonical absolute form.
    fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        anyhow::bail!(
            "Filesystem provider '{}' does not support canonical paths for '{}'",
            self.label(),
            path.display()
        )
    }

    /// Inspect `path` without following it and report filesystem redirections.
    ///
    /// Local implementations treat symbolic links and Windows reparse points,
    /// including junctions, as redirections.
    fn is_symlink(&self, path: &Path) -> Result<bool> {
        anyhow::bail!(
            "Filesystem provider '{}' does not support link inspection for '{}'",
            self.label(),
            path.display()
        )
    }

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

    /// Whether this provider permits locally configured executable content.
    ///
    /// This is a security-sensitive capability and therefore defaults to
    /// denied. Only providers that represent the local machine should opt in.
    fn permits_local_executable_writes(&self) -> bool {
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
            // Skip build artifacts, VCS metadata, and other non-asset directories.
            // These can contain tens of thousands of files and have no assets.
            if entry.is_dir
                && matches!(
                    entry.name.as_str(),
                    "target" | ".git" | ".svn" | ".hg" | "node_modules" | ".cache" | ".gradle"
                )
            {
                continue;
            }
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
