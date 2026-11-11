use crate::utils::doc_source::DocSource;
use gpui::{prelude::*, *};
use regex::Regex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use ui::input::{InputState, TabSize};

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
    /// The text last read from or written to disk; the editor differs from it when unsaved.
    pub saved_markdown: String,
    pub markdown_preview: String,
    pub editor_input_state: Entity<InputState>,
    pub view_mode: ViewMode,
}

impl DocSource for ManualDocsState {
    fn placeholder() -> &'static str {
        ""
    }

    fn initial_content() -> String {
        String::new()
    }
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
            saved_markdown: String::new(),
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

        if !engine_fs::virtual_fs::exists(&docs_folder).unwrap_or(false) {
            let _ = engine_fs::virtual_fs::create_dir_all(&docs_folder);

            let readme_path = docs_folder.join("README.md");
            let _ = engine_fs::virtual_fs::write_file(
                &readme_path,
                b"# Project Documentation\n\nWelcome to your project documentation!\n\n## Getting Started\n\nAdd your documentation here.",
            );
        }

        self.scan_directory(&docs_folder, 0);
        self.rebuild_visible_list();
    }

    fn scan_directory(&mut self, path: &Path, depth: usize) {
        scan_docs_dir(path, depth, &self.expanded_folders, &mut self.file_tree);
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

        if let Ok(content) = engine_fs::virtual_fs::read_to_string(&path) {
            self.editor_input_state.update(cx, |editor, cx| {
                editor.set_value(content.clone(), window, cx);
            });
            self.saved_markdown = content.clone();
            self.current_markdown = content.clone();
            self.markdown_preview = self.resolve_image_urls(&content);
        }
    }

    pub fn update_preview(&mut self, cx: &App) {
        let content = self.editor_input_state.read(cx).value().to_string();
        self.current_markdown = content.clone();
        self.markdown_preview = self.resolve_image_urls(&content);
    }

    fn resolve_image_urls(&self, markdown: &str) -> String {
        rewrite_image_urls(self.selected_file.as_deref(), markdown)
    }

    pub fn save_current_file(
        &mut self,
        _window: &mut Window,
        cx: &App,
    ) -> Result<(), std::io::Error> {
        let Some(path) = &self.selected_file else {
            return Ok(());
        };

        let content = self.editor_input_state.read(cx).value().to_string();
        self.current_markdown = content.clone();

        engine_fs::virtual_fs::write_file(path, self.current_markdown.as_bytes())
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        self.saved_markdown = self.current_markdown.clone();
        self.markdown_preview = self.resolve_image_urls(&self.current_markdown.clone());

        Ok(())
    }

    pub fn create_new_file(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), std::io::Error> {
        let Some(docs_folder) = &self.docs_folder else {
            return Ok(());
        };

        let file_name = markdown_file_name(&name);

        let file_path = docs_folder.join(&file_name);

        if engine_fs::virtual_fs::exists(&file_path).unwrap_or(false) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "File already exists",
            ));
        }

        engine_fs::virtual_fs::write_file(
            &file_path,
            format!("# {}\n\n", name.trim_end_matches(".md")).as_bytes(),
        )
        .map_err(|e| std::io::Error::other(e.to_string()))?;
        self.load_file_tree();
        self.select_file(file_path, window, cx);

        Ok(())
    }

    pub fn delete_file(&mut self, path: PathBuf) -> Result<(), std::io::Error> {
        engine_fs::virtual_fs::delete_path(&path)
            .map_err(|e| std::io::Error::other(e.to_string()))?;

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

impl ManualDocsState {
    /// The open file has edits that are not on disk.
    pub fn is_dirty(&self) -> bool {
        self.selected_file.is_some() && self.current_markdown != self.saved_markdown
    }
}

/// Collect markdown files and directories under `path` (sorted by name, hidden
/// entries skipped); descend only into directories listed in `expanded`.
fn scan_docs_dir(path: &Path, depth: usize, expanded: &HashSet<PathBuf>, out: &mut Vec<FileEntry>) {
    let Ok(mut entries) = engine_fs::virtual_fs::list_dir(path) else {
        return;
    };

    entries.sort_by(|a, b| a.name.cmp(&b.name));

    for entry in entries {
        let path = path.join(&entry.name);
        let name = entry.name;

        if name.starts_with('.') {
            continue;
        }

        let is_directory = entry.is_dir;

        if !is_directory && !name.ends_with(".md") {
            continue;
        }

        out.push(FileEntry {
            name,
            path: path.clone(),
            is_directory,
            depth,
        });

        if is_directory && expanded.contains(&path) {
            scan_docs_dir(&path, depth + 1, expanded, out);
        }
    }
}

/// Name for a new guide: `name` with a `.md` extension appended if missing.
fn markdown_file_name(name: &str) -> String {
    if name.ends_with(".md") {
        name.to_string()
    } else {
        format!("{}.md", name)
    }
}

/// Rewrite relative markdown image links to absolute `file://` URLs (relative
/// to the open file's directory) so the preview can load them. Remote links,
/// existing `file://` links and images that do not exist are left untouched.
fn rewrite_image_urls(selected_file: Option<&Path>, markdown: &str) -> String {
    let Some(selected_file) = selected_file else {
        return markdown.to_string();
    };

    let Some(base_dir) = selected_file.parent() else {
        return markdown.to_string();
    };

    let re = Regex::new(r"!\[([^\]]*)\]\(([^)]+)\)").unwrap();

    re.replace_all(markdown, |caps: &regex::Captures| {
        let alt = &caps[1];
        let url = &caps[2];

        if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://") {
            return caps[0].to_string();
        }

        let image_path = base_dir.join(url);
        if let Ok(absolute_path) = engine_fs::virtual_fs::canonicalize(&image_path) {
            #[cfg(target_os = "windows")]
            let file_url = format!(
                "file:///{}",
                absolute_path.display().to_string().replace("\\", "/")
            );
            #[cfg(not(target_os = "windows"))]
            let file_url = format!("file://{}", absolute_path.display());

            format!("![{}]({})", alt, file_url)
        } else {
            caps[0].to_string()
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui::*` (pulled in by `super::*`) exports a `test` macro that would shadow `#[test]`.
    use core::prelude::v1::test;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ui_documentation_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn markdown_file_name_appends_extension_once() {
        assert_eq!(markdown_file_name("guide"), "guide.md");
        assert_eq!(markdown_file_name("guide.md"), "guide.md");
    }

    #[test]
    fn scan_lists_sorted_markdown_and_dirs_skipping_hidden_and_other_files() {
        let root = temp_dir("scan");
        fs::write(root.join("b.md"), "").unwrap();
        fs::write(root.join("a.md"), "").unwrap();
        fs::write(root.join("notes.txt"), "").unwrap();
        fs::write(root.join(".hidden.md"), "").unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub").join("inner.md"), "").unwrap();

        let mut out = Vec::new();
        scan_docs_dir(&root, 0, &HashSet::new(), &mut out);
        let names: Vec<_> = out
            .iter()
            .map(|e| (e.name.as_str(), e.is_directory, e.depth))
            .collect();
        assert_eq!(
            names,
            vec![("a.md", false, 0), ("b.md", false, 0), ("sub", true, 0)]
        );

        let expanded: HashSet<PathBuf> = [root.join("sub")].into();
        let mut out = Vec::new();
        scan_docs_dir(&root, 0, &expanded, &mut out);
        let names: Vec<_> = out.iter().map(|e| (e.name.as_str(), e.depth)).collect();
        assert_eq!(
            names,
            vec![("a.md", 0), ("b.md", 0), ("sub", 0), ("inner.md", 1)]
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_of_missing_directory_yields_nothing() {
        let mut out = Vec::new();
        scan_docs_dir(
            Path::new("definitely/not/here"),
            0,
            &HashSet::new(),
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn image_rewrite_without_open_file_is_identity() {
        let md = "![x](a.png)";
        assert_eq!(rewrite_image_urls(None, md), md);
    }

    #[test]
    fn image_rewrite_leaves_remote_and_missing_images_alone() {
        let root = temp_dir("img_missing");
        let file = root.join("doc.md");
        let md = "![a](https://x.test/a.png) ![b](http://x.test/b.png) ![c](file:///c.png) ![d](missing.png)";
        assert_eq!(rewrite_image_urls(Some(&file), md), md);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn image_rewrite_makes_existing_relative_images_file_urls() {
        let root = temp_dir("img_exists");
        fs::write(root.join("pic.png"), b"x").unwrap();
        let file = root.join("doc.md");
        let out = rewrite_image_urls(Some(&file), "before ![alt text](pic.png) after");
        assert!(out.starts_with("before ![alt text](file://"), "{out}");
        assert!(out.ends_with("pic.png) after"), "{out}");
        assert!(!out.contains('\\'), "{out}");
        let _ = fs::remove_dir_all(&root);
    }
}
