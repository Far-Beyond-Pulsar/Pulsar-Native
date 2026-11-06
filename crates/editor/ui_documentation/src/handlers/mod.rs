use std::path::PathBuf;

use gpui::*;

use crate::DocumentationWindow;
use crate::utils::{DocCategory, RowAction, ViewMode};

pub fn refresh_current_category(window: &mut DocumentationWindow) {
    match window.current_category {
        DocCategory::Engine => {
            window.engine_docs.tree_items.clear();
            window.engine_docs.expanded_paths.clear();
            window.engine_docs.load_documentation();
        }
        DocCategory::Project => window.project_docs.refresh(),
        DocCategory::Manual => window.manual_docs.load_file_tree(),
    }
}

/// A sidebar row was clicked: expand or collapse it, or open its page.
pub fn activate_row(
    window: &mut DocumentationWindow,
    action: RowAction,
    window_handle: &mut Window,
    cx: &mut App,
) {
    match (window.current_category, action) {
        (DocCategory::Engine, RowAction::Toggle(path)) => window.engine_docs.toggle_expansion(path),
        (DocCategory::Engine, RowAction::Open(path)) => window.engine_docs.load_content(&path),
        (DocCategory::Project, RowAction::Toggle(path)) => {
            window.project_docs.toggle_expansion(path)
        }
        (DocCategory::Project, RowAction::Open(path)) => window.project_docs.load_content(&path),
        (DocCategory::Manual, RowAction::Toggle(path)) => {
            window.manual_docs.toggle_folder(PathBuf::from(path))
        }
        (DocCategory::Manual, RowAction::Open(path)) => {
            window
                .manual_docs
                .select_file(PathBuf::from(path), window_handle, cx)
        }
    }
}

pub fn open_new_file_dialog(window: &mut DocumentationWindow) {
    window.show_new_file_dialog = true;
}

pub fn close_new_file_dialog(window: &mut DocumentationWindow) {
    window.show_new_file_dialog = false;
    window.new_file_name.clear();
}

pub fn create_new_file(window: &mut DocumentationWindow, window_handle: &mut Window, cx: &mut App) {
    let file_name = window.new_file_name.trim().to_string();
    if file_name.is_empty() {
        return;
    }
    if let Err(e) = window
        .manual_docs
        .create_new_file(file_name, window_handle, cx)
    {
        tracing::error!("Failed to create file: {}", e);
        return;
    }
    close_new_file_dialog(window);
}

pub fn save_current_file(window: &mut DocumentationWindow, window_handle: &mut Window, cx: &App) {
    let _ = window.manual_docs.save_current_file(window_handle, cx);
}

pub fn set_view_mode(window: &mut DocumentationWindow, mode: ViewMode) {
    window.manual_docs.set_view_mode(mode);
}

pub fn set_category(window: &mut DocumentationWindow, category: DocCategory) {
    window.current_category = category;
}
