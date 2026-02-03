use gpui::{prelude::*, *};
use ui::input::InputState;
use std::path::PathBuf;
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub enum ProjectTreeNode {
    Category {
        name: String,
        count: usize,
        depth: usize,
    },
    Item {
        category: String,
        item_name: String,
        path: String,
        depth: usize,
    },
}

pub struct ProjectDocsState {
    pub project_root: Option<PathBuf>,
    pub markdown_content: String,
    pub search_query: String,
    pub search_input_state: Entity<InputState>,
    pub is_loading: bool,
    pub error_message: Option<String>,
    pub tree_items: Vec<ProjectTreeNode>,
    pub flat_visible_items: Vec<usize>,
    pub expanded_paths: HashSet<String>,
    pub current_path: Option<String>,
    pub full_docs: Option<pulsar_docs::project_parser::ProjectDocumentation>,
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
            tree_items: Vec::new(),
            flat_visible_items: Vec::new(),
            expanded_paths: HashSet::new(),
            current_path: None,
            full_docs: None,
        };

        // Try to get project root from engine if not provided
        let project_path = project_root.or_else(|| {
            engine_state::get_project_path().and_then(|p| PathBuf::from(p).parent().map(|parent| parent.to_path_buf()))
        });

        // Parse project documentation if we have a project root
        if let Some(project_path) = project_path {
            state.project_root = Some(project_path.clone());
            state.load_project_docs(&project_path);
        } else {
            state.markdown_content = "# No Project Open\n\nOpen a project to view its documentation.\n\nProject documentation is generated from Rust doc comments (`///` and `//!`).".to_string();
        }

        state
    }

    pub fn load_project_docs(&mut self, project_path: &PathBuf) {
        self.is_loading = true;
        self.error_message = None;
        self.tree_items.clear();
        self.flat_visible_items.clear();

        match pulsar_docs::project_parser::parse_project_docs(project_path) {
            Ok(docs) => {
                // Store full docs for later use
                self.full_docs = Some(docs.clone());

                // Build tree structure - add category followed by its items
                if !docs.structs.is_empty() {
                    self.tree_items.push(ProjectTreeNode::Category {
                        name: "Structs".to_string(),
                        count: docs.structs.len(),
                        depth: 0,
                    });
                    for struct_doc in &docs.structs {
                        let path = if struct_doc.path.is_empty() {
                            struct_doc.name.clone()
                        } else {
                            format!("{}::{}", struct_doc.path.join("::"), struct_doc.name)
                        };
                        self.tree_items.push(ProjectTreeNode::Item {
                            category: "Structs".to_string(),
                            item_name: struct_doc.name.clone(),
                            path: path.clone(),
                            depth: 1,
                        });
                    }
                }

                if !docs.enums.is_empty() {
                    self.tree_items.push(ProjectTreeNode::Category {
                        name: "Enums".to_string(),
                        count: docs.enums.len(),
                        depth: 0,
                    });
                    for enum_doc in &docs.enums {
                        let path = if enum_doc.path.is_empty() {
                            enum_doc.name.clone()
                        } else {
                            format!("{}::{}", enum_doc.path.join("::"), enum_doc.name)
                        };
                        self.tree_items.push(ProjectTreeNode::Item {
                            category: "Enums".to_string(),
                            item_name: enum_doc.name.clone(),
                            path: path.clone(),
                            depth: 1,
                        });
                    }
                }

                if !docs.traits.is_empty() {
                    self.tree_items.push(ProjectTreeNode::Category {
                        name: "Traits".to_string(),
                        count: docs.traits.len(),
                        depth: 0,
                    });
                    for trait_doc in &docs.traits {
                        let path = if trait_doc.path.is_empty() {
                            trait_doc.name.clone()
                        } else {
                            format!("{}::{}", trait_doc.path.join("::"), trait_doc.name)
                        };
                        self.tree_items.push(ProjectTreeNode::Item {
                            category: "Traits".to_string(),
                            item_name: trait_doc.name.clone(),
                            path: path.clone(),
                            depth: 1,
                        });
                    }
                }

                if !docs.functions.is_empty() {
                    self.tree_items.push(ProjectTreeNode::Category {
                        name: "Functions".to_string(),
                        count: docs.functions.len(),
                        depth: 0,
                    });
                    for fn_doc in &docs.functions {
                        let path = if fn_doc.path.is_empty() {
                            fn_doc.name.clone()
                        } else {
                            format!("{}::{}", fn_doc.path.join("::"), fn_doc.name)
                        };
                        self.tree_items.push(ProjectTreeNode::Item {
                            category: "Functions".to_string(),
                            item_name: fn_doc.name.clone(),
                            path: path.clone(),
                            depth: 1,
                        });
                    }
                }

                if !docs.constants.is_empty() {
                    self.tree_items.push(ProjectTreeNode::Category {
                        name: "Constants".to_string(),
                        count: docs.constants.len(),
                        depth: 0,
                    });
                    for const_doc in &docs.constants {
                        let path = if const_doc.path.is_empty() {
                            const_doc.name.clone()
                        } else {
                            format!("{}::{}", const_doc.path.join("::"), const_doc.name)
                        };
                        self.tree_items.push(ProjectTreeNode::Item {
                            category: "Constants".to_string(),
                            item_name: const_doc.name.clone(),
                            path: path.clone(),
                            depth: 1,
                        });
                    }
                }

                self.markdown_content = pulsar_docs::project_parser::generate_markdown(&docs);
                self.rebuild_visible_list();
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

    pub fn rebuild_visible_list(&mut self) {
        self.flat_visible_items.clear();
        let query = self.search_query.to_lowercase();
        let is_searching = !query.is_empty();

        for (idx, node) in self.tree_items.iter().enumerate() {
            match node {
                ProjectTreeNode::Category { name, .. } => {
                    let matches = name.to_lowercase().contains(&query);
                    if !is_searching || matches {
                        self.flat_visible_items.push(idx);
                    }
                }
                ProjectTreeNode::Item { category, item_name, .. } => {
                    let matches = item_name.to_lowercase().contains(&query);
                    let parent_expanded = self.expanded_paths.contains(category);

                    if is_searching && matches {
                        self.expanded_paths.insert(category.clone());
                    }

                    if (parent_expanded || (is_searching && matches)) && (!is_searching || matches) {
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
        } else {
            self.expanded_paths.insert(path);
        }
        self.rebuild_visible_list();
    }

    pub fn load_content(&mut self, path: &str) {
        self.current_path = Some(path.to_string());

        // Generate markdown for the selected item
        if let Some(docs) = &self.full_docs {
            // Try to find the item in each category
            for struct_doc in &docs.structs {
                let item_path = if struct_doc.path.is_empty() {
                    struct_doc.name.clone()
                } else {
                    format!("{}::{}", struct_doc.path.join("::"), struct_doc.name)
                };

                if item_path == path {
                    let mut md = format!("# `{}`\n\n", item_path);
                    md.push_str(&format!("**Type:** Struct\n\n"));
                    md.push_str(&format!("**Visibility:** `{}`\n\n", struct_doc.visibility));

                    if let Some(doc) = &struct_doc.doc_comment {
                        md.push_str("## Documentation\n\n");
                        md.push_str(doc);
                        md.push_str("\n\n");
                    }

                    if !struct_doc.fields.is_empty() {
                        md.push_str("## Fields\n\n");
                        for field in &struct_doc.fields {
                            md.push_str(&format!("### `{}`: `{}`\n\n", field.name, field.ty));
                            md.push_str(&format!("**Visibility:** `{}`\n\n", field.visibility));
                            if let Some(field_doc) = &field.doc_comment {
                                md.push_str(field_doc);
                                md.push_str("\n\n");
                            }
                        }
                    }

                    self.markdown_content = md;
                    return;
                }
            }

            // Check enums
            for enum_doc in &docs.enums {
                let item_path = if enum_doc.path.is_empty() {
                    enum_doc.name.clone()
                } else {
                    format!("{}::{}", enum_doc.path.join("::"), enum_doc.name)
                };

                if item_path == path {
                    let mut md = format!("# `{}`\n\n", item_path);
                    md.push_str(&format!("**Type:** Enum\n\n"));
                    md.push_str(&format!("**Visibility:** `{}`\n\n", enum_doc.visibility));

                    if let Some(doc) = &enum_doc.doc_comment {
                        md.push_str("## Documentation\n\n");
                        md.push_str(doc);
                        md.push_str("\n\n");
                    }

                    if !enum_doc.variants.is_empty() {
                        md.push_str("## Variants\n\n");
                        for variant in &enum_doc.variants {
                            md.push_str(&format!("### `{}`\n\n", variant.name));
                            if let Some(variant_doc) = &variant.doc_comment {
                                md.push_str(variant_doc);
                                md.push_str("\n\n");
                            }
                        }
                    }

                    self.markdown_content = md;
                    return;
                }
            }

            // Check traits
            for trait_doc in &docs.traits {
                let item_path = if trait_doc.path.is_empty() {
                    trait_doc.name.clone()
                } else {
                    format!("{}::{}", trait_doc.path.join("::"), trait_doc.name)
                };

                if item_path == path {
                    let mut md = format!("# `{}`\n\n", item_path);
                    md.push_str(&format!("**Type:** Trait\n\n"));
                    md.push_str(&format!("**Visibility:** `{}`\n\n", trait_doc.visibility));

                    if let Some(doc) = &trait_doc.doc_comment {
                        md.push_str("## Documentation\n\n");
                        md.push_str(doc);
                        md.push_str("\n\n");
                    }

                    if !trait_doc.methods.is_empty() {
                        md.push_str("## Methods\n\n");
                        for method in &trait_doc.methods {
                            md.push_str(&format!("### `{}`\n\n", method.name));
                            md.push_str(&format!("**Signature:** `{}`\n\n", method.signature));
                            if let Some(method_doc) = &method.doc_comment {
                                md.push_str(method_doc);
                                md.push_str("\n\n");
                            }
                        }
                    }

                    self.markdown_content = md;
                    return;
                }
            }

            // Check functions
            for fn_doc in &docs.functions {
                let item_path = if fn_doc.path.is_empty() {
                    fn_doc.name.clone()
                } else {
                    format!("{}::{}", fn_doc.path.join("::"), fn_doc.name)
                };

                if item_path == path {
                    let mut md = format!("# `{}`\n\n", item_path);
                    md.push_str(&format!("**Type:** Function\n\n"));
                    md.push_str(&format!("**Visibility:** `{}`\n\n", fn_doc.visibility));
                    md.push_str(&format!("**Signature:** `{}`\n\n", fn_doc.signature));

                    if let Some(doc) = &fn_doc.doc_comment {
                        md.push_str("## Documentation\n\n");
                        md.push_str(doc);
                        md.push_str("\n\n");
                    }

                    self.markdown_content = md;
                    return;
                }
            }

            // Check constants
            for const_doc in &docs.constants {
                let item_path = if const_doc.path.is_empty() {
                    const_doc.name.clone()
                } else {
                    format!("{}::{}", const_doc.path.join("::"), const_doc.name)
                };

                if item_path == path {
                    let mut md = format!("# `{}`\n\n", item_path);
                    md.push_str(&format!("**Type:** Constant\n\n"));
                    md.push_str(&format!("**Visibility:** `{}`\n\n", const_doc.visibility));
                    md.push_str(&format!("**Type:** `{}`\n\n", const_doc.ty));

                    if let Some(doc) = &const_doc.doc_comment {
                        md.push_str("## Documentation\n\n");
                        md.push_str(doc);
                        md.push_str("\n\n");
                    }

                    self.markdown_content = md;
                    return;
                }
            }
        }
    }

    pub fn refresh(&mut self) {
        if let Some(project_path) = &self.project_root.clone() {
            self.load_project_docs(project_path);
        }
    }
}
