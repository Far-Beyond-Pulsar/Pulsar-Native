use super::palette::{PaletteDelegate, PaletteItem};
use super::{Command, CommandType};
use std::path::PathBuf;
use crate::file_utils::{FileInfo, FileType, find_openable_files};

/// Unified item type that can represent either a command or a file
#[derive(Clone, Debug)]
pub enum CommandOrFile {
    Command(Command),
    File(FileInfo),
}

impl PaletteItem for CommandOrFile {
    fn name(&self) -> &str {
        match self {
            CommandOrFile::Command(cmd) => &cmd.name,
            CommandOrFile::File(file) => &file.name,
        }
    }

    fn description(&self) -> &str {
        match self {
            CommandOrFile::Command(cmd) => &cmd.description,
            CommandOrFile::File(file) => file.path.to_str().unwrap_or(""),
        }
    }

    fn icon(&self) -> ui::IconName {
        match self {
            CommandOrFile::Command(cmd) => cmd.icon.clone(),
            CommandOrFile::File(file) => match file.file_type {
                FileType::Folder => ui::IconName::Folder,
                FileType::Class => ui::IconName::FolderOpen,
                FileType::File => ui::IconName::BookOpen,
            },
        }
    }

    fn keywords(&self) -> Vec<&str> {
        match self {
            CommandOrFile::Command(cmd) => cmd.keywords.iter().map(|s| s.as_str()).collect(),
            CommandOrFile::File(_) => vec![],
        }
    }
}

/// Palette delegate for commands and file search
pub struct CommandDelegate {
    commands: Vec<Command>,
    files: Vec<FileInfo>,
    /// The selected item (for retrieval after confirmation)
    pub selected_item: Option<CommandOrFile>,
}

impl CommandDelegate {
    pub fn new(project_root: Option<PathBuf>) -> Self {
        let commands = Self::default_commands();
        let files = if let Some(root) = project_root {
            find_openable_files(&root, Some(1000))
        } else {
            vec![]
        };

        Self {
            commands,
            files,
            selected_item: None,
        }
    }

    fn default_commands() -> Vec<Command> {
        use ui::IconName;

        vec![
            Command::new(
                "Open Settings",
                "Configure editor preferences",
                IconName::Settings,
                CommandType::OpenSettings,
            ),
            Command::new(
                "Toggle Multiplayer",
                "Show/hide multiplayer panel",
                IconName::Globe,
                CommandType::ToggleMultiplayer,
            ),
            Command::new(
                "Toggle Problems",
                "Show/hide problems panel",
                IconName::TriangleAlert,
                CommandType::ToggleProblems,
            )
            .with_keywords(vec!["errors", "warnings", "diagnostics"]),
            Command::new(
                "Toggle File Manager",
                "Show/hide file explorer",
                IconName::Folder,
                CommandType::ToggleFileManager,
            )
            .with_keywords(vec!["explorer", "files", "tree"]),
            Command::new(
                "Build Project",
                "Compile the current project",
                IconName::Hammer,
                CommandType::BuildProject,
            )
            .with_keywords(vec!["compile", "cargo build"]),
            Command::new(
                "Run Project",
                "Build and run the current project",
                IconName::Play,
                CommandType::RunProject,
            )
            .with_keywords(vec!["execute", "start", "cargo run"]),
            Command::new(
                "Restart Analyzer",
                "Restart the Rust analyzer",
                IconName::Refresh,
                CommandType::RestartAnalyzer,
            )
            .with_keywords(vec!["lsp", "rust-analyzer", "reload"]),
            Command::new(
                "Stop Analyzer",
                "Stop the Rust analyzer",
                IconName::CircleX,
                CommandType::StopAnalyzer,
            )
            .with_keywords(vec!["kill", "rust-analyzer"]),
        ]
    }

    pub fn take_selected_item(&mut self) -> Option<CommandOrFile> {
        self.selected_item.take()
    }
}

impl PaletteDelegate for CommandDelegate {
    type Item = CommandOrFile;

    fn placeholder(&self) -> &str {
        "Type a command or search files..."
    }

    fn categories(&self) -> Vec<(String, Vec<Self::Item>)> {
        let mut categories = vec![];

        // Commands category
        if !self.commands.is_empty() {
            categories.push((
                "Commands".to_string(),
                self.commands.iter().map(|c| CommandOrFile::Command(c.clone())).collect(),
            ));
        }

        // Files category
        if !self.files.is_empty() {
            categories.push((
                "Files".to_string(),
                self.files.iter().map(|f| CommandOrFile::File(f.clone())).collect(),
            ));
        }

        categories
    }

    fn confirm(&mut self, item: &Self::Item) {
        self.selected_item = Some(item.clone());
    }

    fn categories_collapsed_by_default(&self) -> bool {
        false
    }

    fn supports_docs(&self) -> bool {
        false
    }
}
