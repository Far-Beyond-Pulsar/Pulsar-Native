//! Workspace panels for Script Editor

use gpui::*;
use ui::{ActiveTheme, StyledExt, dock::{Panel, PanelEvent}, v_flex, input::InputState};
use std::path::PathBuf;
use super::text_editor::TextEditor;
use super::file_explorer::FileExplorer;

/// Individual File Panel - wraps a single file's editor
pub struct FilePanelWrapper {
    text_editor: WeakEntity<TextEditor>,
    file_index: usize,
    file_path: PathBuf,
    input_state: Entity<InputState>,
    focus_handle: FocusHandle,
}

impl FilePanelWrapper {
    pub fn new(
        text_editor: WeakEntity<TextEditor>,
        file_index: usize,
        file_path: PathBuf,
        input_state: Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            text_editor,
            file_index,
            file_path,
            input_state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for FilePanelWrapper {}

impl Render for FilePanelWrapper {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(
                ui::input::TextInput::new(&self.input_state)
                    .h_full()
                    .w_full()
                    .font_family("monospace")
                    .font(gpui::Font {
                        family: "Jetbrains Mono".to_string().into(),
                        weight: gpui::FontWeight::NORMAL,
                        style: gpui::FontStyle::Normal,
                        features: gpui::FontFeatures::default(),
                        fallbacks: Some(gpui::FontFallbacks::from_fonts(vec!["monospace".to_string()])),
                    })
                    .text_size(px(14.0))
            )
    }
}

impl Focusable for FilePanelWrapper {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for FilePanelWrapper {
    fn panel_name(&self) -> &'static str {
        "file"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        let filename = self.file_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled")
            .to_string();
        filename.into_any_element()
    }
}

/// Welcome Panel - shown when no files are open
pub struct WelcomePanelWrapper {
    focus_handle: FocusHandle,
}

impl WelcomePanelWrapper {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for WelcomePanelWrapper {}

impl Render for WelcomePanelWrapper {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .child(
                div()
                    .text_xl()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(cx.theme().foreground)
                    .child("Welcome to Script Editor")
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Open a file from the explorer to start editing")
            )
    }
}

impl Focusable for WelcomePanelWrapper {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for WelcomePanelWrapper {
    fn panel_name(&self) -> &'static str {
        "welcome"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Welcome".into_any_element()
    }
}

/// File Explorer Panel
pub struct FileExplorerPanel {
    file_explorer: Entity<FileExplorer>,
    focus_handle: FocusHandle,
}

impl FileExplorerPanel {
    pub fn new(file_explorer: Entity<FileExplorer>, cx: &mut Context<Self>) -> Self {
        Self {
            file_explorer,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for FileExplorerPanel {}

impl Render for FileExplorerPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(self.file_explorer.clone())
    }
}

impl Focusable for FileExplorerPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for FileExplorerPanel {
    fn panel_name(&self) -> &'static str {
        "file_explorer"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Files".into_any_element()
    }
}
