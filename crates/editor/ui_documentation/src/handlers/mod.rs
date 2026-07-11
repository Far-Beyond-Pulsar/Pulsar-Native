use crate::DocumentationWindow;
use crate::utils::{DocCategory, ViewMode};
use gpui::*;

pub fn refresh_current_category(window: &mut DocumentationWindow) {
    match window.current_category {
        DocCategory::Engine => {
            window.engine_docs.tree_items.clear();
            window.engine_docs.expanded_paths.clear();
            window.engine_docs.load_documentation();
        }
        DocCategory::Manual => {
            window.manual_docs.load_file_tree();
        }
        _ => {}
    }
}

pub fn open_new_file_dialog(window: &mut DocumentationWindow) {
    window.show_new_file_dialog = true;
}

pub fn close_new_file_dialog(window: &mut DocumentationWindow) {
    window.show_new_file_dialog = false;
    window.new_file_name.clear();
}

pub fn create_new_file(
    window: &mut DocumentationWindow,
    window_handle: &mut Window,
    cx: &mut App,
) {
    if !window.new_file_name.is_empty() {
        let file_name = window.new_file_name.clone();
        if let Err(e) = window.manual_docs.create_new_file(file_name, window_handle, cx) {
            tracing::error!("Failed to create file: {}", e);
        }
        window.show_new_file_dialog = false;
        window.new_file_name.clear();
    }
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
