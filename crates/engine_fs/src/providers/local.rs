//! Local filesystem provider implementation

use anyhow::Result;
use std::path::Path;

use super::provider_trait::{FsEntry, FsMetadata, FsProvider};

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
