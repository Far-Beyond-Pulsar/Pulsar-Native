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
}

impl ProjectDocsState {
    pub fn new(window: &mut Window, cx: &mut App, project_root: Option<PathBuf>) -> Self {
        let search_input_state = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("Search project docs...", window, cx);
            state
        });

        Self {
            project_root,
            markdown_content: "# Project Documentation\n\nProject documentation will appear here.\n\nAdd README.md or docs/ folder to your project.".to_string(),
            search_query: String::new(),
            search_input_state,
        }
    }
}
