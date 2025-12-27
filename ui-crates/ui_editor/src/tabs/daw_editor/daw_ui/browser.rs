/// Browser Panel Component
/// Studio-quality left sidebar with files, instruments, effects, loops, and samples
/// Beautiful, colorful, and fully interactive

use super::state::*;
use super::panel::DawPanel;
use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::*, h_flex, v_flex, Icon, IconName, Sizable, StyledExt, ActiveTheme,
    Selectable, input::*, divider::Divider, 
    badge::Badge,
};

pub fn render_browser(state: &mut DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    v_flex()
        .size_full()
        .bg(cx.theme().background)
        // Search bar
        .child(render_search_bar(state, cx))
        // Toolbar with actions
        .child(render_browser_toolbar(state, cx))
        // Content area
        .child(render_browser_content(state, cx))
        // Stats footer
        .child(render_browser_footer(state, cx))
}

fn render_search_bar(state: &mut DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    let query = state.search_query.clone();
    let is_empty = query.is_empty();
    let display_text = if is_empty { "Search files...".to_string() } else { query.clone() };

    h_flex()
        .w_full()
        .px_3()
        .py_3()
        .bg(cx.theme().muted.opacity(0.1))
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .w_full()
                .h(px(36.0))
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().background)
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .items_center()
                        .child(Icon::new(IconName::Search).size_4().text_color(cx.theme().muted_foreground))
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(if is_empty {
                                    cx.theme().muted_foreground.opacity(0.6)
                                } else {
                                    cx.theme().foreground
                                })
                                .child(display_text)
                        )
                        .when(!is_empty, |flex| {
                            flex.child(
                                Icon::new(IconName::Close)
                                    .size_3()
                                    .text_color(cx.theme().muted_foreground)
                                    .cursor_pointer()
                            )
                        })
                )
        )
}

fn render_browser_toolbar(state: &mut DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    h_flex()
        .w_full()
        .h(px(44.0))
        .px_3()
        .py_2()
        .gap_2()
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            h_flex()
                .gap_1()
                .child(
                    Button::new("import-audio")
                        .icon(Icon::new(IconName::Plus).size_4())
                        .compact()
                        .small()
                        .tooltip("Import Audio Files")
                        .on_click(cx.listener(|this, _, window, cx| {
                            handle_import_audio(&mut this.state, window, cx);
                        }))
                )
                .child(
                    Button::new("refresh-browser")
                        .icon(Icon::new(IconName::Replace).size_4())
                        .ghost()
                        .compact()
                        .small()
                        .tooltip("Refresh")
                        .on_click(cx.listener(|this, _, _window, cx| {
                            if let Some(dir) = this.state.project_dir.clone() {
                                this.state.scan_audio_files(&dir);
                                cx.notify();
                            }
                        }))
                )
        )
        .child(
            h_flex()
                .gap_1()
                .child(
                    Button::new("browser-list-view")
                        .icon(Icon::new(IconName::LayoutDashboard).size_4())
                        .ghost()
                        .compact()
                        .small()
                        .selected(true)
                        .tooltip("List View")
                )
                .child(
                    Button::new("browser-grid-view")
                        .icon(Icon::new(IconName::GalleryVerticalEnd).size_4())
                        .ghost()
                        .compact()
                        .small()
                        .tooltip("Grid View")
                )
        )
}

fn render_browser_content(state: &mut DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    div()
        .flex_1()
        .overflow_hidden()
        .child(
            div()
                .id("browser-content")
                .w_full()
                .h_full()
                .scrollable(Axis::Vertical)
                .child(match state.browser_tab {
                    BrowserTab::Files => render_files_tab(state, cx).into_any_element(),
                    BrowserTab::Instruments => render_instruments_tab(state, cx).into_any_element(),
                    BrowserTab::Effects => render_effects_tab(state, cx).into_any_element(),
                    BrowserTab::Loops => render_loops_tab(state, cx).into_any_element(),
                    BrowserTab::Samples => render_samples_tab(state, cx).into_any_element(),
                })
        )
}

