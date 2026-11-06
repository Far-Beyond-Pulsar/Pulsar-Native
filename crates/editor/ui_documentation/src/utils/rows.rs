//! The sidebar tree, as one flat list of rows regardless of where the
//! documentation comes from. Each source turns its own tree into rows here, so
//! `components::sidebar` draws all three the same way.

use gpui::SharedString;
use ui::IconName;

use super::{EngineDocsState, ManualDocsState, ProjectDocsState, ProjectTreeNode, TreeNode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// A crate, category or folder: expands to show what is inside.
    Group,
    /// A section inside a group.
    Section,
    /// A page that can be opened.
    Leaf,
}

/// What clicking a row does. The string is the source's own id for the node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowAction {
    Toggle(String),
    Open(String),
}

#[derive(Clone, Debug)]
pub struct DocRow {
    pub id: SharedString,
    pub depth: usize,
    pub label: SharedString,
    pub icon: IconName,
    pub kind: RowKind,
    pub count: Option<usize>,
    pub expanded: bool,
    pub selected: bool,
    pub action: RowAction,
}

pub fn engine_row_count(state: &EngineDocsState) -> usize {
    state.flat_visible_items.len()
}

pub fn engine_row(state: &EngineDocsState, ix: usize) -> Option<DocRow> {
    let node = state.tree_items.get(*state.flat_visible_items.get(ix)?)?;
    Some(match node {
        TreeNode::Crate { name, depth, .. } => DocRow {
            id: format!("crate-{name}").into(),
            depth: *depth,
            label: name.clone().into(),
            icon: IconName::Package,
            kind: RowKind::Group,
            count: None,
            expanded: state.expanded_paths.contains(name),
            selected: false,
            action: RowAction::Toggle(name.clone()),
        },
        TreeNode::Section {
            crate_name,
            section_name,
            count,
            depth,
        } => {
            let path = format!("{crate_name}/{section_name}");
            DocRow {
                id: format!("section-{path}").into(),
                depth: *depth,
                label: section_name.clone().into(),
                icon: IconName::Folder,
                kind: RowKind::Section,
                count: Some(*count),
                expanded: state.expanded_paths.contains(&path),
                selected: false,
                action: RowAction::Toggle(path),
            }
        }
        TreeNode::Item {
            item_name,
            path,
            depth,
            ..
        } => DocRow {
            id: format!("item-{path}").into(),
            depth: *depth,
            label: item_name.clone().into(),
            icon: IconName::Code,
            kind: RowKind::Leaf,
            count: None,
            expanded: false,
            selected: state.current_path.as_deref() == Some(path.as_str()),
            action: RowAction::Open(path.clone()),
        },
    })
}

pub fn project_row_count(state: &ProjectDocsState) -> usize {
    state.flat_visible_items.len()
}

pub fn project_row(state: &ProjectDocsState, ix: usize) -> Option<DocRow> {
    let node = state.tree_items.get(*state.flat_visible_items.get(ix)?)?;
    Some(match node {
        ProjectTreeNode::Category { name, count, depth } => DocRow {
            id: format!("category-{name}").into(),
            depth: *depth,
            label: name.clone().into(),
            icon: IconName::Folder,
            kind: RowKind::Group,
            count: Some(*count),
            expanded: state.expanded_paths.contains(name),
            selected: false,
            action: RowAction::Toggle(name.clone()),
        },
        ProjectTreeNode::Item {
            item_name,
            path,
            depth,
            ..
        } => DocRow {
            id: format!("project-item-{path}").into(),
            depth: *depth,
            label: item_name.clone().into(),
            icon: IconName::Code,
            kind: RowKind::Leaf,
            count: None,
            expanded: false,
            selected: state.current_path.as_deref() == Some(path.as_str()),
            action: RowAction::Open(path.clone()),
        },
    })
}

pub fn manual_row_count(state: &ManualDocsState) -> usize {
    state.visible_entries.len()
}

pub fn manual_row(state: &ManualDocsState, ix: usize) -> Option<DocRow> {
    let entry = state.file_tree.get(*state.visible_entries.get(ix)?)?;
    let path = entry.path.to_string_lossy().into_owned();
    let expanded = state.expanded_folders.contains(&entry.path);
    Some(if entry.is_directory {
        DocRow {
            id: format!("guide-folder-{path}").into(),
            depth: entry.depth,
            label: entry.name.clone().into(),
            icon: IconName::Folder,
            kind: RowKind::Group,
            count: None,
            expanded,
            selected: false,
            action: RowAction::Toggle(path),
        }
    } else {
        DocRow {
            id: format!("guide-{path}").into(),
            depth: entry.depth,
            label: entry.name.trim_end_matches(".md").to_string().into(),
            icon: IconName::BookOpen,
            kind: RowKind::Leaf,
            count: None,
            expanded: false,
            selected: state.selected_file.as_ref() == Some(&entry.path),
            action: RowAction::Open(path),
        }
    })
}
