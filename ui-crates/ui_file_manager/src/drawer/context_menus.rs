use gpui::*;
use std::path::PathBuf;
use ui::popup_menu::PopupMenuExt;

use super::actions::*;

// ============================================================================
// CONTEXT MENUS - Right-click context menu builders
// ============================================================================

/// Build a context menu for folders
pub fn folder_context_menu(
    path: PathBuf,
    has_clipboard: bool,
) -> impl Fn(ui::popup_menu::PopupMenu, &mut Window, &mut Context<ui::popup_menu::PopupMenu>) -> ui::popup_menu::PopupMenu + 'static {
    move |menu, _window, _cx| {
        let mut menu = menu
            .menu("New Folder", Box::new(NewFolder::default()))
            .menu("New File", Box::new(NewFile::default()))
            .menu("New Class", Box::new(NewClass::default()))
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