fn render_files_tab(state: &mut DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_1()
        .p_2()
        // Categories
        .child(render_file_categories(state, cx))
        .child(Divider::horizontal().my_1())
        // File list with colors
        .children(
            state.filtered_files.iter().filter_map(|&idx| {
                state.audio_files.get(idx).map(|file| {
                    render_audio_file_item(file, idx, state, cx)
                })
            })
        )
        .when(state.audio_files.is_empty(), |flex| {
            flex.child(render_empty_state(
                "No Audio Files", 
                "Click + to import WAV, OGG, FLAC, or AIFF files",
                IconName::Inbox,
                cx
            ))
        })
}

fn render_file_categories(state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    let file_count = state.audio_files.len();
    let wav_count = state.audio_files.iter().filter(|f| f.file_type == "WAV").count();
    let ogg_count = state.audio_files.iter().filter(|f| f.file_type == "OGG").count();
    let flac_count = state.audio_files.iter().filter(|f| f.file_type == "FLAC").count();
    
    v_flex()
        .w_full()
        .gap_1()
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().muted_foreground)
                        .child("CATEGORIES")
                )
                .child(
                    Badge::new()
                        .count(file_count)
                        .color(cx.theme().accent)
                        .small()
                )
        )
        .child(render_category_item("All Files", file_count, IconName::FolderOpen, cx.theme().blue, true, cx))
        .child(render_category_item("WAV", wav_count, IconName::Activity, cx.theme().green, false, cx))
        .child(render_category_item("OGG", ogg_count, IconName::Activity, cx.theme().accent, false, cx))
        .child(render_category_item("FLAC", flac_count, IconName::Activity, cx.theme().cyan, false, cx))
}

fn render_category_item(
    label: &'static str,
    count: usize,
    icon: IconName,
    color: Hsla,
    selected: bool,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    div()
        .w_full()
        .px_2()
        .py_1p5()
        .rounded_md()
        .cursor_pointer()
        .when(selected, |d| d.bg(cx.theme().accent.opacity(0.15)))
        .hover(|d| d.bg(cx.theme().accent.opacity(0.1)))
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .justify_between()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Icon::new(icon).size_4().text_color(color))
                        .child(
                            div()
                                .text_sm()
                                .text_color(if selected {
                                    cx.theme().foreground
                                } else {
                                    cx.theme().muted_foreground
                                })
                                .child(label)
                        )
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground.opacity(0.7))
                        .child(format!("{}", count))
                )
        )
}

fn render_audio_file_item(
    file: &AudioFile,
    idx: usize,
    _state: &DawUiState,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    let file_path = file.path.clone();
    let file_name = file.name.clone();
    let file_name_for_closure = file_name.clone(); // Clone for the closure
    let file_type = file.file_type.clone();
    let size_kb = file.size_bytes / 1024;
    
    // Color coding by type  
    let type_color = match file_type.as_str() {
        "WAV" => cx.theme().green,
        "OGG" => cx.theme().accent,
        "FLAC" => cx.theme().cyan,
        "MP3" => cx.theme().blue,
        "AIFF" => cx.theme().yellow,
        _ => cx.theme().muted_foreground,
    };
    
    let is_even = idx % 2 == 0;
    let duration = file.duration_seconds;
    
    div()
        .id(("audio-file", idx))
        .w_full()
        .px_2()
        .py_2()
        .rounded_md()
        .cursor_pointer()
        .when(!is_even, |d| d.bg(cx.theme().muted.opacity(0.05)))
        .hover(|d| d.bg(cx.theme().accent.opacity(0.15)))
        // Handle click to start drag
        .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
            // Set drag state
            tracing::error!("🎵 Starting drag for file: {} at path: {:?}", file_name_for_closure, file_path);
            this.state.drag_state = DragState::DraggingFile {
                file_path: file_path.clone(),
                file_name: file_name_for_closure.clone(),
            };
            cx.notify();
        }))
        .child(
            v_flex()
                .gap_1p5()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .w(px(4.0))
                                .h(px(4.0))
                                .rounded_full()
                                .bg(type_color)
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .font_medium()
                                .text_color(cx.theme().foreground)
                                .child(file_name)
                        )
                        .child(
                            Icon::new(IconName::Ellipsis)
                                .size_4()
                                .text_color(cx.theme().muted_foreground.opacity(0.5))
                        )
                )
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(
                            div()
                                .px_1p5()
                                .py_0p5()
                                .rounded_sm()
                                .bg(type_color.opacity(0.2))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_medium()
                                        .text_color(type_color)
                                        .child(file_type.clone())
                                )
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground.opacity(0.7))
                                .child(format!("{} KB", size_kb))
                        )
                        .when(duration.is_some(), |flex| {
                            let dur = duration.unwrap();
                            let mins = (dur / 60.0) as u32;
                            let secs = (dur % 60.0) as u32;
                            flex.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground.opacity(0.7))
                                    .child(format!("{}:{:02}", mins, secs))
                            )
                        })
                )
        )
}

