use gpui::{prelude::*, div, px, rgb, App, Context, DismissEvent, Entity, EventEmitter, KeyBinding, MouseButton, Render, SharedString, Window};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, input::{InputState, InputEvent}, input::TextInput, v_flex, ActiveTheme as _, Icon, IconName,
    Sizable as _, StyledExt,
};
use std::sync::Arc;
use std::path::PathBuf;
use super::file_utils::{FileInfo, find_openable_files};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CommandType {
    Files,
    OpenSettings,
    ToggleTerminal,
    ToggleProblems,
    ToggleFileManager,
    BuildProject,
    RunProject,
    RestartAnalyzer,
    StopAnalyzer,
}

#[derive(Clone, Debug)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub icon: IconName,
    pub command_type: CommandType,
    pub keywords: Vec<String>,
}

impl Command {
    fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        icon: IconName,
        command_type: CommandType,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            icon,
            command_type,
            keywords: vec![],
        }
    }

    fn with_keywords(mut self, keywords: Vec<impl Into<String>>) -> Self {
        self.keywords = keywords.into_iter().map(|k| k.into()).collect();
        self
    }

    fn matches(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        
        let query = query.to_lowercase();
        
        // Check name
        if self.name.to_lowercase().contains(&query) {
            return true;
        }
        
        // Check description
        if self.description.to_lowercase().contains(&query) {
            return true;
        }
        
        // Check keywords
        for keyword in &self.keywords {
            if keyword.to_lowercase().contains(&query) {
                return true;
            }
        }
        
        false
    }
}

#[derive(Clone, Debug)]
pub struct CommandSelected {
    pub command: Command,
}

#[derive(Clone, Debug)]
pub struct FileSelected {
    pub path: PathBuf,
}

pub struct CommandPalette {
    pub search_input: Entity<InputState>,
    commands: Vec<Command>,
    filtered_commands: Vec<Command>,
    files: Vec<FileInfo>,
    filtered_files: Vec<FileInfo>,
    selected_index: usize,
    mode: PaletteMode,
    project_root: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum PaletteMode {
    Commands,
    Files,
}

impl EventEmitter<CommandSelected> for CommandPalette {}
impl EventEmitter<FileSelected> for CommandPalette {}
impl EventEmitter<DismissEvent> for CommandPalette {}

impl CommandPalette {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_project(None, window, cx)
    }

