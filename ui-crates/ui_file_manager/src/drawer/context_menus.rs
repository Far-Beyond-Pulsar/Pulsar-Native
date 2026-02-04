use gpui::{Context, Styled, Window};
use rust_i18n::t;
use std::path::PathBuf;
use std::collections::HashMap;
use ui::Icon;

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
        let file_types_clone = file_types.clone();
        
        let mut menu = menu
            .submenu_with_icon(Some(ui::Icon::new(ui::IconName::Plus)), &t!("FileManager.NewFolder").to_string(), window, cx, move |submenu, window, cx| {
                let mut submenu = submenu
                    .menu_with_icon(&t!("FileManager.NewFolder").to_string(), ui::Icon::new(ui::IconName::FolderPlus), Box::new(NewFolder::default()))
                    .separator();

                // Build category tree structure
                let mut category_tree: HashMap<String, CategoryNode> = HashMap::new();
                let mut top_level_items: Vec<plugin_editor_api::FileTypeDefinition> = Vec::new();
                
                for file_type in file_types_clone.iter() {
                    if file_type.categories.is_empty() {
                        top_level_items.push(file_type.clone());
                    } else if file_type.categories.len() == 1 {
                        // Single level category
                        category_tree.entry(file_type.categories[0].clone())
                            .or_insert_with(|| CategoryNode::new(file_type.categories[0].clone()))
                            .items.push(file_type.clone());
                    } else if file_type.categories.len() == 2 {
                        // Two level category
                        let root_cat = file_type.categories[0].clone();
                        let sub_cat = file_type.categories[1].clone();
                        
                        category_tree.entry(root_cat.clone())
                            .or_insert_with(|| CategoryNode::new(root_cat.clone()))
                            .subcategories.entry(sub_cat.clone())
                            .or_insert_with(|| CategoryNode::new(sub_cat))
                            .items.push(file_type.clone());
                    }
                }
                
                // Sort top-level items alphabetically
                top_level_items.sort_by(|a, b| a.display_name.cmp(&b.display_name));
                
                // Add top-level items
                for file_type in top_level_items {
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
                
                // Build category menus
                let mut sorted_categories: Vec<_> = category_tree.into_iter().collect();
                sorted_categories.sort_by(|a, b| a.0.cmp(&b.0));
                
                for (cat_name, mut cat_node) in sorted_categories {
                    // Sort items within category
                    cat_node.items.sort_by(|a, b| a.display_name.cmp(&b.display_name));
                    
                    // Clone subcategories for the closure
                    let subcategories = cat_node.subcategories.clone();
                    
                    submenu = submenu.submenu(&cat_name, window, cx, move |mut cat_submenu, window, cx| {
                        // Add items directly in this category
                        for file_type in &cat_node.items {
                            let icon = Icon::new(file_type.icon.clone()).text_color(file_type.color);
                            
                            cat_submenu = cat_submenu.menu_with_icon(
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
                        
                        // Add subcategories
                        let mut sorted_subcats: Vec<_> = subcategories.clone().into_iter().collect();
                        sorted_subcats.sort_by(|a, b| a.0.cmp(&b.0));
                        
                        for (subcat_name, mut subcat_node) in sorted_subcats {
                            subcat_node.items.sort_by(|a, b| a.display_name.cmp(&b.display_name));
                            
                            cat_submenu = cat_submenu.submenu(&subcat_name, window, cx, move |mut sub_submenu, _window, _cx| {
                                for file_type in &subcat_node.items {
                                    let icon = Icon::new(file_type.icon.clone()).text_color(file_type.color);
                                    
                                    sub_submenu = sub_submenu.menu_with_icon(
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
                                sub_submenu
                            });
                        }
                        
                        cat_submenu
                    });
                }

                submenu
            })
            .separator()
            .menu_with_icon(&t!("FileManager.Cut").to_string(), ui::Icon::new(ui::IconName::Scissor), Box::new(Cut))
            .menu_with_icon(&t!("FileManager.Copy").to_string(), ui::Icon::new(ui::IconName::Copy), Box::new(Copy));

        if has_clipboard {
            menu = menu.menu_with_icon(&t!("FileManager.Paste").to_string(), ui::Icon::new(ui::IconName::PasteClipboard), Box::new(Paste));
        }

        menu = menu
            .separator()
            .menu_with_icon(&t!("FileManager.Rename").to_string(), ui::Icon::new(ui::IconName::EditPencil), Box::new(RenameItem::default()))
            .menu_with_icon(&t!("FileManager.Delete").to_string(), ui::Icon::new(ui::IconName::Trash), Box::new(DeleteItem::default()))
            .separator()
            .menu_with_icon(&t!("FileManager.Duplicate").to_string(), ui::Icon::new(ui::IconName::Copy), Box::new(DuplicateItem::default()))
            .separator()
            .menu_with_icon(&t!("FileManager.OpenInFileManager").to_string(), ui::Icon::new(ui::IconName::ExternalLink), Box::new(OpenInFileManager::default()))
            .menu_with_icon(&t!("FileManager.OpenTerminalHere").to_string(), ui::Icon::new(ui::IconName::Terminal), Box::new(OpenTerminalHere::default()))
            .separator()
            .menu_with_icon(&t!("FileManager.Refresh").to_string(), ui::Icon::new(ui::IconName::Refresh), Box::new(RefreshFileManager));

        menu
    }
}

/// Helper structure for building category tree
#[derive(Clone)]
struct CategoryNode {
    name: String,
    items: Vec<plugin_editor_api::FileTypeDefinition>,
    subcategories: HashMap<String, CategoryNode>,
}

impl CategoryNode {
    fn new(name: String) -> Self {
        Self {
            name,
            items: Vec::new(),
            subcategories: HashMap::new(),
        }
    }
}

/// Build a context menu for files and other items
pub fn item_context_menu(
    path: PathBuf,
    has_clipboard: bool,
    is_class: bool,
) -> impl Fn(ui::popup_menu::PopupMenu, &mut Window, &mut Context<ui::popup_menu::PopupMenu>) -> ui::popup_menu::PopupMenu + 'static {
    // Clone path outside the closure for use in submenu
    let path_for_submenu = path.clone();
    
    move |menu, window, cx| {
        let mut menu = menu;

        // Class-specific actions
        if is_class {
            menu = menu
                .menu_with_icon(&t!("FileManager.Open").to_string(), ui::Icon::new(ui::IconName::BookOpen), Box::new(NewClass::default()))
                .separator();
        }

        menu = menu
            .menu_with_icon(&t!("FileManager.Cut").to_string(), ui::Icon::new(ui::IconName::Scissor), Box::new(Cut))
            .menu_with_icon(&t!("FileManager.Copy").to_string(), ui::Icon::new(ui::IconName::Copy), Box::new(Copy));

        if has_clipboard {
            menu = menu.menu_with_icon(&t!("FileManager.Paste").to_string(), ui::Icon::new(ui::IconName::PasteClipboard), Box::new(Paste));
        }

        menu = menu
            .separator()
            .menu_with_icon(&t!("FileManager.Rename").to_string(), ui::Icon::new(ui::IconName::EditPencil), Box::new(RenameItem::default()))
            .menu_with_icon(&t!("FileManager.Delete").to_string(), ui::Icon::new(ui::IconName::Trash), Box::new(DeleteItem::default()))
            .separator()
            .menu_with_icon(&t!("FileManager.Duplicate").to_string(), ui::Icon::new(ui::IconName::Copy), Box::new(DuplicateItem::default()))
            .separator()
            .menu_with_icon(&t!("FileManager.ValidateAsset").to_string(), ui::Icon::new(ui::IconName::CircleCheck), Box::new(ValidateAsset::default()))
            .menu_with_icon(&t!("FileManager.ToggleFavorite").to_string(), ui::Icon::new(ui::IconName::Star), Box::new(ToggleFavorite::default()))
            .separator();
        
        // Color override submenu
        let submenu_path = path_for_submenu.clone();
        menu = menu.submenu_with_icon(Some(ui::Icon::new(ui::IconName::Palette)), &t!("FileManager.SetColor").to_string(), window, cx, move |submenu, _window, _cx| {
                submenu
                    .menu(&t!("FileManager.ClearColor").to_string(), Box::new(SetColorOverride { 
                        item_path: submenu_path.to_string_lossy().to_string(),
                        color: None,
                    }))
                    .separator()
                    .menu(&"🔴 Red".to_string(), Box::new(SetColorOverride {
                        item_path: submenu_path.to_string_lossy().to_string(),
                        color: Some(ColorData { r: 255, g: 80, b: 80 }),
                    }))
                    .menu(&"🟠 Orange".to_string(), Box::new(SetColorOverride {
                        item_path: submenu_path.to_string_lossy().to_string(),
                        color: Some(ColorData { r: 255, g: 160, b: 80 }),
                    }))
                    .menu(&"🟡 Yellow".to_string(), Box::new(SetColorOverride {
                        item_path: submenu_path.to_string_lossy().to_string(),
                        color: Some(ColorData { r: 255, g: 220, b: 80 }),
                    }))
                    .menu(&"🟢 Green".to_string(), Box::new(SetColorOverride {
                        item_path: submenu_path.to_string_lossy().to_string(),
                        color: Some(ColorData { r: 80, g: 200, b: 120 }),
                    }))
                    .menu(&"🔵 Blue".to_string(), Box::new(SetColorOverride {
                        item_path: submenu_path.to_string_lossy().to_string(),
                        color: Some(ColorData { r: 80, g: 160, b: 255 }),
                    }))
                    .menu(&"🟣 Purple".to_string(), Box::new(SetColorOverride {
                        item_path: submenu_path.to_string_lossy().to_string(),
                        color: Some(ColorData { r: 180, g: 100, b: 255 }),
                    }))
                    .menu(&"🟤 Pink".to_string(), Box::new(SetColorOverride {
                        item_path: submenu_path.to_string_lossy().to_string(),
                        color: Some(ColorData { r: 255, g: 120, b: 180 }),
                    }))
            });
        
        menu = menu
            .menu_with_icon(&t!("FileManager.ToggleGitignore").to_string(), ui::Icon::new(ui::IconName::Gitignore), Box::new(ToggleGitignore::default()))
            .menu_with_icon(&t!("FileManager.ToggleHidden").to_string(), ui::Icon::new(ui::IconName::EyeOff), Box::new(ToggleHidden::default()))
            .separator()
            .menu_with_icon(&t!("FileManager.CheckMultiuserSync").to_string(), ui::Icon::new(ui::IconName::Globe), Box::new(CheckMultiuserSync::default()))
            .menu_with_icon(&t!("FileManager.ShowHistory").to_string(), ui::Icon::new(ui::IconName::Calendar), Box::new(ShowHistory::default()));

        menu
    }
}
