use crate::utils::helpers::copy_dir_all;
use anyhow::Result;
use std::path::{Path, PathBuf};

// Through the virtual filesystem, local or remote: its operations announce
// each change, which keeps the project's file index (what the drawer and the
// sidebar list) current without waiting for a watcher.
fn fs_write(path: &Path, content: &[u8]) -> Result<()> {
    engine_fs::virtual_fs::write_file(path, content)
}

fn fs_mkdir(path: &Path) -> Result<()> {
    engine_fs::virtual_fs::create_dir_all(path)
}

fn fs_delete(path: &Path) -> Result<()> {
    engine_fs::virtual_fs::delete_path(path)
}

fn fs_rename(from: &Path, to: &Path) -> Result<()> {
    engine_fs::virtual_fs::rename(from, to)
}

/// Copy a local file or folder, announcing the copy as the virtual
/// filesystem's own operations do.
pub(crate) fn copy_local(src: &Path, dst: &Path) -> Result<()> {
    if src.is_dir() {
        copy_dir_all(src, dst)?;
    } else {
        std::fs::copy(src, dst)?;
    }
    engine_fs::emit(dst.to_path_buf(), engine_fs::FsChangeKind::Created);
    Ok(())
}

pub struct FileOperations {
    project_path: Option<PathBuf>,
}

impl FileOperations {
    pub fn new(project_path: Option<PathBuf>) -> Self {
        Self { project_path }
    }

    pub fn rename_item(&self, old_path: &Path, new_name: &str) -> Result<PathBuf> {
        let parent = old_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("No parent directory"))?;
        let new_path = parent.join(new_name);
        fs_rename(old_path, &new_path)?;
        Ok(new_path)
    }

    pub fn move_items(&self, sources: &[PathBuf], target_dir: &Path) -> Result<()> {
        fs_mkdir(target_dir)?;
        for src in sources {
            let name = src
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("Invalid source path"))?;
            let dst = target_dir.join(name);
            if dst == *src {
                continue;
            }
            let remote = engine_fs::virtual_fs::is_remote()
                || engine_fs::is_cloud_path(src)
                || engine_fs::is_cloud_path(target_dir);
            if let Err(error) = fs_rename(src, &dst) {
                if remote {
                    return Err(error);
                }
                // A rename across devices fails: copy, then delete.
                copy_local(src, &dst)?;
                fs_delete(src)?;
            }
        }
        Ok(())
    }

    pub fn copy_items(sources: &[PathBuf], target_dir: &Path) -> Result<()> {
        fs_mkdir(target_dir)?;
        for src in sources {
            let name = src
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("Invalid source path"))?;
            let dst = target_dir.join(name);
            if engine_fs::virtual_fs::is_remote()
                || engine_fs::is_cloud_path(src)
                || engine_fs::is_cloud_path(target_dir)
            {
                let data = engine_fs::virtual_fs::read_file(src)?;
                engine_fs::virtual_fs::write_file(&dst, &data)?;
            } else {
                copy_local(src, &dst)?;
            }
        }
        Ok(())
    }
}