fn render_instruments_tab(_state: &mut DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    render_empty_state(
        "No Instruments", 
        "Virtual instruments and samplers will appear here",
        IconName::Album,
        cx
    )
}

fn render_effects_tab(_state: &mut DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    render_empty_state(
        "No Effects", 
        "Audio effects and processors will appear here",
        IconName::Activity,
        cx
    )
}

fn render_loops_tab(_state: &mut DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    render_empty_state(
        "No Loops", 
        "Loop files and audio patterns will appear here",
        IconName::AlbumCarousel,
        cx
    )
}

fn render_samples_tab(_state: &mut DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    render_empty_state(
        "No Samples", 
        "One-shot samples and drum hits will appear here",
        IconName::Heart,
        cx
    )
}

fn render_empty_state(
    title: &'static str,
    description: &'static str,
    icon: IconName,
    cx: &mut Context<DawPanel>,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .p_8()
        .gap_3()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(80.0))
                .h(px(80.0))
                .rounded_full()
                .bg(cx.theme().muted.opacity(0.2))
                .child(
                    v_flex()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(icon)
                                .size(px(40.0))
                                .text_color(cx.theme().muted_foreground.opacity(0.4))
                        )
                )
        )
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child(title)
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground.opacity(0.7))
                .text_center()
                .max_w(px(200.0))
                .child(description)
        )
}

fn render_browser_footer(state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    let total_files = state.audio_files.len();
    let total_size_mb = state.audio_files.iter().map(|f| f.size_bytes).sum::<u64>() / (1024 * 1024);

    h_flex()
        .w_full()
        .h(px(36.0))
        .px_3()
        .gap_2()
        .items_center()
        .justify_between()
        .border_t_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().muted.opacity(0.15))
        .child(
            div()
                .text_xs()
                .font_medium()
                .text_color(cx.theme().muted_foreground)
                .child(format!("{} files", total_files))
        )
        .child(
            div()
                .text_xs()
                .font_medium()
                .text_color(cx.theme().muted_foreground)
                .child(format!("{} MB", total_size_mb))
        )
}

// Event handlers

fn handle_import_audio(_state: &mut DawUiState, _window: &mut Window, cx: &mut Context<DawPanel>) {
    cx.spawn(async move |this, cx| {
        let files = rfd::AsyncFileDialog::new()
            .add_filter("Audio Files", &["wav", "mp3", "ogg", "flac", "aiff"])
            .set_title("Import Audio Files")
            .pick_files()
            .await;

        if let Some(files) = files {
            for file in files {
                let source = file.path().to_path_buf();
                
                let _ = cx.update(|cx| {
                    let _ = this.update(cx, |this, cx| {
                        if let Ok(_) = this.state.import_audio_file(source) {
                            tracing::info!("✅ Audio file imported successfully");
                        }
                        cx.notify();
                    });
                });
            }
        }
    })
    .detach();
}
