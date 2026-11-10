use crate::utils::doc_source::{DocSource, make_search_input};
use gpui::*;
use pulsar_docs::{CrateIndex, get_crate_index, get_doc_content, list_crates};
use std::collections::HashSet;
use ui::input::InputState;

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

impl DocSource for EngineDocsState {
    fn placeholder() -> &'static str {
        "Search engine docs..."
    }

    fn initial_content() -> String {
        "# Engine Documentation\n\nSelect an item from the sidebar to view its documentation."
            .to_string()
    }
}

impl EngineDocsState {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        let search_input_state = make_search_input::<Self>(window, cx);

        let mut state = Self {
            tree_items: Vec::new(),
            flat_visible_items: Vec::new(),
            expanded_paths: HashSet::new(),
            current_path: None,
            markdown_content: Self::initial_content(),
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
        self.flat_visible_items = visible_nodes(
            &self.tree_items,
            &mut self.expanded_paths,
            &self.search_query,
        );

        if !self.search_query.is_empty() && self.flat_visible_items.is_empty() {
            self.markdown_content = format!(
                "# No Results\n\nNo documentation found matching \"{}\".\n\nTry a different search term.",
                self.search_query
            );
        }
    }

    pub fn toggle_expansion(&mut self, path: String) {
        toggle_path(&mut self.expanded_paths, path);
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

/// Indices of the nodes to show. While searching, matching sections and items
/// are shown and their ancestors are added to `expanded`.
fn visible_nodes(
    tree_items: &[TreeNode],
    expanded: &mut HashSet<String>,
    search_query: &str,
) -> Vec<usize> {
    let mut visible = Vec::new();
    let query = search_query.to_lowercase();
    let is_searching = !query.is_empty();

    for (idx, node) in tree_items.iter().enumerate() {
        match node {
            TreeNode::Crate { name, .. } => {
                let matches = name.to_lowercase().contains(&query);
                if !is_searching || matches {
                    visible.push(idx);
                }
            }
            TreeNode::Section {
                crate_name,
                section_name,
                ..
            } => {
                let matches = section_name.to_lowercase().contains(&query);
                let parent_expanded = expanded.contains(crate_name);

                if is_searching && matches {
                    expanded.insert(crate_name.clone());
                }

                if parent_expanded || (is_searching && matches) {
                    visible.push(idx);
                }
            }
            TreeNode::Item {
                crate_name,
                section_name,
                item_name,
                ..
            } => {
                let section_path = format!("{}/{}", crate_name, section_name);
                let matches = item_name.to_lowercase().contains(&query);
                let section_expanded = expanded.contains(&section_path);

                if is_searching && matches {
                    expanded.insert(crate_name.clone());
                    expanded.insert(section_path.clone());
                }

                if section_expanded || (is_searching && matches) {
                    visible.push(idx);
                }
            }
        }
    }
    visible
}

/// Expand `path` if collapsed; if expanded, collapse it and every descendant.
fn toggle_path(expanded: &mut HashSet<String>, path: String) {
    if expanded.contains(&path) {
        expanded.remove(&path);
        let prefix = format!("{}/", path);
        expanded.retain(|p| !p.starts_with(&prefix));
    } else {
        expanded.insert(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui::*` (pulled in by `super::*`) exports a `test` macro that would shadow `#[test]`.
    use core::prelude::v1::test;
    use pulsar_docs::CrateIndex;

    fn index(name: &str) -> CrateIndex {
        CrateIndex {
            name: name.into(),
            version: "0.0.0".into(),
            description: None,
            sections: Vec::new(),
        }
    }

    /// core { Structs { Vec3 } }, ui { Widgets { Button } }
    fn tree() -> Vec<TreeNode> {
        let section = |c: &str, s: &str| TreeNode::Section {
            crate_name: c.into(),
            section_name: s.into(),
            count: 1,
            depth: 1,
        };
        let item = |c: &str, s: &str, i: &str| TreeNode::Item {
            crate_name: c.into(),
            section_name: s.into(),
            item_name: i.into(),
            path: format!("{c}/{i}"),
            doc_summary: None,
            depth: 2,
        };
        vec![
            TreeNode::Crate {
                name: "core".into(),
                index: index("core"),
                depth: 0,
            },
            section("core", "Structs"),
            item("core", "Structs", "Vec3"),
            TreeNode::Crate {
                name: "ui".into(),
                index: index("ui"),
                depth: 0,
            },
            section("ui", "Widgets"),
            item("ui", "Widgets", "Button"),
        ]
    }

    fn set(paths: &[&str]) -> HashSet<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn collapsed_tree_shows_only_crates() {
        let mut expanded = HashSet::new();
        assert_eq!(visible_nodes(&tree(), &mut expanded, ""), vec![0, 3]);
    }

    #[test]
    fn expansion_reveals_children_level_by_level() {
        let mut expanded = set(&["core"]);
        assert_eq!(visible_nodes(&tree(), &mut expanded, ""), vec![0, 1, 3]);
        let mut expanded = set(&["core", "core/Structs"]);
        assert_eq!(visible_nodes(&tree(), &mut expanded, ""), vec![0, 1, 2, 3]);
    }

    #[test]
    fn search_expands_ancestors_of_matching_items_and_hides_the_rest() {
        let mut expanded = HashSet::new();
        // "button" matches only the item; its crate row does not match so it is hidden,
        // but its ancestors are marked expanded for when the search is cleared.
        let visible = visible_nodes(&tree(), &mut expanded, "BUTTON");
        assert_eq!(visible, vec![5]);
        assert_eq!(expanded, set(&["ui", "ui/Widgets"]));
    }

    #[test]
    fn search_on_crate_name_shows_the_crate_row() {
        let mut expanded = HashSet::new();
        assert_eq!(visible_nodes(&tree(), &mut expanded, "core"), vec![0]);
    }

    #[test]
    fn search_with_no_match_is_empty() {
        let mut expanded = HashSet::new();
        assert!(visible_nodes(&tree(), &mut expanded, "zzz").is_empty());
        assert!(expanded.is_empty());
    }

    #[test]
    fn collapsing_a_path_collapses_descendants_but_not_siblings() {
        let mut expanded = set(&["core", "core/Structs", "core2", "ui"]);
        toggle_path(&mut expanded, "core".into());
        assert_eq!(expanded, set(&["core2", "ui"]));
    }

    #[test]
    fn toggling_a_collapsed_path_expands_it() {
        let mut expanded = HashSet::new();
        toggle_path(&mut expanded, "core".into());
        assert_eq!(expanded, set(&["core"]));
    }
}
