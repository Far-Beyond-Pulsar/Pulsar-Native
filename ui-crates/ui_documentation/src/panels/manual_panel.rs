use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, Sizable, StyledExt,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, IconName, Icon,
    text::TextView,
    resizable::{h_resizable, resizable_panel, ResizableState},
    input::TextInput,
    scroll::ScrollbarAxis,
    hierarchical_tree::tree_colors,
};
use ui::render_tree_folder;
use crate::manual_docs::{ManualDocsState, FileEntry, ViewMode};

pub struct ManualDocsPanel;

impl ManualDocsPanel {
    pub fn new() -> Self {
        Self
    }

    pub fn render<V: 'static>(
        &self,
        state: &ManualDocsState,
        sidebar_resizable: Entity<ResizableState>,
        on_new_file: impl Fn(&mut V, &gpui::ClickEvent, &mut Window, &mut Context<V>) + 'static,
        on_save_file: impl Fn(&mut V, &gpui::ClickEvent, &mut Window, &mut Context<V>) + 'static,
        on_mode_change: impl Fn(&mut V, ViewMode, &mut Window, &mut Context<V>) + 'static + Clone,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> impl IntoElement 
    where
        V: Render,
    {
        let visible_files: Vec<_> = state.visible_entries.iter()
            .map(|&idx| state.file_tree[idx].clone())
            .collect();
        
        let file_entries: Vec<AnyElement> = visible_files.into_iter().map(|entry| {
            Self::render_file_entry(&entry, state, cx)
        }).collect();
        
        let theme = cx.theme().clone();
        let has_selection = state.selected_file.is_some();
        let file_name = state.selected_file
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|s| s.to_string());

