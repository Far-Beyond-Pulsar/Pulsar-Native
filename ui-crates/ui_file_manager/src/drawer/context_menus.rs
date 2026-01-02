use gpui::*;
use std::path::PathBuf;
use ui::popup_menu::PopupMenuExt;
use ui::{Icon, IconName};

use super::actions::*;

// ============================================================================
// CONTEXT MENUS - Right-click context menu builders
// ============================================================================

/// Map FileIcon to UI IconName and color
fn file_icon_to_ui(file_icon: &plugin_editor_api::FileIcon) -> (IconName, Hsla) {
    use plugin_editor_api::FileIcon;
    
    match file_icon {
        FileIcon::File => (IconName::Page, rgb(0x9E9E9E).into()),
        FileIcon::Code => (IconName::Code, rgb(0x2196F3).into()),
        FileIcon::Component => (IconName::Component, rgb(0xFF9800).into()),
        FileIcon::Database => (IconName::Database, rgb(0x4CAF50).into()),
        FileIcon::Music | FileIcon::Audio => (IconName::MusicNote, rgb(0x9C27B0).into()),
        FileIcon::Image => (IconName::Image, rgb(0xE91E63).into()),
        FileIcon::Video => (IconName::Movie, rgb(0xF44336).into()),
        FileIcon::Archive => (IconName::ARchive, rgb(0x795548).into()),
        FileIcon::Document => (IconName::Page, rgb(0xFF5722).into()),
        
        // Programming languages
        FileIcon::Rust => (IconName::Code, rgb(0xFF5722).into()),
        FileIcon::Python => (IconName::Code, rgb(0x3776AB).into()),
        FileIcon::JavaScript => (IconName::Code, rgb(0xF7DF1E).into()),
        FileIcon::TypeScript => (IconName::Code, rgb(0x3178C6).into()),
        FileIcon::Cpp => (IconName::Code, rgb(0x00599C).into()),
        FileIcon::CSharp => (IconName::Code, rgb(0x239120).into()),
        FileIcon::Go => (IconName::Code, rgb(0x00ADD8).into()),
        
        // Asset types
        FileIcon::Model3D => (IconName::Box, rgb(0x00BCD4).into()),
        FileIcon::Texture => (IconName::Image, rgb(0xE91E63).into()),
        FileIcon::Material => (IconName::Palette, rgb(0x9C27B0).into()),
        FileIcon::Animation => (IconName::Play, rgb(0xFF5722).into()),
        FileIcon::Particle => (IconName::Star, rgb(0xFFC107).into()),
        FileIcon::Level => (IconName::Map, rgb(0xF44336).into()),
        FileIcon::Prefab => (IconName::Component, rgb(0xFF9800).into()),
        
        // Type system
        FileIcon::Struct => (IconName::Box, rgb(0x00BCD4).into()),
        FileIcon::Enum => (IconName::List, rgb(0x673AB7).into()),
        FileIcon::Trait => (IconName::Code, rgb(0x3F51B5).into()),
        FileIcon::Interface => (IconName::Code, rgb(0x3F51B5).into()),
        FileIcon::Class => (IconName::Component, rgb(0xFF9800).into()),
        
        FileIcon::Custom(_) => (IconName::Page, rgb(0x9E9E9E).into()),
    }
}

/// Build a context menu for folders
pub fn folder_context_menu(
    path: PathBuf,
    has_clipboard: bool,
    file_types: Vec<plugin_editor_api::FileTypeDefinition>,
) -> impl Fn(ui::popup_menu::PopupMenu, &mut Window, &mut Context<ui::popup_menu::PopupMenu>) -> ui::popup_menu::PopupMenu + 'static {
    move |menu, window, cx| {
        let mut file_types_clone = file_types.clone();
        // Sort file types alphabetically by display name
        file_types_clone.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        
        let mut menu = menu
            .submenu("Create", window, cx, move |submenu, _window, _cx| {
                let mut submenu = submenu
                    .menu("Folder", Box::new(NewFolder::default()));

                // Add all registered file types from plugins (sorted alphabetically)
                for file_type in file_types_clone.iter() {
                    let (icon_name, color) = file_icon_to_ui(&file_type.icon);
                    let icon = Icon::new(icon_name).text_color(color);
                    
                    submenu = submenu.menu_with_icon(
                        file_type.display_name.clone(),
                        icon,
                        Box::new(CreateAsset {
                            file_type_id: file_type.id.as_str().to_string(),
                            display_name: file_type.display_name.clone(),
                            extension: file_type.extension.clone(),
                            default_content: file_type.default_content.clone(),
                        })
                    );
                }

                submenu
            })
            .separator()
            .menu("Cut", Box::new(Cut))
            .menu("Copy", Box::new(Copy));

        if has_clipboard {
            menu = menu.menu("Paste", Box::new(Paste));
        }

        menu = menu
            .separator()
            .menu("Rename", Box::new(RenameItem::default()))
            .menu("Delete", Box::new(DeleteItem::default()))
            .separator()
            .menu("Duplicate", Box::new(DuplicateItem::default()))
            .separator()
            .menu("Refresh", Box::new(RefreshFileManager));

        menu
    }
}

/// Build a context menu for files and other items
pub fn item_context_menu(
    path: PathBuf,
    has_clipboard: bool,
    is_class: bool,
) -> impl Fn(ui::popup_menu::PopupMenu, &mut Window, &mut Context<ui::popup_menu::PopupMenu>) -> ui::popup_menu::PopupMenu + 'static {
    move |menu, _window, _cx| {
        let mut menu = menu;

        // Class-specific actions
        if is_class {
            menu = menu
                .menu("Open Class", Box::new(NewClass::default())) // Reuse action or create OpenClass
                .separator();
        }

        menu = menu
            .menu("Cut", Box::new(Cut))
            .menu("Copy", Box::new(Copy));

        if has_clipboard {
            menu = menu.menu("Paste", Box::new(Paste));
        }

        menu = menu
            .separator()
            .menu("Rename", Box::new(RenameItem::default()))
            .menu("Delete", Box::new(DeleteItem::default()))
            .separator()
            .menu("Duplicate", Box::new(DuplicateItem::default()));

        menu
    }
}
