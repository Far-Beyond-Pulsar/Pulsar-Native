use gpui::{Context, Styled, Window};
use rust_i18n::t;
use std::path::PathBuf;
use ui::Icon;

use crate::utils::actions::*;

pub fn folder_context_menu(
    path: PathBuf,
    has_clipboard: bool,
    file_types: Vec<plugin_editor_api::FileTypeDefinition>,
) -> impl Fn(
    ui::popup_menu::PopupMenu,
    &mut Window,
    &mut Context<ui::popup_menu::PopupMenu>,
) -> ui::popup_menu::PopupMenu
       + 'static {
    move |menu, window, cx| {
        let path_str = path.to_string_lossy().to_string();
        let ft_clone = file_types.clone();
        let ps_sub = path_str.clone();
        let mut menu = menu.submenu_with_icon(
            Some(ui::Icon::new(ui::IconName::Plus)),
            t!("FileManager.New").to_string(),
            window,
            cx,
            move |submenu, _, _| {
                let mut sub = submenu;
                for ft in ft_clone.iter() {
                    let id = ft.id.clone();
                    let name = ft.display_name.clone();
                    let ext = ft.extension.clone();
                    sub = sub.menu(
                        t!("FileManager.NewAsset", name => name).to_string(),
                        Box::new(CreateAsset {
                            file_type_id: id.to_string(),
                            display_name: name,
                            extension: ext,
                            default_content: serde_json::Value::Null,
                        }),
                    );
                }
                sub.separator().menu(
                    t!("FileManager.NewFolder").to_string(),
                    Box::new(NewFolder {
                        folder_path: ps_sub.clone(),
                    }),
                )
            },
        );
        menu = menu
            .separator()
            .menu(
                t!("FileManager.OpenInFileManager").to_string(),
                Box::new(OpenInFileManager {
                    item_path: path_str.clone(),
                }),
            )
            .menu(
                t!("FileManager.OpenTerminalHere").to_string(),
                Box::new(OpenTerminalHere {
                    folder_path: path_str.clone(),
                }),
            );
        if has_clipboard {
            menu = menu
                .separator()
                .menu(t!("FileManager.Paste").to_string(), Box::new(Paste));
        }
        menu
    }
}

pub fn item_context_menu(
    path: PathBuf,
    has_clipboard: bool,
    is_class: bool,
) -> impl Fn(
    ui::popup_menu::PopupMenu,
    &mut Window,
    &mut Context<ui::popup_menu::PopupMenu>,
) -> ui::popup_menu::PopupMenu
       + 'static {
    move |menu, _window, _cx| {
        let path_str = path.to_string_lossy().to_string();
        let mut menu = menu;
        if is_class {
            menu = menu.menu(
                t!("FileManager.OpenClass").to_string(),
                Box::new(OpenInFileManager {
                    item_path: path_str.clone(),
                }),
            );
        }
        menu = menu
            .menu(
                t!("FileManager.Rename").to_string(),
                Box::new(RenameItem {
                    item_path: path_str.clone(),
                }),
            )
            .menu(
                t!("FileManager.Duplicate").to_string(),
                Box::new(DuplicateItem {
                    item_path: path_str.clone(),
                }),
            )
            .menu(
                t!("FileManager.Delete").to_string(),
                Box::new(DeleteItem {
                    item_path: path_str.clone(),
                }),
            )
            .separator()
            .menu(t!("FileManager.Copy").to_string(), Box::new(Copy))
            .menu(t!("FileManager.Cut").to_string(), Box::new(Cut));
        if has_clipboard {
            menu = menu.menu(t!("FileManager.Paste").to_string(), Box::new(Paste));
        }
        menu = menu
            .separator()
            .menu(
                t!("FileManager.OpenInFileManager").to_string(),
                Box::new(OpenInFileManager {
                    item_path: path_str.clone(),
                }),
            )
            .menu(
                t!("FileManager.OpenTerminalHere").to_string(),
                Box::new(OpenTerminalHere {
                    folder_path: path_str.clone(),
                }),
            );
        if is_class {
            menu = menu.separator().submenu(
                t!("FileManager.Class").to_string(),
                _window,
                _cx,
                move |sub, _, _| {
                    sub.menu(
                        t!("FileManager.SetAsBlueprint").to_string(),
                        Box::new(ValidateAsset {
                            item_path: path_str.clone(),
                        }),
                    )
                },
            );
        }
        menu
    }
}
