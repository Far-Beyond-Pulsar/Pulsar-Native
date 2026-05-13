//! Full-featured code editor backed by InputState — syntax highlighting, virtualization,
//! minimap, find/replace, LSP diagnostics, undo/redo, and all keyboard shortcuts.

use anyhow::Result;
use gpui::{prelude::FluentBuilder as _, *};
use std::path::PathBuf;

use crate::{
    h_flex,
    input::{InputEvent, InputState, TabSize, TextInput},
    v_flex, ActiveTheme,
};

#[derive(Clone)]
pub enum CodeEditorEvent {
    Changed { content: String },
    Saved { path: PathBuf, content: String },
}

/// Language detection from file extension.
fn detect_language(path: &PathBuf) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "rs" => "rust",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "jsx" => "jsx",
        "tsx" => "tsx",
        "py" => "python",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" => "cpp",
        "java" => "java",
        "cs" => "c_sharp",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "json" | "jsonc" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "xml" | "svg" | "xhtml" => "xml",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" => "scss",
        "sh" | "bash" | "zsh" => "bash",
        "md" | "mdx" => "markdown",
        "sql" => "sql",
        "lua" => "lua",
        "r" => "r",
        "ex" | "exs" => "elixir",
        "hs" => "haskell",
        "nix" => "nix",
        "dockerfile" => "dockerfile",
        _ => "plaintext",
    }
}

pub struct CodeEditor {
    input: Entity<InputState>,
    path: Option<PathBuf>,
    language: SharedString,
    is_modified: bool,
    _subscriptions: Vec<Subscription>,
}

impl CodeEditor {
    /// Create a new CodeEditor with the given language for syntax highlighting.
    ///
    /// The editor includes: virtual scrolling, syntax highlighting, line numbers,
    /// minimap, find/replace (Cmd/Ctrl+F), full keyboard shortcuts, LSP diagnostics,
    /// undo/redo, and all standard editor features.
    pub fn new(
        language: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let language: SharedString = language.into();

        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor(language.clone())
                .minimap(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
                .line_number(true)
        });

        let _subscriptions = vec![cx.subscribe_in(
            &input,
            window,
            |this: &mut CodeEditor, _, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.is_modified = true;
                    let content = this.input.read(cx).value().to_string();
                    cx.emit(CodeEditorEvent::Changed { content });
                }
            },
        )];

        Self {
            input,
            path: None,
            language,
            is_modified: false,
            _subscriptions,
        }
    }

    /// Set the text content of the editor.
    pub fn set_text(
        &mut self,
        content: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = content.into();
        self.input.update(cx, |state, cx| {
            state.set_value(content, window, cx);
        });
        self.is_modified = false;
        cx.notify();
    }

    /// Load a file from disk, auto-detecting language from extension.
    pub fn load_file(
        &mut self,
        path: impl Into<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let path = path.into();
        let content: SharedString = std::fs::read_to_string(&path)?.into();
        let lang = detect_language(&path);
        self.input.update(cx, |state, cx| {
            state.set_highlighter(lang, cx);
            state.set_value(content.clone(), window, cx);
        });
        self.language = lang.into();
        self.path = Some(path);
        self.is_modified = false;
        cx.notify();
        Ok(())
    }

    /// Save the current content to disk (requires a path — set via `load_file` or `set_path`).
    pub fn save(&mut self, cx: &mut Context<Self>) -> Result<()> {
        if let Some(path) = self.path.clone() {
            let content = self.input.read(cx).value().to_string();
            std::fs::write(&path, &content)?;
            self.is_modified = false;
            cx.emit(CodeEditorEvent::Saved { path, content });
            cx.notify();
        }
        Ok(())
    }

    /// Set the file path (for save-to-disk). Does not load content.
    pub fn set_path(&mut self, path: impl Into<PathBuf>) {
        self.path = Some(path.into());
    }

    /// Change the active syntax highlighting language.
    pub fn set_language(&mut self, language: impl Into<SharedString>, cx: &mut Context<Self>) {
        let language = language.into();
        self.language = language.clone();
        self.input.update(cx, |state, cx| {
            state.set_highlighter(language, cx);
        });
    }

    /// Get the full text content.
    pub fn content(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// Whether the editor content has unsaved changes.
    pub fn is_modified(&self) -> bool {
        self.is_modified
    }

    /// Expose the underlying InputState entity for advanced usage (LSP, diagnostics, etc.).
    pub fn input_state(&self) -> &Entity<InputState> {
        &self.input
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.input.read(cx);
        let total_lines = state.text().len_lines(ropey::LineType::LF);
        let pos = state.cursor_position();
        let line = pos.line + 1;
        let col = pos.character + 1;
        let lang = self.language.clone();
        let modified = self.is_modified;
        let path_label: SharedString = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|s| s.to_string().into())
            .unwrap_or_else(|| "Untitled".into());

        h_flex()
            .w_full()
            .h(px(24.0))
            .px_3()
            .bg(cx.theme().secondary)
            .border_t_1()
            .border_color(cx.theme().border)
            .justify_between()
            .items_center()
            .text_xs()
            .text_color(cx.theme().secondary_foreground)
            .child(
                h_flex()
                    .gap_3()
                    .child(path_label)
                    .when(modified, |this: gpui::Div| this.child("●")),
            )
            .child(
                h_flex()
                    .gap_4()
                    .child(format!("Ln {line}, Col {col}"))
                    .child(format!("{total_lines} lines"))
                    .child(lang)
                    .child("UTF-8"),
            )
    }
}

impl EventEmitter<CodeEditorEvent> for CodeEditor {}

impl Focusable for CodeEditor {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl Render for CodeEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(TextInput::new(&self.input).h_full().appearance(false)),
            )
            .child(self.render_status_bar(cx))
    }
}
