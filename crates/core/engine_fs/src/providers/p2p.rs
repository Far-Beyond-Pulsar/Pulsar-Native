use anyhow::{bail, Context as _, Result};
use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use pulsar_multiplayer_core::protocol::{
    FileChanged, FileChunk, FileManifest, RequestFile, SessionMessage,
};
use pulsar_multiplayer_core::session::FileChangeKind;
use pulsar_multiplayer_core::transport::SessionChannel;

use super::provider_trait::{FsEntry, FsMetadata, FsProvider, ManifestEntry};
use crate::events;

pub struct P2pFsProvider {
    channel: Arc<dyn SessionChannel>,
}

impl P2pFsProvider {
    pub fn new(channel: Arc<dyn SessionChannel>, _project_id: String) -> Self {
        Self { channel }
    }

    fn to_rel(&self, path: &Path) -> String {
        let s = path.to_string_lossy().replace('\\', "/");
        s.trim_start_matches('/').to_string()
    }

    fn block_on<F: Future>(&self, fut: F) -> F::Output {
        // Works in both async (tokio) and sync contexts.
        futures::executor::block_on(fut)
    }

    fn fetch_manifest(&self) -> Result<FileManifest> {
        self.block_on(self.channel.send(SessionMessage::RequestFileManifest))
            .context("Failed to send RequestFileManifest")?;
        loop {
            let msg = self
                .block_on(self.channel.recv())
                .context("Failed to receive manifest response")?;
            match msg {
                SessionMessage::FileManifest(m) => return Ok(m),
                SessionMessage::Error(e) => bail!("Remote error: {} ({})", e.message, e.code),
                _ => continue,
            }
        }
    }
}

impl FsProvider for P2pFsProvider {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        let rel = self.to_rel(path);
        self.block_on(self.channel.send(SessionMessage::RequestFile(RequestFile {
            path: rel.clone(),
            offset: None,
        })))
        .context("Failed to send RequestFile")?;

        let mut buf = Vec::new();
        loop {
            let msg = self
                .block_on(self.channel.recv())
                .context("Failed to receive file chunk")?;
            match msg {
                SessionMessage::FileChunk(chunk) => {
                    buf.extend_from_slice(&chunk.data);
                    if chunk.is_last {
                        return Ok(buf);
                    }
                }
                SessionMessage::Error(e) => {
                    bail!("Remote error reading file: {} ({})", e.message, e.code)
                }
                _ => continue,
            }
        }
    }

    fn write_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        let rel = self.to_rel(path);
        self.block_on(self.channel.send(SessionMessage::FileChunk(FileChunk {
            path: rel,
            offset: 0,
            data: content.to_vec(),
            is_last: true,
        })))
        .context("Failed to send FileChunk")?;
        events::emit(path.to_path_buf(), events::FsChangeKind::Modified);
        Ok(())
    }

    fn create_file(&self, path: &Path, content: &[u8]) -> Result<()> {
        if self.exists(path)? {
            bail!("File already exists: {}", path.display());
        }
        self.write_file(path, content)?;
        events::emit(path.to_path_buf(), events::FsChangeKind::Created);
        Ok(())
    }

    fn delete_path(&self, path: &Path) -> Result<()> {
        let rel = self.to_rel(path);
        self.block_on(self.channel.send(SessionMessage::FileChanged(FileChanged {
            path: rel,
            kind: FileChangeKind::Deleted,
        })))
        .context("Failed to send delete notification")?;
        events::emit(path.to_path_buf(), events::FsChangeKind::Deleted);
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        let content = self.read_file(from)?;
        self.write_file(to, &content)?;
        self.delete_path(from)?;
        Ok(())
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<FsEntry>> {
        let manifest = self.fetch_manifest()?;
        let prefix = self.to_rel(path);
        let prefix_slash = if prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", prefix)
        };

        let mut seen = std::collections::HashSet::new();
        let mut entries: Vec<FsEntry> = Vec::new();

        for entry in &manifest.entries {
            let tail = if prefix_slash.is_empty() {
                &entry.path
            } else if entry.path.starts_with(&prefix_slash) {
                &entry.path[prefix_slash.len()..]
            } else {
                continue;
            };

            let child_name = match tail.find('/') {
                Some(idx) => &tail[..idx],
                None => tail,
            };

            if child_name.is_empty() || !seen.insert(child_name.to_string()) {
                continue;
            }

            let is_dir = tail.contains('/') || entry.is_dir;
            entries.push(FsEntry {
                name: child_name.to_string(),
                is_dir,
                size: if is_dir { 0 } else { entry.size },
                modified: if is_dir { None } else { entry.modified },
            });
        }

        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        let rel = self.to_rel(path);
        self.block_on(self.channel.send(SessionMessage::FileChanged(FileChanged {
            path: rel,
            kind: FileChangeKind::Created,
        })))
        .context("Failed to send directory creation notification")?;
        events::emit(path.to_path_buf(), events::FsChangeKind::Created);
        Ok(())
    }

    fn exists(&self, path: &Path) -> Result<bool> {
        let manifest = self.fetch_manifest()?;
        let rel = self.to_rel(path);
        Ok(manifest.entries.iter().any(|e| e.path == rel))
    }

    fn metadata(&self, path: &Path) -> Result<FsMetadata> {
        let manifest = self.fetch_manifest()?;
        let rel = self.to_rel(path);
        let entry = manifest
            .entries
            .iter()
            .find(|e| e.path == rel)
            .context("Path not found in remote manifest")?;
        Ok(FsMetadata {
            is_dir: entry.is_dir,
            size: entry.size,
            modified: entry.modified,
        })
    }

    fn manifest(&self, path: &Path) -> Result<Vec<ManifestEntry>> {
        let manifest = self.fetch_manifest()?;
        let prefix = self.to_rel(path);
        let entries: Vec<ManifestEntry> = manifest
            .entries
            .into_iter()
            .filter(|e| {
                if prefix.is_empty() {
                    true
                } else {
                    e.path.starts_with(&prefix)
                }
            })
            .map(|e| ManifestEntry {
                path: e.path,
                is_dir: e.is_dir,
                size: e.size,
                modified: e.modified,
            })
            .collect();
        Ok(entries)
    }

    fn is_remote(&self) -> bool {
        true
    }

    fn label(&self) -> &str {
        "P2P"
    }
}
