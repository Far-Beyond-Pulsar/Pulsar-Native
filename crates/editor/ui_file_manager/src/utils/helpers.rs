use crate::utils::fs_metadata::FsMetadataManager;
use crate::utils::types::FileItem;
use std::path::{Path, PathBuf};
use ui::IconName;

pub fn format_file_size(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if size >= GB {
        format!("{:.2} GB", size as f64 / GB as f64)
    } else if size >= MB {
        format!("{:.2} MB", size as f64 / MB as f64)
    } else if size >= KB {
        format!("{:.2} KB", size as f64 / KB as f64)
    } else {
        format!("{} B", size)
    }
}

pub fn format_modified_time(time: Option<std::time::SystemTime>) -> String {
    time.and_then(|t| {
        t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| {
            chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + d)
                .format("%b %d, %Y %H:%M")
                .to_string()
        })
    })
    .unwrap_or_else(|| "Unknown".to_string())
}

pub fn get_icon_color_for_file_type(
    item: &FileItem,
    theme: &ui::Theme,
    meta: &mut FsMetadataManager,
) -> gpui::Hsla {
    if let Some(c) = meta.get_color_override(&item.path) {
        return c;
    }
    item.file_type_def
        .as_ref()
        .map(|d| d.color)
        .unwrap_or(theme.muted_foreground)
}

pub fn get_icon_for_file_type(item: &FileItem) -> IconName {
    item.file_type_def
        .as_ref()
        .map(|d| d.icon.clone())
        .unwrap_or(if item.is_folder {
            IconName::Folder
        } else {
            IconName::Page
        })
}

pub fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let e = entry?;
        let ty = e.file_type()?;
        if ty.is_dir() {
            copy_dir_all(&e.path(), &dst.join(e.file_name()))?;
        } else {
            std::fs::copy(e.path(), dst.join(e.file_name()))?;
        }
    }
    Ok(())
}

pub fn cloud_join(base: &Path, component: &str) -> PathBuf {
    if engine_fs::is_cloud_path(base) {
        let s = base.to_string_lossy().replace('\\', "/");
        PathBuf::from(format!("{}/{}", s.trim_end_matches('/'), component))
    } else {
        base.join(component)
    }
}
