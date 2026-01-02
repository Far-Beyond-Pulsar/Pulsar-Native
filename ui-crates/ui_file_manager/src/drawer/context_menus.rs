use gpui::Hsla;
use std::path::PathBuf;
use ui::popup_menu::PopupMenuExt;
use ui::{Icon, IconName};

use super::actions::*;

// ============================================================================
// CONTEXT MENUS - Right-click context menu builders
// ============================================================================

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
                    .menu("Folder", Box::new(NewFolder::default()))
                    .separator();

                // Add all registered file types from plugins (sorted alphabetically)
                for file_type in file_types_clone.iter() {
                    let icon = Icon::new(file_type.icon.clone()).text_color(file_type.color);
                    
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