        h_resizable("docs-horizontal", sidebar_resizable)
            .child(
                resizable_panel()
                    .size(px(260.0))
                    .child(Self::render_sidebar(file_entries, &theme, cx, on_new_file))
            )
            .child(
                resizable_panel()
                    .child(Self::render_editor_area(
                        state,
                        has_selection,
                        file_name,
                        on_save_file,
                        on_mode_change,
                        window,
                        &theme,
                        cx,
                    ))
            )
    }

    fn render_sidebar<V: 'static>(
        file_entries: Vec<AnyElement>,
        theme: &ui::ThemeColor,
        cx: &mut Context<V>,
        on_new_file: impl Fn(&mut V, &gpui::ClickEvent, &mut Window, &mut Context<V>) + 'static,
    ) -> impl IntoElement 
    where
        V: Render,
    {
        let file_count = file_entries.len();
        
        v_flex()
            .size_full()
            .bg(theme.sidebar.opacity(0.95))
            .border_r_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .w_full()
                    .h(px(48.0))
                    .items_center()
                    .justify_between()
                    .px_4()
                    .bg(theme.sidebar)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Icon::new(IconName::BookOpen)
                                    .size_4()
                                    .text_color(tree_colors::DOC_TEAL)
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.foreground)
                                    .child("Files")
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py(px(3.0))
                                    .rounded(px(6.0))
                                    .bg(theme.accent.opacity(0.12))
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.accent)
                                    .child(format!("{}", file_count))
                            )
                    )
                    .child(
                        Button::new("new-file")
                            .icon(IconName::Plus)
                            .ghost()
                            .small()
                            .tooltip("New File")
                            .on_click(cx.listener(on_new_file))
                    )
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .size_full()
                            .py_2()
                            .scrollable(ScrollbarAxis::Vertical)
                            .children(file_entries)
                    )
            )
    }

    fn render_editor_area<V: 'static>(
        state: &ManualDocsState,
        has_selection: bool,
        file_name: Option<String>,
        on_save_file: impl Fn(&mut V, &gpui::ClickEvent, &mut Window, &mut Context<V>) + 'static,
        on_mode_change: impl Fn(&mut V, ViewMode, &mut Window, &mut Context<V>) + 'static + Clone,
        window: &mut Window,
        theme: &ui::ThemeColor,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: Render,
    {
        let on_editor_mode = on_mode_change.clone();
        let on_preview_mode = on_mode_change.clone();
        let on_split_mode = on_mode_change;

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                // Modern toolbar with segmented control
                h_flex()
                    .w_full()
                    .h(px(52.0))
                    .items_center()
                    .justify_between()
                    .px_4()
                    .gap_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .bg(theme.sidebar.opacity(0.5))
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .when(has_selection, |this| {
                                this.child(
                                    Icon::new(IconName::BookOpen)
                                        .size_4()
                                        .text_color(theme.accent)
                                )
                            })
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(if has_selection {
                                        theme.foreground
                                    } else {
                                        theme.muted_foreground
                                    })
                                    .child(file_name.unwrap_or_else(|| "No file selected".to_string()))
                            )
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                // Segmented control for view modes
                                h_flex()
                                    .gap_px()
                                    .p_px()
                                    .rounded_lg()
                                    .bg(theme.muted.opacity(0.3))
                                    .child(
                                        Button::new("mode-editor")
                                            .label("Edit")
                                            .icon(IconName::Code)
                                            .xsmall()
                                            .when(state.view_mode == ViewMode::Editor, |btn| {
                                                btn.bg(theme.accent)
                                                    .text_color(theme.accent_foreground)
                                            })
                                            .when(state.view_mode != ViewMode::Editor, |btn| btn.ghost())
                                            .on_click(cx.listener(move |view, _event, window, cx| {
                                                on_editor_mode(view, ViewMode::Editor, window, cx);
                                            }))
                                    )
                                    .child(
                                        Button::new("mode-preview")
                                            .label("Preview")
                                            .icon(IconName::Eye)
                                            .xsmall()
                                            .when(state.view_mode == ViewMode::Preview, |btn| {
                                                btn.bg(theme.accent)
                                                    .text_color(theme.accent_foreground)
                                            })
                                            .when(state.view_mode != ViewMode::Preview, |btn| btn.ghost())
                                            .on_click(cx.listener(move |view, _event, window, cx| {
                                                on_preview_mode(view, ViewMode::Preview, window, cx);
                                            }))
                                    )
                                    .child(
                                        Button::new("mode-split")
                                            .label("Split")
                                            .icon(IconName::SplitARea)
                                            .xsmall()
                                            .when(state.view_mode == ViewMode::Split, |btn| {
                                                btn.bg(theme.accent)
                                                    .text_color(theme.accent_foreground)
                                            })
                                            .when(state.view_mode != ViewMode::Split, |btn| btn.ghost())
                                            .on_click(cx.listener(move |view, _event, window, cx| {
                                                on_split_mode(view, ViewMode::Split, window, cx);
                                            }))
                                    )
                            )
                            .child(
                                div()
                                    .h_6()
                                    .w_px()
                                    .bg(theme.border.opacity(0.4))
                            )
                            .child(
                                Button::new("save-file")
                                    .icon(IconName::Check)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Save File")
                                    .on_click(cx.listener(on_save_file))
                            )
                    )
            )
            .child(
                Self::render_view_mode_content(state, window, cx, theme)
            )
    }

    fn render_view_mode_content<V: 'static>(
        state: &ManualDocsState,
        window: &mut Window,
        cx: &mut Context<V>,
        theme: &ui::ThemeColor,
    ) -> impl IntoElement
    where
        V: Render,
    {
        let view_mode = state.view_mode;
        let markdown = state.markdown_preview.clone();
        let editor_state = state.editor_input_state.clone();

        div()
            .flex_1()
            .overflow_hidden()
            .child(match view_mode {
                ViewMode::Editor => {
                    div()
                        .size_full()
                        .bg(theme.background)
                        .p_4()
                        .child(
                            TextInput::new(&editor_state)
                                .w_full()
                                .h_full()
                                .appearance(true)
                                .bordered(true)
                        )
                        .into_any_element()
                }
                ViewMode::Preview => {
                    div()
                        .size_full()
                        .bg(theme.background)
                        .overflow_hidden()
                        .child(
                            div()
                                .size_full()
                                .scrollable(ScrollbarAxis::Vertical)
                                .child(
                                    div()
                                        .w_full()
                                        .max_w(px(1200.0))
                                        .mx_auto()
                                        .px_8()
                                        .py_8()
                                        .child(
                                            TextView::markdown(
                                                "manual-docs-preview",
                                                markdown.clone(),
                                                window,
                                                cx,
                                            )
                                            .debounce_ms(30)
                                            .selectable()
                                        )
                                )
                        )
                        .into_any_element()
                }
                ViewMode::Split => {
                    h_flex()
                        .size_full()
                        .child(
                            div()
                                .flex_1()
                                .size_full()
                                .bg(theme.background)
                                .border_r_1()
                                .border_color(theme.border)
                                .p_3()
                                .child(
                                    TextInput::new(&editor_state)
                                        .w_full()
                                        .h_full()
                                        .appearance(true)
                                        .bordered(true)
                                )
                        )
                        .child(
                            div()
                                .flex_1()
                                .size_full()
                                .bg(theme.background)
                                .overflow_hidden()
                                .child(
                                    div()
                                        .size_full()
                                        .scrollable(ScrollbarAxis::Vertical)
                                        .child(
                                            div()
                                                .w_full()
                                                .max_w(px(900.0))
                                                .mx_auto()
                                                .px_6()
                                                .py_6()
                                                .child(
                                                    TextView::markdown(
                                                        "manual-docs-preview-split",
                                                        markdown,
                                                        window,
                                                        cx,
                                                    )
                                                    .debounce_ms(30)
                                                    .selectable()
                                                )
                                        )
                                )
                        )
                        .into_any_element()
                }
            })
    }

    fn render_file_entry<V: 'static>(
        entry: &FileEntry,
        state: &ManualDocsState,
        cx: &mut Context<V>,
    ) -> AnyElement
    where
        V: Render,
    {
        let is_selected = state.selected_file.as_ref() == Some(&entry.path);
        let is_expanded = state.expanded_folders.contains(&entry.path);
        
        // Use render_tree_folder for directories
        if entry.is_directory {
            let icon = if is_expanded { IconName::FolderOpen } else { IconName::Folder };
            return render_tree_folder(
                &format!("doc-folder-{:#?}", entry.path),
                &entry.name,
                icon,
                tree_colors::FOLDER,
                entry.depth,
                is_expanded,
                |_view, _event, _window, _cx| {
                    // Folder click handled elsewhere
                },
                cx,
            );
        }
        
        // For files, render custom since they have different styling needs
        let theme = cx.theme();
        let indent = px(entry.depth as f32 * 16.0);

        div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(32.0))
            .pl(indent + px(12.0))
            .pr_3()
            .mx_2()
            .rounded(px(6.0))
            .when(is_selected, |style| {
                style
                    .bg(theme.accent)
                    .shadow_sm()
            })
            .when(!is_selected, |style| {
                style.hover(|s| s.bg(theme.accent.opacity(0.1)))
            })
            .cursor_pointer()
            .child(
                Icon::new(IconName::BookOpen)
                    .size_4()
                    .text_color(if is_selected {
                        theme.accent_foreground
                    } else {
                        tree_colors::DOC_TEAL
                    })
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::NORMAL)
                    .text_color(if is_selected {
                        theme.accent_foreground
                    } else {
                        theme.foreground
                    })
                    .child(entry.name.clone())
            )
            .into_any_element()
    }
}
