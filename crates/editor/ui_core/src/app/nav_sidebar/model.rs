//! What the sidebar shows, independent of how it is drawn.
//!
//! Open editors are listed in sections: tabs the user pinned first, then the
//! groups the user made, then the rest grouped by the kind of editor (every
//! blueprint together, and so on). A pinned or grouped file stays listed after
//! its tab closes, so it reopens in one click. Below them is the project's
//! content tree: its folders, and the files in each expanded folder.

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
    /// A group the user made, by its id.
    Custom(u32),
    /// The automatic group of one kind of editor.
    Group(String),
}

/// A group of tabs the user made and named.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomGroup {
    pub id: u32,
    pub name: String,
    /// In the order they were added.
    pub members: Vec<TabKey>,
    pub collapsed: bool,
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

/// A file as the content tree lists it.
#[derive(Clone, Debug)]
pub struct ContentFile {
    pub path: PathBuf,
    pub name: String,
    pub icon: IconName,
    /// The file type's colour, when it has one.
    pub color: Option<gpui::Hsla>,
}

/// A row of the content tree, in display order: each folder's subfolders
/// first, then its files.
#[derive(Clone, Debug)]
pub enum ContentRow {
    Folder(FolderRow),
    File { file: ContentFile, depth: usize },
}

/// What a project saves about its sidebar, in `.pulsar/layout.json`. Paths
/// inside the project are stored relative to it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedSidebar {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<TabKey>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<SavedGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collapsed_groups: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expanded_folders: Vec<PathBuf>,
}

/// A [`CustomGroup`] as saved; ids are given out again on restore.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedGroup {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<TabKey>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collapsed: bool,
}

