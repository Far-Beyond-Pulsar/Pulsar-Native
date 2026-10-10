//! The content tree as a list the pane can window.
//!
//! The flat list of what the tree shows is built once per change (a folder
//! expanded or collapsed, the project's files changing on disk) and kept.
//! Folders are stored as rows; the files of a folder are stored as one run
//! that shares the drawer's cached listing, so a folder of a million files
//! costs one entry here, not a million. A row is only turned into something
//! drawable ([`ContentRow`]) when the pane asks for it, which is for the few
//! that are on screen.

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use ui_file_manager::utils::get_icon_for_file_type;
use ui_file_manager::utils::types::FileItem;
use ui_file_manager::FolderNode;

use super::model::{ContentFile, ContentRow, FolderRow, SidebarModel, SKIPPED_FOLDERS};

enum Segment {
    Folder(FolderRow),
    /// The files directly in one folder, after its subfolders.
    Files {
        depth: usize,
        items: Rc<Vec<FileItem>>,
        /// Indices into `items`, ascending, that are not listed: folder-based
        /// assets, which the tree lists once, as their folder.
        hidden: Vec<usize>,
    },
}

impl Segment {
    fn len(&self) -> usize {
        match self {
            Segment::Folder(_) => 1,
            Segment::Files { items, hidden, .. } => items.len() - hidden.len(),
        }
    }
}

/// The rows the content tree shows, in display order, below its root.
#[derive(Default)]
pub struct ContentList {
    segments: Vec<Segment>,
    /// The row index each segment starts at.
    starts: Vec<usize>,
    len: usize,
}

/// A file as the tree lists it.
pub fn content_file(item: &FileItem) -> ContentFile {
    ContentFile {
        icon: get_icon_for_file_type(item),
        color: item.file_type_def.as_ref().map(|def| def.color),
        path: item.path.clone(),
        name: item.name.clone(),
    }
}

