//! What the sidebar shows, independent of how it is drawn.
//!
//! Open editors are listed in sections: tabs the user pinned first, then the
//! rest grouped by the kind of editor (every blueprint together, and so on).
//! A pinned file stays listed after its tab closes, so it reopens in one
//! click. Below them is the project's folder tree.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ui::IconName;
use ui_file_manager::FolderNode;

/// Identifies a tab across sessions: the file it edits, or for a panel with no
/// file (the level editor) its panel name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum TabKey {
    File(PathBuf),
    Panel(String),
}

impl TabKey {
    pub fn of(panel_name: &str, file: Option<&Path>) -> Self {
        match file {
            Some(path) => Self::File(path.to_path_buf()),
            None => Self::Panel(panel_name.to_string()),
        }
    }
}

/// One row in the editor list.
#[derive(Clone, Debug)]
pub struct SidebarTab {
    pub key: TabKey,
    /// Position among all open tabs, as `activate_open_editor_by_global_index`
    /// counts them. `None` for a pinned file whose tab is closed.
    pub index: Option<usize>,
    pub title: String,
    /// The editor's panel name; tabs are grouped by it.
    pub kind: String,
    pub icon: Option<IconName>,
    pub active: bool,
    pub unsaved: bool,
}

impl SidebarTab {
    pub fn is_open(&self) -> bool {
        self.index.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SectionId {
    Pinned,
    Group(String),
}

impl SectionId {
    /// The name `collapsed_groups` stores.
    fn storage_name(&self) -> String {
        match self {
            Self::Pinned => PINNED_SECTION.to_string(),
            Self::Group(kind) => kind.clone(),
        }
    }
}

const PINNED_SECTION: &str = "__pinned__";

#[derive(Clone, Debug)]
pub struct TabSection {
    pub id: SectionId,
    pub label: String,
    pub tabs: Vec<SidebarTab>,
    pub collapsed: bool,
}

/// A folder row of the content tree, in display order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderRow {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub has_children: bool,
    pub expanded: bool,
}

/// What a project saves about its sidebar, in `.pulsar/layout.json`. Paths
/// inside the project are stored relative to it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedSidebar {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<TabKey>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collapsed_groups: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expanded_folders: Vec<PathBuf>,
}

impl SavedSidebar {
    pub fn is_empty(&self) -> bool {
        self.pinned.is_empty()
            && self.collapsed_groups.is_empty()
            && self.expanded_folders.is_empty()
    }
}

/// Folders the tree never lists: build output, not content.
const SKIPPED_FOLDERS: &[&str] = &["target"];

#[derive(Debug, Default)]
pub struct SidebarModel {
    /// In the order they were pinned.
    pinned: Vec<TabKey>,
    collapsed: BTreeSet<String>,
    expanded_folders: BTreeSet<PathBuf>,
}

impl SidebarModel {
    /// The editor list for `open` (all open tabs, in tab order).
    pub fn sections(&self, open: &[SidebarTab]) -> Vec<TabSection> {
        let mut sections = Vec::new();

        let pinned: Vec<SidebarTab> = self
            .pinned
            .iter()
            .filter_map(|key| match open.iter().find(|tab| &tab.key == key) {
                Some(tab) => Some(tab.clone()),
                // A closed panel can't be reopened from here; a closed file can.
                None => match key {
                    TabKey::File(path) => Some(closed_file(path)),
                    TabKey::Panel(_) => None,
                },
            })
            .collect();
        if !pinned.is_empty() {
            sections.push(self.section(SectionId::Pinned, "Pinned".into(), pinned));
        }

        let mut groups: Vec<(String, Vec<SidebarTab>)> = Vec::new();
        for tab in open.iter().filter(|tab| !self.is_pinned(&tab.key)) {
            match groups.iter_mut().find(|(kind, _)| *kind == tab.kind) {
                Some((_, tabs)) => tabs.push(tab.clone()),
                None => groups.push((tab.kind.clone(), vec![tab.clone()])),
            }
        }
        for (kind, tabs) in groups {
            let label = group_label(&kind);
            sections.push(self.section(SectionId::Group(kind), label, tabs));
        }
        sections
    }

    fn section(&self, id: SectionId, label: String, tabs: Vec<SidebarTab>) -> TabSection {
        let collapsed = self.collapsed.contains(&id.storage_name());
        TabSection {
            id,
            label,
            tabs,
            collapsed,
        }
    }