impl SavedSidebar {
    pub fn is_empty(&self) -> bool {
        self.pinned.is_empty()
            && self.groups.is_empty()
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
    /// In the order they were made.
    groups: Vec<CustomGroup>,
    next_group_id: u32,
    /// Collapsed sections other than the user's groups: "__pinned__" or a panel name.
    collapsed: BTreeSet<String>,
    expanded_folders: BTreeSet<PathBuf>,
}

impl SidebarModel {
    /// The editor list for `open` (all open tabs, in tab order).
    pub fn sections(&self, open: &[SidebarTab]) -> Vec<TabSection> {
        let mut sections = Vec::new();

        let pinned = listed(&self.pinned, open);
        if !pinned.is_empty() {
            sections.push(self.section(SectionId::Pinned, "Pinned".into(), pinned));
        }

        for group in &self.groups {
            let members: Vec<TabKey> = group
                .members
                .iter()
                .filter(|key| !self.is_pinned(key))
                .cloned()
                .collect();
            // Listed even when empty: it is where tabs are dropped.
            sections.push(TabSection {
                id: SectionId::Custom(group.id),
                label: group.name.clone(),
                tabs: listed(&members, open),
                collapsed: group.collapsed,
            });
        }

        let mut groups: Vec<(String, Vec<SidebarTab>)> = Vec::new();
        for tab in open
            .iter()
            .filter(|tab| !self.is_pinned(&tab.key) && self.group_of(&tab.key).is_none())
        {
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
        let collapsed = match &id {
            SectionId::Pinned => self.collapsed.contains(PINNED_SECTION),
            SectionId::Group(kind) => self.collapsed.contains(kind),
            SectionId::Custom(id) => self.group(*id).is_some_and(|g| g.collapsed),
        };
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
        let name = match id {
            SectionId::Pinned => PINNED_SECTION.to_string(),
            SectionId::Group(kind) => kind.clone(),
            SectionId::Custom(id) => {
                if let Some(group) = self.group_mut(*id) {
                    group.collapsed = !group.collapsed;
                }
                return;
            }
        };
        if !self.collapsed.remove(&name) {
            self.collapsed.insert(name);
        }
    }

    pub fn groups(&self) -> &[CustomGroup] {
        &self.groups
    }

    pub fn group(&self, id: u32) -> Option<&CustomGroup> {
        self.groups.iter().find(|g| g.id == id)
    }

    fn group_mut(&mut self, id: u32) -> Option<&mut CustomGroup> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    /// The user's group `key` is in.
    pub fn group_of(&self, key: &TabKey) -> Option<u32> {
        self.groups
            .iter()
            .find(|g| g.members.contains(key))
            .map(|g| g.id)
    }

    /// A name for a new group that no group has yet: "Group 1", "Group 2"…
    pub fn next_group_name(&self) -> String {
        (1..)
            .map(|n| format!("Group {n}"))
            .find(|name| self.groups.iter().all(|g| &g.name != name))
            .expect("an unused name")
    }

    /// Make an empty group; returns its id.
    pub fn create_group(&mut self, name: impl Into<String>) -> u32 {
        self.next_group_id += 1;
        let id = self.next_group_id;
        self.groups.push(CustomGroup {
            id,
            name: name.into(),
            members: Vec::new(),
            collapsed: false,
        });
        id
    }

    /// Rename a group; a blank name keeps the old one.
    pub fn rename_group(&mut self, id: u32, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        if let Some(group) = self.group_mut(id) {
            group.name = name.to_string();
        }
    }

    /// Remove a group; its tabs go back to their editor-kind groups.
    pub fn delete_group(&mut self, id: u32) {
        self.groups.retain(|g| g.id != id);
    }

    /// Put `key` in group `to`, or with `None` back in its editor-kind group.
    /// A tab is in at most one of the user's groups.
    pub fn move_to_group(&mut self, key: &TabKey, to: Option<u32>) {
        if to.is_some_and(|id| self.group(id).is_none()) {
            return;
        }
        for group in &mut self.groups {
            group.members.retain(|k| k != key);
        }
        if let Some(group) = to.and_then(|id| self.group_mut(id)) {
            group.members.push(key.clone());
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

    /// The visible rows under `root` (the root itself not included). `files`
    /// lists the files directly in a folder; it is asked only about folders
    /// that are shown.
    pub fn content_rows(
        &self,
        root: &FolderNode,
        files: &dyn Fn(&Path) -> Vec<ContentFile>,
    ) -> Vec<ContentRow> {
        fn visit(
            model: &SidebarModel,
            node: &FolderNode,
            depth: usize,
            files: &dyn Fn(&Path) -> Vec<ContentFile>,
            out: &mut Vec<ContentRow>,
        ) {
            for child in &node.children {
                if depth == 0 && SKIPPED_FOLDERS.contains(&child.name.as_str()) {
                    continue;
                }
                let expanded = model.folder_expanded(&child.path);
                let child_files = files_of(child, files);
                out.push(ContentRow::Folder(FolderRow {
                    path: child.path.clone(),
                    name: child.name.clone(),
                    depth,
                    has_children: !child.children.is_empty() || !child_files.is_empty(),
                    expanded,
                }));
                if expanded {
                    visit(model, child, depth + 1, files, out);
                }
            }
            for file in files_of(node, files) {
                out.push(ContentRow::File { file, depth });
            }
        }
        /// A folder-based asset can show up both as a folder and as a file;
        /// the tree lists it once, as the folder.
        fn files_of(
            node: &FolderNode,
            files: &dyn Fn(&Path) -> Vec<ContentFile>,
        ) -> Vec<ContentFile> {
            let mut listed = files(&node.path);
            listed.retain(|file| node.children.iter().all(|child| child.path != file.path));
            listed
        }
        let mut rows = Vec::new();
        visit(self, root, 0, files, &mut rows);
        rows
    }

    /// Keep the folders under `old` expanded after it was renamed or moved to
    /// `new`.
    pub fn folder_moved(&mut self, old: &Path, new: &Path) {
        let moved: Vec<PathBuf> = self
            .expanded_folders
            .iter()
            .filter(|path| path.starts_with(old))
            .cloned()
            .collect();
        for path in moved {
            self.expanded_folders.remove(&path);
            let rest = path.strip_prefix(old).expect("filtered on the prefix");
            self.expanded_folders.insert(new.join(rest));
        }
    }

    pub fn save(&self, project_root: &Path) -> SavedSidebar {
        let save_key = |key: &TabKey| match key {
            TabKey::File(path) => TabKey::File(relative_to(path, project_root)),
            other => other.clone(),
        };
        SavedSidebar {
            pinned: self.pinned.iter().map(save_key).collect(),
            groups: self
                .groups
                .iter()
                .map(|g| SavedGroup {
                    name: g.name.clone(),
                    members: g.members.iter().map(save_key).collect(),
                    collapsed: g.collapsed,
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
        let restore_key = |key: TabKey| match key {
            TabKey::File(path) => TabKey::File(project_root.join(path)),
            other => other,
        };
        self.pinned = saved.pinned.into_iter().map(restore_key).collect();
        self.groups.clear();
        for saved_group in saved.groups {
            let id = self.create_group(saved_group.name);
            let group = self.group_mut(id).expect("just made");
            group.members = saved_group.members.into_iter().map(restore_key).collect();
            group.collapsed = saved_group.collapsed;
        }
        self.collapsed = saved.collapsed_groups.into_iter().collect();
        self.expanded_folders = saved
            .expanded_folders
            .into_iter()
            .map(|path| project_root.join(path))
            .collect();
    }
}

/// The rows for `keys`: open tabs as they are, closed files as reopenable
/// rows. A closed panel can't be reopened from here, so it is left out.
fn listed(keys: &[TabKey], open: &[SidebarTab]) -> Vec<SidebarTab> {
    keys.iter()
        .filter_map(|key| match open.iter().find(|tab| &tab.key == key) {
            Some(tab) => Some(tab.clone()),
            None => match key {
                TabKey::File(path) => Some(closed_file(path)),
                TabKey::Panel(_) => None,
            },
        })
        .collect()
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
    /// Something opened from the drawer (a context menu) keeps it open.
    held: bool,
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

    /// Keep the drawer open while a menu opened from it is up, though the
    /// pointer is over the menu rather than the drawer.
    pub fn set_held(&mut self, held: bool) -> HoverAction {
        self.held = held && self.open;
        self.update()
    }

    fn update(&mut self) -> HoverAction {
        if self.rail || self.drawer || self.held {
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
        if self.open && !self.rail && !self.drawer && !self.held {
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
        self.held = false;
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

    fn no_files(_: &Path) -> Vec<ContentFile> {
        Vec::new()
    }

    fn file(path: &str) -> ContentFile {
        ContentFile {
            path: path.into(),
            name: Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            icon: IconName::Page,
            color: None,
        }
    }

    /// (name, depth, is a folder) for each row.
    fn rows_of(
        model: &SidebarModel,
        root: &FolderNode,
        files: &dyn Fn(&Path) -> Vec<ContentFile>,
    ) -> Vec<(String, usize, bool)> {
        model
            .content_rows(root, files)
            .into_iter()
            .map(|row| match row {
                ContentRow::Folder(folder) => (folder.name, folder.depth, true),
                ContentRow::File { file, depth } => (file.name, depth, false),
            })
            .collect()
    }

    #[test]
    fn folder_rows_follow_expansion_and_skip_build_output() {
        let mut model = SidebarModel::default();
        let tree = project();
        let names = |model: &SidebarModel, root: &FolderNode| {
            rows_of(model, root, &no_files)
                .into_iter()
                .map(|(name, depth, _)| (name, depth))
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
        let rows = model.content_rows(content, &no_files);
        let ContentRow::Folder(characters) = &rows[1] else {
            panic!("a folder row")
        };
        assert!(characters.has_children && characters.expanded);
        let ContentRow::Folder(maps) = &rows[0] else {
            panic!("a folder row")
        };
        assert!(!maps.has_children);
    }

    #[test]
    fn files_follow_each_shown_folders_subfolders() {
        let mut model = SidebarModel::default();
        let tree = project();
        let content = content_root(&tree);
        let files = |folder: &Path| match folder.to_str().unwrap() {
            "/p/Content" => vec![file("/p/Content/Door.class")],
            "/p/Content/Maps" => vec![file("/p/Content/Maps/Arena.level")],
            "/p/Content/Characters/Hero" => vec![file("/p/Content/Characters/Hero/Hero.png")],
            // A folder-based asset the folder tree also lists.
            "/p/Content/Characters" => vec![file("/p/Content/Characters/Hero")],
            _ => vec![],
        };

        assert_eq!(
            rows_of(&model, content, &files),
            [
                ("Maps".into(), 0, true),
                ("Characters".into(), 0, true),
                ("Door.class".into(), 0, false),
            ]
        );
        let rows = model.content_rows(content, &files);
        let ContentRow::Folder(maps) = &rows[0] else {
            panic!("a folder row")
        };
        assert!(maps.has_children, "files alone make a folder expandable");

        model.toggle_folder(Path::new("/p/Content/Maps"));
        model.toggle_folder(Path::new("/p/Content/Characters"));
        model.toggle_folder(Path::new("/p/Content/Characters/Hero"));
        assert_eq!(
            rows_of(&model, content, &files),
            [
                ("Maps".into(), 0, true),
                ("Arena.level".into(), 1, false),
                ("Characters".into(), 0, true),
                ("Hero".into(), 1, true),
                ("Hero.png".into(), 2, false),
                ("Door.class".into(), 0, false),
            ],
            "the folder-based Hero is listed once, as its folder"
        );
    }

    #[test]
    fn a_moved_folder_keeps_its_expanded_subfolders() {
        let mut model = SidebarModel::default();
        model.toggle_folder(Path::new("/p/Content/Characters"));
        model.toggle_folder(Path::new("/p/Content/Characters/Hero"));
        model.toggle_folder(Path::new("/p/Content/Maps"));
        model.folder_moved(Path::new("/p/Content/Characters"), Path::new("/p/Content/Cast"));
        assert!(model.folder_expanded(Path::new("/p/Content/Cast")));
        assert!(model.folder_expanded(Path::new("/p/Content/Cast/Hero")));
        assert!(!model.folder_expanded(Path::new("/p/Content/Characters")));
        assert!(model.folder_expanded(Path::new("/p/Content/Maps")));
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

    fn labels(sections: &[TabSection]) -> Vec<&str> {
        sections.iter().map(|s| s.label.as_str()).collect()
    }

    #[test]
    fn user_groups_follow_pinned_and_take_their_tabs_out_of_the_kind_groups() {
        let mut model = SidebarModel::default();
        let work = model.create_group("Work");
        let hero = TabKey::File("/p/Player.class".into());
        let main = TabKey::File("/p/main.rs".into());
        model.move_to_group(&main, Some(work));
        model.move_to_group(&hero, Some(work));
        model.toggle_pin(&TabKey::Panel("Level Editor".into()));

        let sections = model.sections(&open_tabs());
        assert_eq!(labels(&sections), ["Pinned", "Work", "Blueprint Editor"]);
        assert_eq!(sections[1].id, SectionId::Custom(work));
        assert_eq!(
            titles(&sections[1]),
            ["/p/main.rs", "/p/Player.class"],
            "in the order added"
        );
        assert_eq!(titles(&sections[2]), ["/p/Enemy.class"]);
    }

    #[test]
    fn a_tab_is_in_one_user_group_at_most_and_can_go_back() {
        let mut model = SidebarModel::default();
        let a = model.create_group("A");
        let b = model.create_group("B");
        let key = TabKey::File("/p/Enemy.class".into());
        model.move_to_group(&key, Some(a));
        model.move_to_group(&key, Some(b));
        assert_eq!(model.group_of(&key), Some(b));
        assert!(model.group(a).unwrap().members.is_empty());

        model.move_to_group(&key, Some(99));
        assert_eq!(
            model.group_of(&key),
            Some(b),
            "an unknown group changes nothing"
        );

        model.move_to_group(&key, None);
        assert_eq!(model.group_of(&key), None);
        let sections = model.sections(&open_tabs());
        let blueprints = sections
            .iter()
            .find(|s| s.label == "Blueprint Editor")
            .unwrap();
        assert_eq!(blueprints.tabs.len(), 2, "back in its editor-kind group");
    }

    #[test]
    fn empty_groups_stay_listed_and_deleting_one_returns_its_tabs() {
        let mut model = SidebarModel::default();
        let empty = model.create_group(model.next_group_name());
        assert_eq!(model.group(empty).unwrap().name, "Group 1");
        assert_eq!(model.next_group_name(), "Group 2");
        let sections = model.sections(&open_tabs());
        assert_eq!(sections[0].label, "Group 1");
        assert!(sections[0].tabs.is_empty(), "listed as a drop target");

        let key = TabKey::File("/p/main.rs".into());
        model.move_to_group(&key, Some(empty));
        model.delete_group(empty);
        assert!(model.groups().is_empty());
        assert_eq!(model.group_of(&key), None);
        assert!(labels(&model.sections(&open_tabs())).contains(&"Script Editor"));
    }

    #[test]
    fn groups_rename_collapse_and_keep_closed_files() {
        let mut model = SidebarModel::default();
        let id = model.create_group("Old");
        model.rename_group(id, "  New  ");
        model.rename_group(id, "   ");
        assert_eq!(
            model.group(id).unwrap().name,
            "New",
            "trimmed; blank is ignored"
        );

        model.move_to_group(&TabKey::File("/p/Closed.class".into()), Some(id));
        model.move_to_group(&TabKey::Panel("Gone".into()), Some(id));
        model.toggle_section(&SectionId::Custom(id));
        let section = &model.sections(&open_tabs())[0];
        assert!(section.collapsed);
        assert_eq!(
            titles(section),
            ["Closed.class"],
            "a closed panel is not listed"
        );
        assert!(!section.tabs[0].is_open());
    }

    #[test]
    fn groups_are_saved_relative_to_the_project() {
        let mut model = SidebarModel::default();
        let id = model.create_group("Hero");
        model.move_to_group(&TabKey::File("/p/Content/Hero.class".into()), Some(id));
        model.toggle_section(&SectionId::Custom(id));

        let saved = model.save(Path::new("/p"));
        assert_eq!(
            saved.groups,
            [SavedGroup {
                name: "Hero".into(),
                members: vec![TabKey::File("Content/Hero.class".into())],
                collapsed: true,
            }]
        );
        let json = serde_json::to_string(&saved).unwrap();
        let mut moved = SidebarModel::default();
        moved.restore(serde_json::from_str(&json).unwrap(), Path::new("/q"));
        let group = &moved.groups()[0];
        assert_eq!(group.name, "Hero");
        assert!(group.collapsed);
        assert_eq!(
            group.members,
            [TabKey::File("/q/Content/Hero.class".into())]
        );
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

    #[test]
    fn a_menu_from_the_drawer_keeps_it_open_until_it_closes() {
        let mut hover = HoverState::default();
        assert_eq!(hover.set_held(true), HoverAction::None, "nothing to hold");
        assert!(!hover.is_open());

        hover.set_drawer(true);
        hover.set_held(true);
        // The pointer moves onto the menu, off the drawer.
        assert_eq!(hover.set_drawer(false), HoverAction::KeepOpen);
        assert!(!hover.close_if_unhovered());
        assert_eq!(hover.set_held(false), HoverAction::CloseLater);
        assert!(hover.close_if_unhovered());
    }
}
