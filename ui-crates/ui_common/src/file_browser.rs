//! Unified File Browser Component
//!
//! A reusable file browser that integrates with engine_fs for all file operations.
//! Used by both the file drawer and script editor.

use gpui::*;
use ui::{v_flex, h_flex, ActiveTheme, StyledExt, Sizable, IconName, button::Button};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub enum FileBrowserEvent {
    /// User wants to create a new asset
    CreateAsset {
        kind: engine_fs::AssetKind,
        directory: PathBuf,
    },
    /// User wants to open a file
    OpenFile(PathBuf),
    /// User wants to delete a file
    DeleteFile(PathBuf),
    /// User wants to rename a file
    RenameFile {
        old_path: PathBuf,
        new_path: PathBuf,
    },
    /// User wants to create a new folder
    CreateFolder(PathBuf),
}

pub struct FileBrowser {
    focus_handle: FocusHandle,
    root_path: PathBuf,
    current_path: PathBuf,
    entries: Vec<FileEntry>,
    expanded_dirs: Vec<PathBuf>,
    selected_entry: Option<usize>,
    show_create_menu: bool,
}

#[derive(Clone, Debug)]
struct FileEntry {
    path: PathBuf,
    name: String,
    is_directory: bool,
    icon: String,
    depth: usize,
}

impl FileBrowser {
    pub fn new(root_path: PathBuf, cx: &mut Context<Self>) -> Self {
        let current_path = root_path.clone();
        let mut browser = Self {
            focus_handle: cx.focus_handle(),
            root_path: root_path.clone(),
            current_path,
            entries: Vec::new(),
            expanded_dirs: vec![root_path],
            selected_entry: None,
            show_create_menu: false,
        };
        browser.refresh();
        browser
    }
    
    /// Refresh the file list
    pub fn refresh(&mut self) {
        self.entries.clear();
        let root = self.root_path.clone();
        self.scan_directory(&root, 0);
    }
    
    fn scan_directory(&mut self, dir: &Path, depth: usize) {
        // Check if directory is expanded
        if depth > 0 && !self.expanded_dirs.contains(&dir.to_path_buf()) {
            return;
        }
        
        if let Ok(entries) = std::fs::read_dir(dir) {
            let mut items: Vec<_> = entries.filter_map(|e| e.ok()).collect();
            items.sort_by_key(|e| (!e.path().is_dir(), e.file_name()));
            
            for entry in items {
                let path = entry.path();
                
                // Skip hidden files and target directory
                if let Some(name) = path.file_name() {
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with('.') || name_str == "target" {
                        continue;
                    }
                }
                
                let is_directory = path.is_dir();
                let name = entry.file_name().to_string_lossy().to_string();
                let icon = if is_directory {
                    "📁".to_string()
                } else {
                    Self::get_file_icon(&path)
                };
                
                self.entries.push(FileEntry {
                    path: path.clone(),
                    name,
                    is_directory,
                    icon,
                    depth,
                });
                
                // Recursively scan subdirectories
                if is_directory {
                    self.scan_directory(&path, depth + 1);
                }
            }
        }
    }
    
    fn get_file_icon(path: &Path) -> String {
        if let Some(ext) = path.extension() {
            match ext.to_string_lossy().as_ref() {
                "rs" => "🦀",
                "lua" => "🌙",
                "json" => "📄",
                "toml" => "⚙️",
                "md" => "📝",
                "wgsl" => "✨",
                "db" => "📊",
                ext if ext.contains("alias") => "🔗",
                ext if ext.contains("struct") => "📦",
                ext if ext.contains("enum") => "🎯",
                ext if ext.contains("trait") => "🔧",
                ext if ext.contains("blueprint") => "🔷",
                ext if ext.contains("scene") => "🎬",
                ext if ext.contains("mat") => "🎨",
                _ => "📄",
            }
        } else {
            "📄"
        }.to_string()
    }
    
