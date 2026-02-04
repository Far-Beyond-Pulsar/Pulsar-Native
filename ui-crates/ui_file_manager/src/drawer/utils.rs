use ui::IconName;
use super::types::FileItem;
use super::fs_metadata::FsMetadataManager;

// ============================================================================
// UTILITY FUNCTIONS
// ============================================================================

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
            let datetime = chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + d);
            datetime.format("%b %d, %Y %H:%M").to_string()
        })
    })
    .unwrap_or_else(|| "Unknown".to_string())
}

pub fn get_icon_color_for_file_type(item: &FileItem, theme: &ui::Theme, fs_metadata: &mut FsMetadataManager) -> gpui::Hsla {
    // Check for color override first
    if let Some(override_color) = fs_metadata.get_color_override(&item.path) {
        return override_color;
    }
    
    // Fall back to file type color or theme default
    item.file_type_def.as_ref()
        .map(|def| def.color)
        .unwrap_or(theme.muted_foreground)
}

pub fn get_icon_for_file_type(item: &FileItem) -> IconName {
    item.file_type_def.as_ref()
        .map(|def| def.icon.clone())
        .unwrap_or(if item.is_folder { IconName::Folder } else { IconName::Page })
}

pub fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            std::fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}