    pub fn is_pinned(&self, key: &TabKey) -> bool {
        self.pinned.contains(key)
    }

    /// Pin or unpin; returns whether `key` is pinned now.
    pub fn toggle_pin(&mut self, key: &TabKey) -> bool {
        if let Some(ix) = self.pinned.iter().position(|k| k == key) {
            self.pinned.remove(ix);
            false
        } else {
            self.pinned.push(key.clone());
            true
        }
    }

    pub fn toggle_section(&mut self, id: &SectionId) {
        let name = id.storage_name();
        if !self.collapsed.remove(&name) {
            self.collapsed.insert(name);
        }
    }

    pub fn folder_expanded(&self, path: &Path) -> bool {
        self.expanded_folders.contains(path)
    }

    pub fn toggle_folder(&mut self, path: &Path) {
        if !self.expanded_folders.remove(path) {
            self.expanded_folders.insert(path.to_path_buf());
        }
    }

    /// The visible folder rows under `root` (the root itself not included).
    pub fn folder_rows(&self, root: &FolderNode) -> Vec<FolderRow> {
        fn visit(model: &SidebarModel, node: &FolderNode, depth: usize, out: &mut Vec<FolderRow>) {
            for child in &node.children {
                if depth == 0 && SKIPPED_FOLDERS.contains(&child.name.as_str()) {
                    continue;
                }
                let expanded = model.folder_expanded(&child.path);
                out.push(FolderRow {
                    path: child.path.clone(),
                    name: child.name.clone(),
                    depth,
                    has_children: !child.children.is_empty(),
                    expanded,
                });
                if expanded {
                    visit(model, child, depth + 1, out);
                }
            }
        }
        let mut rows = Vec::new();
        visit(self, root, 0, &mut rows);
        rows
    }

    pub fn save(&self, project_root: &Path) -> SavedSidebar {
        SavedSidebar {
            pinned: self
                .pinned
                .iter()
                .map(|key| match key {
                    TabKey::File(path) => TabKey::File(relative_to(path, project_root)),
                    other => other.clone(),
                })
                .collect(),
            collapsed_groups: self.collapsed.iter().cloned().collect(),
            expanded_folders: self
                .expanded_folders
                .iter()
                .map(|path| relative_to(path, project_root))
                .collect(),
        }
    }

    pub fn restore(&mut self, saved: SavedSidebar, project_root: &Path) {
        self.pinned = saved
            .pinned
            .into_iter()
            .map(|key| match key {
                TabKey::File(path) => TabKey::File(project_root.join(path)),
                other => other,
            })
            .collect();
        self.collapsed = saved.collapsed_groups.into_iter().collect();
        self.expanded_folders = saved
            .expanded_folders
            .into_iter()
            .map(|path| project_root.join(path))
            .collect();
    }
}

fn closed_file(path: &Path) -> SidebarTab {
    SidebarTab {
        key: TabKey::File(path.to_path_buf()),
        index: None,
        title: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
        kind: String::new(),
        icon: None,
        active: false,
        unsaved: false,
    }
}

