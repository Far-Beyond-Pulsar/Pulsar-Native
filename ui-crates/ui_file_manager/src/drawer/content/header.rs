use gpui::prelude::*;
use gpui::*;
use std::path::{Path, PathBuf};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
    input::TextInput,
    ActiveTheme as _, Icon, IconName, StyledExt,
};

use crate::drawer::{
    actions::*,
    types::{ViewMode, SortBy, SortOrder},
};

// ============================================================================
// HEADER RENDERING - Breadcrumbs, toolbar, and search
// ============================================================================

/// Render the content area header with breadcrumbs and toolbar
pub fn render_header(
    selected_folder: Option<&PathBuf>,
    project_path: Option<&PathBuf>,
    view_mode: ViewMode,
    sort_by: SortBy,
    sort_order: SortOrder,
    show_hidden_files: bool,
    file_filter_state: &Entity<ui::input::InputState>,
    window: &mut Window,
    cx: &mut AppContext,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_2()
        .child(render_breadcrumbs(selected_folder, project_path, cx))
        .child(h_flex()
            .w_full()
            .gap_2()
            .child(render_toolbar(view_mode, sort_by, sort_order, show_hidden_files, cx))
            .child(render_search_bar(file_filter_state))
        )
}

/// Render breadcrumb navigation
fn render_breadcrumbs(
    selected_folder: Option<&PathBuf>,
    project_path: Option<&PathBuf>,
    cx: &mut AppContext,
) -> impl IntoElement {
    h_flex()
        .gap_1()
        .items_center()
        .child(
            Icon::new(IconName::Folder)
                .size_4()
                .text_color(cx.theme().muted_foreground)
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(
                    selected_folder
                        .and_then(|p| {
                            // Get relative path from project root
                            if let Some(project) = project_path {
                                p.strip_prefix(project).ok()
                                    .and_then(|rel| rel.to_str())
                                    .map(|s| s.to_string())
                            } else {
                                p.to_str().map(|s| s.to_string())
                            }
                        })
                        .unwrap_or_else(|| "No folder selected".to_string())
                )
        )
}

/// Render toolbar with action buttons
fn render_toolbar(
    view_mode: ViewMode,
    sort_by: SortBy,
    sort_order: SortOrder,
    show_hidden_files: bool,
    cx: &mut AppContext,
) -> impl IntoElement {
    h_flex()
        .gap_1()
        .items_center()
        // New item buttons
        .child(
            Button::new("new-folder")
                .icon(IconName::FolderPlus)
                .ghost()
                .tooltip("New Folder")
                .on_click(|_event, window, cx| {
                    cx.dispatch_action(&NewFolder::default());
                })
        )
        .child(
            Button::new("new-file")
                .icon(IconName::PagePlus)
                .ghost()
                .tooltip("New File")
                .on_click(|_event, window, cx| {
                    cx.dispatch_action(&NewFile::default());
                })
        )
        .child(
            Button::new("new-class")
                .icon(IconName::Component)
                .ghost()
                .tooltip("New Class")
                .on_click(|_event, window, cx| {
                    cx.dispatch_action(&NewClass::default());
                })
        )
        .child(
            div()
                .w(px(1.))
                .h_4()
                .bg(cx.theme().border)
        )
        // View mode toggle
        .child(
            Button::new("toggle-view")
                .icon(match view_mode {
                    ViewMode::Grid => IconName::LayoutDashboard,
                    ViewMode::List => IconName::List,
                })
                .ghost()
                .tooltip("Toggle View Mode")
                .on_click(|_event, window, cx| {
                    cx.dispatch_action(&ToggleViewMode);
                })
        )
        // Sort controls
        .child(
            Button::new("sort-name")
                .icon(IconName::SortAscending)
                .ghost()
                .tooltip(format!("Sort by {} ({})",
                    match sort_by {
                        SortBy::Name => "Name",
                        SortBy::Modified => "Modified",
                        SortBy::Size => "Size",
                        SortBy::Type => "Type",
                    },
                    match sort_order {
                        SortOrder::Ascending => "Ascending",
                        SortOrder::Descending => "Descending",
                    }
                ))
                .on_click(|_event, window, cx| {
                    // Cycle through sort modes
                    // TODO: Implement sort cycling action
                })
        )
        .child(
            div()
                .w(px(1.))
                .h_4()
                .bg(cx.theme().border)
        )
        // Show/hide hidden files
        .child(
            Button::new("toggle-hidden")
                .icon(if show_hidden_files { IconName::EyeOff } else { IconName::Eye })
                .ghost()
                .tooltip(if show_hidden_files { "Hide Hidden Files" } else { "Show Hidden Files" })
                .on_click(|_event, window, cx| {
                    cx.dispatch_action(&ToggleHiddenFiles);
                })
        )
        // Refresh button
        .child(
            Button::new("refresh")
                .icon(IconName::Refresh)
                .ghost()
                .tooltip("Refresh")
                .on_click(|_event, window, cx| {
                    cx.dispatch_action(&RefreshFileManager);
                })
        )
}

/// Render search/filter input
fn render_search_bar(
    file_filter_state: &Entity<ui::input::InputState>,
) -> impl IntoElement {
    h_flex()
        .flex_1()
        .justify_end()
        .child(
            TextInput::new(file_filter_state)
                .prefix(Icon::new(IconName::Search).size_4())
                .w(px(200.))
        )
}