    fn toggle_directory(&mut self, path: &PathBuf) {
        if let Some(pos) = self.expanded_dirs.iter().position(|p| p == path) {
            self.expanded_dirs.remove(pos);
        } else {
            self.expanded_dirs.push(path.clone());
        }
        self.refresh();
    }
}

impl EventEmitter<FileBrowserEvent> for FileBrowser {}

impl Focusable for FileBrowser {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FileBrowser {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                // Toolbar
                h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .bg(cx.theme().secondary)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("new-asset")
                            .icon(IconName::Plus)
                            .label("New")
                            .xsmall()
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.show_create_menu = !this.show_create_menu;
                                cx.notify();
                            }))
                    )
                    .child(
                        Button::new("refresh")
                            .icon(IconName::Search)
                            .xsmall()
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.refresh();
                                cx.notify();
                            }))
                    )
            )
            .child(
                // File list
                v_flex()
                    .flex_1()
                    .overflow_hidden()
                    .p_2()
                    .scrollable(gpui::Axis::Vertical)
                    .children(
                        self.entries.iter().enumerate().map(|(idx, entry)| {
                            let is_selected = self.selected_entry == Some(idx);
                            let indent = px((entry.depth * 16) as f32);
                            
                            div()
                                .w_full()
                                .pl(indent)
                                .child(
                                    {
                                        let path = entry.path.clone();
                                        let is_dir = entry.is_directory;
                                        let icon = entry.icon.clone();
                                        let name = entry.name.clone();
                                        
                                        let mut row = h_flex()
                                            .w_full()
                                            .px_2()
                                            .py_1()
                                            .gap_2()
                                            .items_center()
                                            .rounded(px(4.0))
                                            .cursor_pointer()
                                            .hover(|style| style.bg(cx.theme().secondary))
                                            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _window, cx| {
                                                this.selected_entry = Some(idx);
                                                if is_dir {
                                                    this.toggle_directory(&path);
                                                } else {
                                                    cx.emit(FileBrowserEvent::OpenFile(path.clone()));
                                                }
                                                cx.notify();
                                            }))
                                            .child(
                                                div()
                                                    .text_base()
                                                    .child(icon)
                                            )
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(cx.theme().foreground)
                                                    .child(name)
                                            );
                                        
                                        if is_selected {
                                            row = row.bg(cx.theme().accent.opacity(0.1));
                                        }
                                        
                                        row
                                    }
                                )
                        })
                    )
            )
            .children(if self.show_create_menu {
                Some(self.render_create_menu(cx))
            } else {
                None
            })
    }
}

impl FileBrowser {
    fn render_create_menu(&self, cx: &App) -> impl IntoElement {
        v_flex()
            .absolute()
            .top(px(50.0))
            .left(px(10.0))
            .w(px(300.0))
            .max_h(px(500.0))
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .rounded(px(8.0))
            .shadow_lg()
            .overflow_hidden()
            .scrollable(gpui::Axis::Vertical)
            .child(
                // Header
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child("Create New Asset")
                    )
            )
            .children(
                engine_fs::AssetCategory::all().iter().map(|category| {
                    v_flex()
                        .w_full()
                        .child(
                            // Category header
                            div()
                                .w_full()
                                .px_3()
                                .py_2()
                                .bg(cx.theme().secondary.opacity(0.5))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(
                                            div()
                                                .text_base()
                                                .child(category.icon())
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_semibold()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(category.display_name())
                                        )
                                )
                        )
                        .children(
                            engine_fs::AssetKind::by_category(*category).iter().map(|kind| {
                                div()
                                    .w_full()
                                    .px_4()
                                    .py_2()
                                    .hover(|this| this.bg(cx.theme().secondary))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_base()
                                                    .child(kind.icon())
                                            )
                                            .child(
                                                v_flex()
                                                    .gap_0()
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .text_color(cx.theme().foreground)
                                                            .child(kind.display_name())
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child(kind.description())
                                                    )
                                            )
                                    )
                            })
                        )
                })
            )
    }
}