fn relative_to(path: &Path, root: &Path) -> PathBuf {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

/// A readable group name from a panel name: "Blueprint Editor" stays as it is,
/// "script_editor" becomes "Script Editor".
pub fn group_label(kind: &str) -> String {
    if kind.is_empty() {
        return "Other".into();
    }
    if kind.contains(' ') {
        return kind.to_string();
    }
    kind.split(['_', '-', '.'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The folder the content tree starts at: the project's `Content` folder when
/// it has one, otherwise the project folder itself.
pub fn content_root(tree: &FolderNode) -> &FolderNode {
    tree.children
        .iter()
        .find(|child| child.name.eq_ignore_ascii_case("content"))
        .unwrap_or(tree)
}

/// Whether the overlay drawer is open, from where the pointer is.
///
/// Hovering the rail opens it. Leaving both the rail and the drawer closes it,
/// but only after [`HOVER_CLOSE_DELAY`]: moving from the rail onto the drawer
/// crosses no gap, yet the two hover events can arrive a frame apart.
#[derive(Debug, Default)]
pub struct HoverState {
    rail: bool,
    drawer: bool,
    open: bool,
}

pub const HOVER_CLOSE_DELAY: std::time::Duration = std::time::Duration::from_millis(220);

#[derive(Debug, PartialEq, Eq)]
pub enum HoverAction {
    /// Nothing to do.
    None,
    /// The drawer just opened (or stays open): cancel a pending close.
    KeepOpen,
    /// Neither part is hovered: close after the delay unless hovered again.
    CloseLater,
}

impl HoverState {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_rail(&mut self, hovered: bool) -> HoverAction {
        self.rail = hovered;
        self.update()
    }

    pub fn set_drawer(&mut self, hovered: bool) -> HoverAction {
        self.drawer = hovered;
        self.update()
    }

    fn update(&mut self) -> HoverAction {
        if self.rail || self.drawer {
            self.open = true;
            HoverAction::KeepOpen
        } else if self.open {
            HoverAction::CloseLater
        } else {
            HoverAction::None
        }
    }

    /// The close delay ran out; returns whether the drawer closed.
    pub fn close_if_unhovered(&mut self) -> bool {
        if self.open && !self.rail && !self.drawer {
            self.open = false;
            true
        } else {
            false
        }
    }

    /// Close at once, e.g. after a tab was chosen.
    pub fn close(&mut self) {
        self.open = false;
        self.rail = false;
        self.drawer = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(index: usize, kind: &str, file: Option<&str>) -> SidebarTab {
        SidebarTab {
            key: TabKey::of(kind, file.map(Path::new)),
            index: Some(index),
            title: file.unwrap_or(kind).to_string(),
            kind: kind.to_string(),
            icon: None,
            active: index == 0,
            unsaved: false,
        }
    }

    fn open_tabs() -> Vec<SidebarTab> {
        vec![
            tab(0, "Level Editor", None),
            tab(1, "Blueprint Editor", Some("/p/Player.class")),
            tab(2, "script_editor", Some("/p/main.rs")),
            tab(3, "Blueprint Editor", Some("/p/Enemy.class")),
        ]
    }

    fn titles(section: &TabSection) -> Vec<&str> {
        section.tabs.iter().map(|t| t.title.as_str()).collect()
    }

    #[test]
    fn tabs_group_by_editor_in_first_seen_order() {
        let sections = SidebarModel::default().sections(&open_tabs());
        let labels: Vec<&str> = sections.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Level Editor", "Blueprint Editor", "Script Editor"]
        );
        assert_eq!(titles(&sections[1]), ["/p/Player.class", "/p/Enemy.class"]);
    }

    #[test]
    fn pinned_tabs_lead_in_pin_order_and_leave_their_group() {
        let mut model = SidebarModel::default();
        assert!(model.toggle_pin(&TabKey::File("/p/Enemy.class".into())));
        assert!(model.toggle_pin(&TabKey::Panel("Level Editor".into())));
        let sections = model.sections(&open_tabs());

        assert_eq!(sections[0].id, SectionId::Pinned);
        assert_eq!(titles(&sections[0]), ["/p/Enemy.class", "Level Editor"]);
        assert_eq!(titles(&sections[1]), ["/p/Player.class"]);
        assert!(sections.iter().all(|s| s.label != "Level Editor"));

        assert!(!model.toggle_pin(&TabKey::Panel("Level Editor".into())));
        assert_eq!(model.sections(&open_tabs())[1].label, "Level Editor");
    }

    #[test]
    fn a_pinned_file_stays_listed_after_its_tab_closes() {
        let mut model = SidebarModel::default();
        model.toggle_pin(&TabKey::File("/p/Enemy.class".into()));
        model.toggle_pin(&TabKey::Panel("Gone".into()));
        let open: Vec<SidebarTab> = open_tabs()
            .into_iter()
            .filter(|t| t.index != Some(3))
            .collect();

        let pinned = &model.sections(&open)[0];
        assert_eq!(pinned.tabs.len(), 1, "a closed panel is not listed");
        assert_eq!(pinned.tabs[0].title, "Enemy.class");
        assert!(!pinned.tabs[0].is_open());
    }

    #[test]
    fn sections_collapse_by_id() {
        let mut model = SidebarModel::default();
        model.toggle_section(&SectionId::Group("Blueprint Editor".into()));
        let sections = model.sections(&open_tabs());
        assert!(sections[1].collapsed);
        assert!(!sections[0].collapsed);
        model.toggle_section(&SectionId::Group("Blueprint Editor".into()));
        assert!(!model.sections(&open_tabs())[1].collapsed);
    }

    #[test]
    fn group_labels_read_as_words() {
        assert_eq!(group_label("Blueprint Editor"), "Blueprint Editor");
        assert_eq!(group_label("script_editor"), "Script Editor");
        assert_eq!(group_label("asset-viewer"), "Asset Viewer");
        assert_eq!(group_label(""), "Other");
    }

    fn folder(path: &str, children: Vec<FolderNode>) -> FolderNode {
        FolderNode {
            path: path.into(),
            name: Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            children,
            expanded: false,
        }
    }

    fn project() -> FolderNode {
        folder(
            "/p",
            vec![
                folder(
                    "/p/Content",
                    vec![
                        folder("/p/Content/Maps", vec![]),
                        folder(
                            "/p/Content/Characters",
                            vec![folder("/p/Content/Characters/Hero", vec![])],
                        ),
                    ],
                ),
                folder("/p/target", vec![folder("/p/target/debug", vec![])]),
                folder("/p/src", vec![]),
            ],
        )
    }

    #[test]
    fn the_tree_starts_at_the_content_folder_when_there_is_one() {
        let tree = project();
        assert_eq!(content_root(&tree).path, Path::new("/p/Content"));
        let flat = folder("/q", vec![folder("/q/src", vec![])]);
        assert_eq!(content_root(&flat).path, Path::new("/q"));
    }

    #[test]
    fn folder_rows_follow_expansion_and_skip_build_output() {
        let mut model = SidebarModel::default();
        let tree = project();
        let names = |model: &SidebarModel, root: &FolderNode| {
            model
                .folder_rows(root)
                .into_iter()
                .map(|r| (r.name, r.depth))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&model, &tree),
            [("Content".into(), 0), ("src".into(), 0)]
        );

        let content = content_root(&tree);
        assert_eq!(
            names(&model, content),
            [("Maps".into(), 0), ("Characters".into(), 0)]
        );
        model.toggle_folder(Path::new("/p/Content/Characters"));
        assert_eq!(
            names(&model, content),
            [
                ("Maps".into(), 0),
                ("Characters".into(), 0),
                ("Hero".into(), 1)
            ]
        );
        let rows = model.folder_rows(content);
        assert!(rows[1].has_children && rows[1].expanded);
        assert!(!rows[0].has_children);
    }

    #[test]
    fn saved_state_is_project_relative_and_round_trips() {
        let mut model = SidebarModel::default();
        model.toggle_pin(&TabKey::File("/p/Content/Hero.class".into()));
        model.toggle_pin(&TabKey::Panel("Level Editor".into()));
        model.toggle_section(&SectionId::Pinned);
        model.toggle_folder(Path::new("/p/Content/Maps"));

        let saved = model.save(Path::new("/p"));
        assert_eq!(saved.pinned[0], TabKey::File("Content/Hero.class".into()));
        assert_eq!(saved.expanded_folders, [PathBuf::from("Content/Maps")]);
        let json = serde_json::to_string(&saved).unwrap();
        let back: SavedSidebar = serde_json::from_str(&json).unwrap();

        let mut moved = SidebarModel::default();
        moved.restore(back, Path::new("/elsewhere"));
        assert!(moved.is_pinned(&TabKey::File("/elsewhere/Content/Hero.class".into())));
        assert!(moved.is_pinned(&TabKey::Panel("Level Editor".into())));
        assert!(moved.folder_expanded(Path::new("/elsewhere/Content/Maps")));
        assert!(moved.sections(&[])[0].collapsed);
    }

    #[test]
    fn the_drawer_opens_on_hover_and_closes_only_once_both_parts_are_left() {
        let mut hover = HoverState::default();
        assert_eq!(hover.set_drawer(false), HoverAction::None);
        assert_eq!(hover.set_rail(true), HoverAction::KeepOpen);
        assert!(hover.is_open());

        // Rail to drawer: the rail's leave can arrive before the drawer's enter.
        assert_eq!(hover.set_rail(false), HoverAction::CloseLater);
        assert_eq!(hover.set_drawer(true), HoverAction::KeepOpen);
        assert!(
            !hover.close_if_unhovered(),
            "the late close finds the drawer hovered"
        );
        assert!(hover.is_open());

        assert_eq!(hover.set_drawer(false), HoverAction::CloseLater);
        assert!(hover.close_if_unhovered());
        assert!(!hover.is_open());
    }
}
