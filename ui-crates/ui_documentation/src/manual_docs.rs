use gpui::{prelude::*, *};
use ui::input::{InputState, TabSize};
use std::path::{Path, PathBuf};
use std::fs;
use std::collections::HashSet;
use regex::Regex;

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_directory: bool,
    pub depth: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewMode {
    Editor,
    Preview,
    Split,
}

pub struct ManualDocsState {
    pub project_root: Option<PathBuf>,
    pub docs_folder: Option<PathBuf>,
    pub file_tree: Vec<FileEntry>,
    pub visible_entries: Vec<usize>,
    pub expanded_folders: HashSet<PathBuf>,
    pub selected_file: Option<PathBuf>,
    pub current_markdown: String,
    pub markdown_preview: String,
    pub editor_input_state: Entity<InputState>,
    pub view_mode: ViewMode,
}

impl ManualDocsState {
    pub fn file_entries(&self) -> Vec<FileEntry> {
        self.visible_entries
            .iter()
            .map(|&idx| self.file_tree[idx].clone())
            .collect()
    }

    pub fn new(window: &mut Window, cx: &mut App, project_root: Option<PathBuf>) -> Self {
        let editor_input_state = cx.new(|cx| {
            // Create code editor for markdown editing
            InputState::new(window, cx)
                .code_editor("markdown")
                .line_number(true)
                .minimap(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
                .soft_wrap(true)
        });

        let mut state = Self {
            project_root: project_root.clone(),
            docs_folder: project_root.as_ref().map(|p| p.join("docs")),
            file_tree: Vec::new(),
            visible_entries: Vec::new(),
            expanded_folders: HashSet::new(),
            selected_file: None,
            current_markdown: String::new(),
            markdown_preview: String::new(),
            editor_input_state,
            view_mode: ViewMode::Split,
        };

        state.load_file_tree();
        state
    }

    pub fn load_file_tree(&mut self) {
        self.file_tree.clear();
        self.visible_entries.clear();

        let docs_folder = match &self.docs_folder {
            Some(folder) => folder.clone(),
            None => return,
        };

        // Create docs folder if it doesn't exist
        if !docs_folder.exists() {
            let _ = fs::create_dir_all(&docs_folder);

            // Create a default README.md
            let readme_path = docs_folder.join("README.md");
            let _ = fs::write(
                &readme_path,
                "# Project Documentation\n\nWelcome to your project documentation!\n\n## Getting Started\n\nAdd your documentation here."
            );
        }

        self.scan_directory(&docs_folder, 0);
        self.rebuild_visible_list();
    }

    fn scan_directory(&mut self, path: &Path, depth: usize) {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };

        let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };

            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden files
            if name.starts_with('.') {
                continue;
            }

            let is_directory = metadata.is_dir();

            // Only show .md files and directories
            if !is_directory && !name.ends_with(".md") {
                continue;
            }

            self.file_tree.push(FileEntry {
                name,
                path: path.clone(),
                is_directory,
                depth,
            });

            if is_directory && self.expanded_folders.contains(&path) {
                self.scan_directory(&path, depth + 1);
            }
        }
    }

    pub fn rebuild_visible_list(&mut self) {
        self.visible_entries.clear();

        for (idx, _) in self.file_tree.iter().enumerate() {
            self.visible_entries.push(idx);
        }
    }

    pub fn toggle_folder(&mut self, path: PathBuf) {
        if self.expanded_folders.contains(&path) {
            self.expanded_folders.remove(&path);
        } else {
            self.expanded_folders.insert(path);
        }

        self.load_file_tree();
    }

    pub fn select_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut App) {
        self.selected_file = Some(path.clone());

        // Load file into editor
        if let Ok(content) = fs::read_to_string(&path) {
            self.editor_input_state.update(cx, |editor, cx| {
                editor.set_value(content.clone(), window, cx);
            });
            self.current_markdown = content.clone();
            self.markdown_preview = self.resolve_image_urls(&content);
        }
    }

    pub fn update_preview(&mut self, cx: &App) {
        // Get content from editor and update preview
        let content = self.editor_input_state.read(cx).value().to_string();
        self.current_markdown = content.clone();
        self.markdown_preview = self.resolve_image_urls(&content);
    }

    fn resolve_image_urls(&self, markdown: &str) -> String {
        // Resolve relative image URLs to absolute file:// URLs
        let Some(selected_file) = &self.selected_file else {
            return markdown.to_string();
        };

        let Some(base_dir) = selected_file.parent() else {
            return markdown.to_string();
        };

        // Regex to match markdown images: ![alt](url)
        let re = Regex::new(r"!\[([^\]]*)\]\(([^)]+)\)").unwrap();

        re.replace_all(markdown, |caps: &regex::Captures| {
            let alt = &caps[1];
            let url = &caps[2];

            // Skip if already absolute URL (http://, https://, file://)
            if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://") {
                return caps[0].to_string();
            }

            // Resolve relative path
            let image_path = base_dir.join(url);
            if let Ok(absolute_path) = image_path.canonicalize() {
                // Convert to file:// URL with proper path format
                #[cfg(target_os = "windows")]
                let file_url = format!("file:///{}", absolute_path.display().to_string().replace("\\", "/"));
                #[cfg(not(target_os = "windows"))]
                let file_url = format!("file://{}", absolute_path.display());

                format!("![{}]({})", alt, file_url)
            } else {
                // If file doesn't exist, keep original
                caps[0].to_string()
            }
        }).to_string()
    }

    pub fn save_current_file(&mut self, _window: &mut Window, cx: &App) -> Result<(), std::io::Error> {
        let Some(path) = &self.selected_file else {
            return Ok(());
        };

        // Get content from editor and save
        let content = self.editor_input_state.read(cx).value().to_string();
        self.current_markdown = content.clone();

        fs::write(path, &self.current_markdown)?;

        // Update preview after save
        self.markdown_preview = self.current_markdown.clone();

        Ok(())
    }

    pub fn create_new_file(&mut self, name: String, window: &mut Window, cx: &mut App) -> Result<(), std::io::Error> {
        let Some(docs_folder) = &self.docs_folder else {
            return Ok(());
        };

        let file_name = if name.ends_with(".md") {
            name.clone()
        } else {
            format!("{}.md", name)
        };

        let file_path = docs_folder.join(&file_name);

        if file_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "File already exists"
            ));
        }

        fs::write(&file_path, format!("# {}\n\n", name.trim_end_matches(".md")))?;
        self.load_file_tree();
        self.select_file(file_path, window, cx);

        Ok(())
    }

    pub fn delete_file(&mut self, path: PathBuf) -> Result<(), std::io::Error> {
        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }

        if self.selected_file.as_ref() == Some(&path) {
            self.selected_file = None;
            self.current_markdown.clear();
            self.markdown_preview.clear();
        }

        self.load_file_tree();
        Ok(())
    }

    pub fn set_view_mode(&mut self, mode: ViewMode) {
        self.view_mode = mode;
    }
}
