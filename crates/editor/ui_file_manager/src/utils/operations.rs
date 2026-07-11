use crate::utils::helpers::copy_dir_all;
use anyhow::Result;
use std::path::{Path, PathBuf};

fn fs_write(path: &Path, content: &[u8]) -> Result<()> {
    if engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(path) {
        engine_fs::virtual_fs::write_file(path, content)
    } else {
        std::fs::write(path, content).map_err(anyhow::Error::from)
    }
}

fn fs_mkdir(path: &Path) -> Result<()> {
    if engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(path) {
        engine_fs::virtual_fs::create_dir_all(path)
    } else {
        std::fs::create_dir_all(path).map_err(anyhow::Error::from)
    }
}

fn fs_delete(path: &Path) -> Result<()> {
    if engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(path) {
        engine_fs::virtual_fs::delete_path(path)
    } else if path.is_dir() {
        std::fs::remove_dir_all(path).map_err(anyhow::Error::from)
    } else {
        std::fs::remove_file(path).map_err(anyhow::Error::from)
    }
}

fn fs_rename(from: &Path, to: &Path) -> Result<()> {
    if engine_fs::virtual_fs::is_remote()
        || engine_fs::is_cloud_path(from)
        || engine_fs::is_cloud_path(to)
    {
        engine_fs::virtual_fs::rename(from, to)
    } else {
        std::fs::rename(from, to).map_err(anyhow::Error::from)
    }
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
            if engine_fs::virtual_fs::is_remote()
                || engine_fs::is_cloud_path(src)
                || engine_fs::is_cloud_path(target_dir)
            {
                engine_fs::virtual_fs::rename(src, &dst)?;
            } else if src.is_dir() {
                copy_dir_all(src, &dst)?;
                std::fs::remove_dir_all(src)?;
            } else {
                std::fs::copy(src, &dst)?;
                std::fs::remove_file(src)?;
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
            } else if src.is_dir() {
                copy_dir_all(src, &dst)?;
            } else {
                std::fs::copy(src, &dst)?;
            }
        }
        Ok(())
    }
}