impl ContentList {
    /// The rows under `root` (the root itself not included). `files` lists
    /// the files directly in a folder; it is asked only about folders that
    /// are shown, and about each shown folder's subfolders to tell whether
    /// they can be expanded.
    pub fn build(
        model: &SidebarModel,
        root: &FolderNode,
        files: &dyn Fn(&Path) -> Rc<Vec<FileItem>>,
    ) -> Self {
        fn visit(
            model: &SidebarModel,
            node: &FolderNode,
            depth: usize,
            files: &dyn Fn(&Path) -> Rc<Vec<FileItem>>,
            out: &mut Vec<Segment>,
        ) {
            let items = files(&node.path);
            // A folder-based asset shows up both as a folder and as a file;
            // the tree lists it once, as the folder, which opens as the asset.
            let mut assets: HashMap<&Path, usize> = HashMap::new();
            let mut hidden = Vec::new();
            if !node.children.is_empty() {
                let children: std::collections::HashSet<&Path> =
                    node.children.iter().map(|c| c.path.as_path()).collect();
                for (index, item) in items.iter().enumerate() {
                    if children.contains(item.path.as_path()) {
                        assets.insert(item.path.as_path(), index);
                        hidden.push(index);
                    }
                }
            }
            for child in &node.children {
                if depth == 0 && SKIPPED_FOLDERS.contains(&child.name.as_str()) {
                    continue;
                }
                let expanded = model.folder_expanded(&child.path);
                let has_files = !files(&child.path).is_empty();
                let asset = assets
                    .get(child.path.as_path())
                    .map(|&index| content_file(&items[index]));
                out.push(Segment::Folder(FolderRow {
                    path: child.path.clone(),
                    name: child.name.clone(),
                    depth,
                    has_children: !child.children.is_empty() || has_files,
                    expanded,
                    asset,
                }));
                if expanded {
                    visit(model, child, depth + 1, files, out);
                }
            }
            if items.len() > hidden.len() {
                out.push(Segment::Files {
                    depth,
                    items,
                    hidden,
                });
            }
        }

        let mut segments = Vec::new();
        visit(model, root, 0, files, &mut segments);
        let mut starts = Vec::with_capacity(segments.len());
        let mut len = 0;
        for segment in &segments {
            starts.push(len);
            len += segment.len();
        }
        Self {
            segments,
            starts,
            len,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }


    /// Row `index`, made drawable.
    pub fn row(&self, index: usize) -> Option<ContentRow> {
        if index >= self.len {
            return None;
        }
        let segment = self.starts.partition_point(|&start| start <= index) - 1;
        let offset = index - self.starts[segment];
        Some(match &self.segments[segment] {
            Segment::Folder(folder) => ContentRow::Folder(folder.clone()),
            Segment::Files {
                depth,
                items,
                hidden,
            } => {
                // The offset-th item that is not hidden.
                let mut item = offset;
                for &skipped in hidden {
                    if skipped <= item {
                        item += 1;
                    } else {
                        break;
                    }
                }
                ContentRow::File {
                    file: content_file(&items[item]),
                    depth: *depth,
                }
            }
        })
    }

    /// The rows in `range`, made drawable.
    #[cfg(test)]
    pub fn rows(&self, range: std::ops::Range<usize>) -> Vec<ContentRow> {
        range.filter_map(|index| self.row(index)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

    fn item(path: &str) -> FileItem {
        FileItem {
            path: path.into(),
            name: Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            file_type_def: None,
            is_folder: false,
            size: 0,
            modified: None,
            is_ghost: false,
            restore_commit: None,
        }
    }

    fn no_files(_: &Path) -> Rc<Vec<FileItem>> {
        Rc::new(Vec::new())
    }

    /// (name, depth, is a folder) for each row.
    fn rows_of(
        model: &SidebarModel,
        root: &FolderNode,
        files: &dyn Fn(&Path) -> Rc<Vec<FileItem>>,
    ) -> Vec<(String, usize, bool)> {
        let list = ContentList::build(model, root, files);
        list.rows(0..list.len())
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

        let content = super::super::model::content_root(&tree);
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
        let list = ContentList::build(&model, content, &no_files);
        let Some(ContentRow::Folder(characters)) = list.row(1) else {
            panic!("a folder row")
        };
        assert!(characters.has_children && characters.expanded);
        let Some(ContentRow::Folder(maps)) = list.row(0) else {
            panic!("a folder row")
        };
        assert!(!maps.has_children);
    }

    #[test]
    fn files_follow_each_shown_folders_subfolders() {
        let mut model = SidebarModel::default();
        let tree = project();
        let content = super::super::model::content_root(&tree);
        let files = |folder: &Path| {
            Rc::new(match folder.to_str().unwrap() {
                "/p/Content" => vec![item("/p/Content/Door.class")],
                "/p/Content/Maps" => vec![item("/p/Content/Maps/Arena.level")],
                "/p/Content/Characters/Hero" => {
                    vec![item("/p/Content/Characters/Hero/Hero.png")]
                }
                // A folder-based asset the folder tree also lists.
                "/p/Content/Characters" => vec![item("/p/Content/Characters/Hero")],
                _ => vec![],
            })
        };

        assert_eq!(
            rows_of(&model, content, &files),
            [
                ("Maps".into(), 0, true),
                ("Characters".into(), 0, true),
                ("Door.class".into(), 0, false),
            ]
        );
        let list = ContentList::build(&model, content, &files);
        let Some(ContentRow::Folder(maps)) = list.row(0) else {
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
        let list = ContentList::build(&model, content, &files);
        let asset_of = |name: &str| {
            list.rows(0..list.len())
                .into_iter()
                .find_map(|row| match row {
                    ContentRow::Folder(folder) if folder.name == name => Some(folder.asset),
                    _ => None,
                })
        };
        assert_eq!(
            asset_of("Hero").flatten().map(|asset| asset.path),
            Some(PathBuf::from("/p/Content/Characters/Hero")),
            "a folder-based asset's row opens as the asset"
        );
        assert!(
            asset_of("Maps").expect("Maps row").is_none(),
            "a plain folder"
        );
    }

    #[test]
    fn a_huge_folder_is_one_run_and_rows_are_made_on_demand() {
        let model = SidebarModel::default();
        let tree = folder("/p/Content", vec![folder("/p/Content/Sub", vec![])]);
        // 100k files, with the one that is also the subfolder's asset among them.
        let mut names: Vec<FileItem> = (0..100_000)
            .map(|n| item(&format!("/p/Content/f{n:06}.png")))
            .collect();
        names.insert(500, item("/p/Content/Sub"));
        let listing = Rc::new(names);
        let files = {
            let listing = listing.clone();
            move |folder: &Path| {
                if folder == Path::new("/p/Content") {
                    listing.clone()
                } else {
                    Rc::new(Vec::new())
                }
            }
        };
        let list = ContentList::build(&model, &tree, &files);
        // One folder row plus every file except the hidden asset.
        assert_eq!(list.len(), 1 + 100_000);
        let Some(ContentRow::File { file, .. }) = list.row(1) else {
            panic!("a file row")
        };
        assert_eq!(file.name, "f000000.png");
        // Rows after the hidden index are shifted past it.
        let Some(ContentRow::File { file, .. }) = list.row(1 + 500) else {
            panic!("a file row")
        };
        assert_eq!(file.name, "f000500.png");
        let Some(ContentRow::File { file, .. }) = list.row(list.len() - 1) else {
            panic!("a file row")
        };
        assert_eq!(file.name, "f099999.png");
        assert!(list.row(list.len()).is_none());
    }
}