    pub fn new_with_project(project_root: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("Type a command or search files...", window, cx);
            state
        });

        let commands = Self::default_commands();
        let filtered_commands = commands.clone();

        // Load files if we have a project root
        let files = if let Some(ref root) = project_root {
            find_openable_files(root, Some(8))
        } else {
            Vec::new()
        };
        let filtered_files = Vec::new();

        // Subscribe to input changes to update the filter
        cx.subscribe(&search_input, |this, _input, event: &InputEvent, cx| {
            match event {
                InputEvent::Change => {
                    let query = this.search_input.read(cx).text().to_string();
                    this.update_filter(&query, cx);
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    this.select_item(cx);
                }
                _ => {}
            }
        }).detach();

        Self {
            search_input,
            commands,
            filtered_commands,
            files,
            filtered_files,
            selected_index: 0,
            mode: PaletteMode::Commands,
            project_root,
        }
    }

    fn default_commands() -> Vec<Command> {
        vec![
            Command::new(
                "Search Files",
                "Search and open files in the project",
                IconName::Search,
                CommandType::Files,
            )
            .with_keywords(vec!["file", "open", "find"]),
            Command::new(
                "Toggle File Manager",
                "Show or hide the file explorer",
                IconName::Folder,
                CommandType::ToggleFileManager,
            )
            .with_keywords(vec!["files", "explorer", "sidebar", "ctrl+b"]),
            Command::new(
                "Toggle Terminal",
                "Show or hide the terminal",
                IconName::Terminal,
                CommandType::ToggleTerminal,
            )
            .with_keywords(vec!["console", "shell", "cmd"]),
            Command::new(
                "Toggle Problems",
                "Show or hide the problems panel",
                IconName::TriangleAlert,
                CommandType::ToggleProblems,
            )
            .with_keywords(vec!["errors", "warnings", "diagnostics"]),
            Command::new(
                "Open Settings",
                "Open application settings",
                IconName::Settings,
                CommandType::OpenSettings,
            )
            .with_keywords(vec!["preferences", "config", "configuration"]),
            Command::new(
                "Build Project",
                "Build the current project",
                IconName::Hammer,
                CommandType::BuildProject,
            )
            .with_keywords(vec!["compile", "make", "cargo build"]),
            Command::new(
                "Run Project",
                "Run the current project",
                IconName::Play,
                CommandType::RunProject,
            )
            .with_keywords(vec!["execute", "start", "cargo run"]),
            Command::new(
                "Restart Rust Analyzer",
                "Restart the Rust language server",
                IconName::Undo,
                CommandType::RestartAnalyzer,
            )
            .with_keywords(vec!["lsp", "language server", "intellisense"]),
            Command::new(
                "Stop Rust Analyzer",
                "Stop the Rust language server",
                IconName::X,
                CommandType::StopAnalyzer,
            )
            .with_keywords(vec!["kill", "terminate"]),
        ]
    }

    fn enter_file_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = PaletteMode::Files;
        self.selected_index = 0;

        // Show all files initially
        self.filtered_files = self.files.clone();
        cx.notify();
    }

    fn update_filter(&mut self, query: &str, _cx: &mut Context<Self>) {
        match self.mode {
            PaletteMode::Commands => {
                if query.is_empty() {
                    self.filtered_commands = self.commands.clone();
                } else {
                    self.filtered_commands = self
                        .commands
                        .iter()
                        .filter(|cmd| cmd.matches(query))
                        .cloned()
                        .collect();
                }
            }
            PaletteMode::Files => {
                use super::file_utils::search_files;
                self.filtered_files = search_files(&self.files, query);
            }
        }
        self.selected_index = 0;
    }

    fn select_item(&mut self, cx: &mut Context<Self>) {
        match self.mode {
            PaletteMode::Commands => {
                if let Some(command) = self.filtered_commands.get(self.selected_index) {
                    // Check if this is the "Search Files" command
                    if command.command_type == CommandType::Files {
                        self.enter_file_mode(cx);
                        return;
                    }

                    cx.emit(CommandSelected {
                        command: command.clone(),
                    });
                }
            }
            PaletteMode::Files => {
                if let Some(file) = self.filtered_files.get(self.selected_index) {
                    cx.emit(FileSelected {
                        path: file.path.clone(),
                    });
                }
            }
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let item_count = match self.mode {
            PaletteMode::Commands => self.filtered_commands.len(),
            PaletteMode::Files => self.filtered_files.len(),
        };

        if item_count == 0 {
            return;
        }

        let new_index = (self.selected_index as isize + delta)
            .rem_euclid(item_count as isize) as usize;

        self.selected_index = new_index;
        cx.notify();
    }
}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_index = self.selected_index;
        
        v_flex()
            .w(px(600.))
            .max_h(px(500.))
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .rounded(px(8.))
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                // Stop propagation to prevent closing the palette when clicking inside
                cx.stop_propagation();
            })
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _window, cx| {
                // Handle navigation keys that should not be processed by the input
                match event.keystroke.key.as_str() {
                    "down" | "arrowdown" => {
                        this.move_selection(1, cx);
                        cx.stop_propagation(); // Prevent input from handling this
                    }
                    "up" | "arrowup" => {
                        this.move_selection(-1, cx);
                        cx.stop_propagation(); // Prevent input from handling this
                    }
                    "escape" => {
                        cx.emit(DismissEvent);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(
                // Search input
                h_flex()
                    .p_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        TextInput::new(&self.search_input)
                            .appearance(false)
                            .bordered(false)
                            .prefix(
                                Icon::new(IconName::Search)
                                    .size(px(18.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .w_full(),
                    ),
            )
            .child(
                // Results list (commands or files based on mode)
                v_flex()
                    .flex_1()
                    .overflow_hidden()
                    .gap_0p5()
                    .p_2()
                    .when(self.mode == PaletteMode::Commands, |this| {
                        this.children(self.filtered_commands.iter().enumerate().map(|(i, cmd)| {
                            let is_selected = i == selected_index;
                            let command = cmd.clone();

                            h_flex()
                                .w_full()
                                .px_3()
                                .py_2()
                                .rounded(px(6.))
                                .gap_3()
                                .items_center()
                                .cursor_pointer()
                                .when(is_selected, |this| {
                                    this.bg(cx.theme().primary.opacity(0.15))
                                })
                                .hover(|s| s.bg(cx.theme().muted.opacity(0.2)))
                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                    this.selected_index = i;
                                    this.select_item(cx);
                                }))
                                .child(
                                    Icon::new(command.icon)
                                        .size(px(20.))
                                        .text_color(if is_selected {
                                            cx.theme().primary
                                        } else {
                                            cx.theme().muted_foreground
                                        }),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .gap_0p5()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .text_color(if is_selected {
                                                    cx.theme().foreground
                                                } else {
                                                    cx.theme().foreground.opacity(0.9)
                                                })
                                                .child(command.name.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(command.description.clone()),
                                        ),
                                )
                        }))
                    })
                    .when(self.mode == PaletteMode::Files, |this| {
                        this.children(self.filtered_files.iter().enumerate().map(|(i, file)| {
                            let is_selected = i == selected_index;
                            let file_info = file.clone();
                            let icon = match file_info.file_type {
                                super::file_utils::FileType::Script => IconName::Code,
                                super::file_utils::FileType::Class => IconName::Box,
                                super::file_utils::FileType::DawProject => IconName::MusicNote,
                                _ => IconName::FileNotFound,
                            };

                            h_flex()
                                .w_full()
                                .px_3()
                                .py_2()
                                .rounded(px(6.))
                                .gap_3()
                                .items_center()
                                .cursor_pointer()
                                .when(is_selected, |this| {
                                    this.bg(cx.theme().primary.opacity(0.15))
                                })
                                .hover(|s| s.bg(cx.theme().muted.opacity(0.2)))
                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                    this.selected_index = i;
                                    this.select_item(cx);
                                }))
                                .child(
                                    Icon::new(icon)
                                        .size(px(20.))
                                        .text_color(if is_selected {
                                            cx.theme().primary
                                        } else {
                                            cx.theme().muted_foreground
                                        }),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .gap_0p5()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .text_color(if is_selected {
                                                    cx.theme().foreground
                                                } else {
                                                    cx.theme().foreground.opacity(0.9)
                                                })
                                                .child(file_info.name.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(file_info.path.to_string_lossy().to_string()),
                                        ),
                                )
                        }))
                    }),
            )
            .when(
                (self.mode == PaletteMode::Commands && self.filtered_commands.is_empty()) ||
                (self.mode == PaletteMode::Files && self.filtered_files.is_empty()),
                |this| {
                this.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p_8()
                        .child(
                            v_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    Icon::new(IconName::Search)
                                        .size(px(48.))
                                        .text_color(cx.theme().muted_foreground.opacity(0.3)),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if self.mode == PaletteMode::Files {
                                            "No files found"
                                        } else {
                                            "No commands found"
                                        }),
                                ),
                        ),
                )
            })
    }
}
