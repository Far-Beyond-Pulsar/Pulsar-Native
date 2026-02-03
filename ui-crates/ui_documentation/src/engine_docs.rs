use gpui::{prelude::*, *};
use ui::input::InputState;
use pulsar_docs::{get_doc_content, get_crate_index, list_crates, CrateIndex};
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub enum TreeNode {
    Crate {
        name: String,
        index: CrateIndex,
        depth: usize,
    },
    Section {
        crate_name: String,
        section_name: String,
        count: usize,
        depth: usize,
    },
    Item {
        crate_name: String,
        section_name: String,
        item_name: String,
        path: String,
        doc_summary: Option<String>,
        depth: usize,
    },
}

pub struct EngineDocsState {
    pub tree_items: Vec<TreeNode>,
    pub flat_visible_items: Vec<usize>,
    pub expanded_paths: HashSet<String>,
    pub current_path: Option<String>,
    pub markdown_content: String,
    pub search_query: String,
    pub search_input_state: Entity<InputState>,
}

impl EngineDocsState {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        let search_input_state = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("Search engine docs...", window, cx);
            state
        });

        let mut state = Self {
            tree_items: Vec::new(),
            flat_visible_items: Vec::new(),
            expanded_paths: HashSet::new(),
            current_path: None,
            markdown_content: "# Engine Documentation\n\nSelect an item from the sidebar to view its documentation.".to_string(),
            search_query: String::new(),
            search_input_state,
        };

        state.load_documentation();
        state
    }

    pub fn load_documentation(&mut self) {
        use pulsar_docs::docs_available;

        if !docs_available() {
            self.markdown_content = "# No Documentation Available\n\nDocumentation has not been generated yet. Build in release mode to generate docs.".to_string();
            return;
        }

        self.tree_items.clear();
        let mut crates = list_crates();
        crates.sort();

        for crate_name in crates {
            if let Some(index) = get_crate_index(&crate_name) {
                self.tree_items.push(TreeNode::Crate {
                    name: crate_name.clone(),
                    index: index.clone(),
                    depth: 0,
                });

                let mut sections = index.sections.clone();
                sections.sort_by(|a, b| a.name.cmp(&b.name));

                for section in &sections {
                    self.tree_items.push(TreeNode::Section {
                        crate_name: crate_name.clone(),
                        section_name: section.name.clone(),
                        count: section.count,
                        depth: 1,
                    });

                    let mut items = section.items.clone();
                    items.sort_by(|a, b| a.name.cmp(&b.name));

                    for item in &items {
                        self.tree_items.push(TreeNode::Item {
                            crate_name: crate_name.clone(),
                            section_name: section.name.clone(),
                            item_name: item.name.clone(),
                            path: format!("{}/{}", crate_name, item.path),
                            doc_summary: item.doc_summary.clone(),
                            depth: 2,
                        });
                    }
                }
            }
        }

        self.rebuild_visible_list();
    }

    pub fn rebuild_visible_list(&mut self) {
        self.flat_visible_items.clear();
        let query = self.search_query.to_lowercase();
        let is_searching = !query.is_empty();

        for (idx, node) in self.tree_items.iter().enumerate() {
            match node {
                TreeNode::Crate { name, .. } => {
                    let matches = name.to_lowercase().contains(&query);
                    if !is_searching || matches {
                        self.flat_visible_items.push(idx);
                    }
                }
                TreeNode::Section { crate_name, section_name, .. } => {
                    let matches = section_name.to_lowercase().contains(&query);
                    let parent_expanded = self.expanded_paths.contains(crate_name);

                    if is_searching && matches {
                        self.expanded_paths.insert(crate_name.clone());
                    }

                    if (parent_expanded || (is_searching && matches)) && (!is_searching || matches || parent_expanded) {
                        self.flat_visible_items.push(idx);
                    }
                }
                TreeNode::Item { crate_name, section_name, item_name, .. } => {
                    let section_path = format!("{}/{}", crate_name, section_name);
                    let matches = item_name.to_lowercase().contains(&query);
                    let section_expanded = self.expanded_paths.contains(&section_path);

                    if is_searching && matches {
                        self.expanded_paths.insert(crate_name.clone());
                        self.expanded_paths.insert(section_path.clone());
                    }

                    if (section_expanded || (is_searching && matches)) && (!is_searching || matches) {
                        self.flat_visible_items.push(idx);
                    }
                }
            }
        }

        if is_searching && self.flat_visible_items.is_empty() {
            self.markdown_content = format!(
                "# No Results\n\nNo documentation found matching \"{}\".\n\nTry a different search term.",
                self.search_query
            );
        }
    }

    pub fn toggle_expansion(&mut self, path: String) {
        if self.expanded_paths.contains(&path) {
            self.expanded_paths.remove(&path);

            let child_paths: Vec<String> = self.expanded_paths
                .iter()
                .filter(|p| p.starts_with(&format!("{}/", path)))
                .cloned()
                .collect();

            for child_path in child_paths {
                self.expanded_paths.remove(&child_path);
            }
        } else {
            self.expanded_paths.insert(path);
        }

        self.rebuild_visible_list();
    }

    pub fn load_content(&mut self, path: &str) {
        self.current_path = Some(path.to_string());

        if let Some(markdown) = get_doc_content(path) {
            self.markdown_content = markdown;
        } else {
            self.markdown_content = format!("# Error\n\nFailed to load documentation: {}", path);
        }
    }
}
