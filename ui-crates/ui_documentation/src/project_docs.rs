use gpui::{prelude::*, *};
use ui::{
    input::{InputState, TextInput},
};
use std::path::PathBuf;

pub struct ProjectDocsState {
    pub project_root: Option<PathBuf>,
    pub markdown_content: String,
    pub search_query: String,
    pub search_input_state: Entity<InputState>,
    pub is_loading: bool,
    pub error_message: Option<String>,
}

impl ProjectDocsState {
    pub fn new(window: &mut Window, cx: &mut App, project_root: Option<PathBuf>) -> Self {
        let search_input_state = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("Search project docs...", window, cx);
            state
        });

        let mut state = Self {
            project_root: project_root.clone(),
            markdown_content: "# Project Documentation\n\nLoading project documentation...".to_string(),
            search_query: String::new(),
            search_input_state,
            is_loading: false,
            error_message: None,
        };

        // Parse project documentation if we have a project root
        if let Some(project_path) = project_root {
            state.load_project_docs(&project_path);
        } else {
            state.markdown_content = "# No Project Open\n\nOpen a project to view its documentation.\n\nProject documentation is generated from Rust doc comments (`///` and `//!`).".to_string();
        }

        state
    }

    pub fn load_project_docs(&mut self, project_path: &PathBuf) {
        self.is_loading = true;
        self.error_message = None;

        match pulsar_docs::project_parser::parse_project_docs(project_path) {
            Ok(docs) => {
                self.markdown_content = pulsar_docs::project_parser::generate_markdown(&docs);
                self.is_loading = false;
            }
            Err(e) => {
                self.error_message = Some(e.to_string());
                self.markdown_content = format!(
                    "# Error Loading Project Documentation\n\n**Error:** {}\n\n## Troubleshooting\n\n- Ensure the project has a `src/` directory\n- Check that Rust files are valid and parseable\n- Verify file permissions",
                    e
                );
                self.is_loading = false;
            }
        }
    }

    pub fn refresh(&mut self) {
        if let Some(project_path) = &self.project_root.clone() {
            self.load_project_docs(project_path);
        }
    }
}
