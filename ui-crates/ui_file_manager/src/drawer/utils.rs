use ui::IconName;
use super::types::FileType;

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

pub fn get_icon_color_for_file_type(file_type: &FileType, theme: &ui::Theme) -> gpui::Hsla {
    match file_type {
        FileType::Folder => theme.muted_foreground,
        FileType::Class => gpui::rgb(0x9C27B0).into(), // Purple
        FileType::Script => gpui::rgb(0x2196F3).into(), // Blue
        FileType::DawProject => gpui::rgb(0xFF9800).into(), // Orange
        FileType::LevelScene => gpui::rgb(0xF44336).into(), // Red
        FileType::Database => gpui::rgb(0x4CAF50).into(), // Green
        FileType::Config => gpui::rgb(0x9E9E9E).into(), // Gray
        FileType::StructType => gpui::rgb(0x00BCD4).into(), // Cyan
        FileType::EnumType => gpui::rgb(0x673AB7).into(), // Deep Purple
        FileType::TraitType => gpui::rgb(0x3F51B5).into(), // Indigo
        FileType::AliasType => gpui::rgb(0x607D8B).into(), // Blue Gray
        FileType::Image => gpui::rgb(0xE91E63).into(), // Pink
        FileType::Audio => gpui::rgb(0x9C27B0).into(), // Purple
        FileType::Video => gpui::rgb(0xF44336).into(), // Red
        FileType::Document => gpui::rgb(0xFF5722).into(), // Deep Orange
        FileType::Archive => gpui::rgb(0x795548).into(), // Brown
        FileType::Other => theme.muted_foreground,
    }
}

pub fn get_icon_for_file_type(file_type: &FileType) -> IconName {
    match file_type {
        FileType::Folder => IconName::Folder,
        FileType::Class => IconName::Component,
        FileType::Script => IconName::Code,
        FileType::DawProject => IconName::MusicNote,
        FileType::LevelScene => IconName::Map,
        FileType::Database => IconName::Database,
        FileType::Config => IconName::Settings,
        FileType::StructType => IconName::Box,
        FileType::EnumType => IconName::List,
        FileType::TraitType => IconName::Code,
        FileType::AliasType => IconName::Link,
        FileType::Image => IconName::Image,
        FileType::Audio => IconName::MusicNote,
        FileType::Video => IconName::Movie,
        FileType::Document => IconName::Page,
        FileType::Archive => IconName::ARchive,
        FileType::Other => IconName::Page,
    }
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
