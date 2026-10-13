//! The `layout.json` file format and its I/O.
//!
//! ```json
//! { "version": 2,
//!   "windows": { "0": { "dock": …, "window": …, "sidebar": …,
//!                       "popouts": [], "dormant": [], "chrome": {} } } }
//! ```
//!
//! The file is a map of window *slots*; each slot is a [`SavedWindowLayout`].
//! Only slot [`PRIMARY_SLOT`] is read and written today. Writing a slot is a
//! read-modify-write that keeps every other slot as a raw JSON value, so a
//! later version (or another window) can own them without this code needing to
//! understand them.
//!
//! `popouts` lists the pop-out windows opened from the window (bounds + their
//! whole dock tree); see [`SavedPopout`]. Their trees get the same project-relative
//! path rewriting as the main window. An unreadable entry is skipped on its own.
//!
//! `dormant` lists tabs kept for a plugin that is not loaded (see [`super::dormant`]);
//! `reinsert_dormant` offers them to the resolver again on the next load.
//!
//! Version 1 files (`{ version, dock, window, sidebar }`) are migrated on read
//! as slot 0. Any other version is ignored.
//!
//! The file is replaced atomically (write beside, then rename).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ui::dock::{DockAreaState, DockState, PanelState};

use super::super::chrome::SavedChrome;
use super::super::nav_sidebar::model::SavedSidebar;
use super::dormant::{self, AnchorKey, DormantLeaf, DormantRoot};
use super::window_geometry::SavedWindow;

/// Bump when the format changes incompatibly.
pub(super) const LAYOUT_VERSION: u32 = 2;
/// The format before window slots existed; still readable.
const LEGACY_VERSION: u32 = 1;
pub(super) const LAYOUT_FILE: &str = "layout.json";
/// The slot of the primary project window.
pub(super) const PRIMARY_SLOT: &str = "0";

/// Everything saved about one editor window.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct SavedWindowLayout {
    pub dock: DockAreaState,
    /// Window size, position and maximized / fullscreen state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<SavedWindow>,
    /// Pinned tabs, collapsed groups and expanded folders of the left sidebar.
    #[serde(default, skip_serializing_if = "SavedSidebar::is_empty")]
    pub sidebar: SavedSidebar,
    /// Pop-out windows opened from this window. An entry that does not parse is
    /// skipped on its own.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient_list"
    )]
    pub popouts: Vec<SavedPopout>,
    /// Tabs whose panel could not be recreated this session because nothing is
    /// registered to build it (see [`super::dormant`]). Written back verbatim
    /// and offered again on the next load.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "dormant::lenient"
    )]
    pub dormant: Vec<DormantLeaf>,
    /// Drawers and other window chrome (see [`super::super::chrome`]).
    #[serde(default, skip_serializing_if = "SavedChrome::is_empty")]
    pub chrome: SavedChrome,
}

/// One pop-out window: where it was and the whole tree it held (a pop-out can
/// have been split, or had tabs dropped into it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct SavedPopout {
    pub bounds: SavedWindow,
    /// The pop-out dock's tree, in the same form as the main window's centre.
    pub dock: PanelState,
}

/// Deserialize a list leniently: an entry that does not parse is skipped
/// instead of making the whole window slot unreadable.
pub(super) fn lenient_list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let values = Vec::<Value>::deserialize(deserializer)?;
    Ok(values
        .into_iter()
        .filter_map(|value| match serde_json::from_value(value) {
            Ok(item) => Some(item),
            Err(error) => {
                tracing::warn!("layout: dropping an unreadable saved entry: {error}");
                None
            }
        })
        .collect())
}

/// The file as stored: slots kept raw so unknown ones survive a rewrite.
#[derive(Default)]
struct RawLayout {
    windows: BTreeMap<String, Value>,
}

pub(super) fn layout_path(project_root: &Path) -> PathBuf {
    project_root.join(".pulsar").join(LAYOUT_FILE)
}

/// Parse `text` as a layout of any readable version, normalised to slots.
fn parse(text: &str, path: &Path) -> Option<RawLayout> {
    let mut value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(
                "ignoring unreadable saved layout {}: {error}",
                path.display()
            );
            return None;
        }
    };
    let version = value.get("version").and_then(Value::as_u64);
    match version {
        Some(v) if v == u64::from(LAYOUT_VERSION) => {
            let Value::Object(map) = value.get_mut("windows")?.take() else {
                return None;
            };
            Some(RawLayout {
                windows: map.into_iter().collect(),
            })
        }
        Some(v) if v == u64::from(LEGACY_VERSION) => {
            // v1 was a single window: the slot is the file minus `version`.
            value.as_object_mut()?.remove("version");
            Some(RawLayout {
                windows: BTreeMap::from([(PRIMARY_SLOT.to_string(), value)]),
            })
        }
        other => {
            tracing::info!(
                found = ?other,
                expected = LAYOUT_VERSION,
                "ignoring saved layout of another version"
            );
            None
        }
    }
}

fn read_raw(path: &Path) -> Option<RawLayout> {
    let text = std::fs::read_to_string(path).ok()?;
    parse(&text, path)
}

/// The saved layout of window `slot`, if the file and the slot are readable.
pub(super) fn read_slot(path: &Path, slot: &str) -> Option<SavedWindowLayout> {
    let raw = read_raw(path)?;
    match serde_json::from_value(raw.windows.get(slot)?.clone()) {
        Ok(layout) => Some(layout),
        Err(error) => {
            tracing::warn!("ignoring unreadable layout slot {slot}: {error}");
            None
        }
    }
}

/// Replace window `slot` in the file, leaving every other slot untouched.
pub(super) fn write_slot(
    path: &Path,
    slot: &str,
    layout: &SavedWindowLayout,
) -> std::io::Result<()> {
    let mut raw = read_raw(path).unwrap_or_default();
    raw.windows
        .insert(slot.to_string(), serde_json::to_value(layout)?);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = serde_json::json!({ "version": LAYOUT_VERSION, "windows": raw.windows });
    let json = serde_json::to_vec_pretty(&file)?;
    // Write beside the target and rename, so a crash mid-write cannot leave a
    // truncated layout behind.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

// ── Project-relative paths ───────────────────────────────────────────────────

/// Rewrite recorded file paths inside the project as project-relative.
fn relativize(state: &mut PanelState, root: &Path) {
    if let Some(value) = state.file_mut() {
        if let Some(path) = value.as_str().map(PathBuf::from) {
            if let Ok(relative) = path.strip_prefix(root) {
                *value = relative.to_string_lossy().replace('\\', "/").into();
            }
        }
    }
    for child in &mut state.children {
        relativize(child, root);
    }
}

/// Inverse of [`relativize`]: make recorded paths absolute under `root`.
fn absolutize(state: &mut PanelState, root: &Path) {
    if let Some(value) = state.file_mut() {
        if let Some(path) = value.as_str().map(PathBuf::from) {
            if path.is_relative() {
                *value = root.join(path).to_string_lossy().into_owned().into();
            }
        }
    }
    for child in &mut state.children {
        absolutize(child, root);
    }
}

fn map_dock_panels(dock: &mut Option<DockState>, f: impl FnOnce(&mut PanelState)) {
    if let Some(dock) = dock {
        // `DockState` exposes its tree read-only; round-trip through serde to
        // rewrite it, which also keeps this independent of its field layout.
        if let Ok(mut value) = serde_json::to_value(&*dock) {
            if let Some(panel) = value.get_mut("panel") {
                if let Ok(mut state) = serde_json::from_value::<PanelState>(panel.take()) {
                    f(&mut state);
                    if let Ok(v) = serde_json::to_value(&state) {
                        *panel = v;
                    }
                }
            }
            if let Ok(updated) = serde_json::from_value::<DockState>(value) {
                *dock = updated;
            }
        }
    }
}

/// Run `f` on the tree of every root of `dock`, saying which root it is.
fn edit_roots(dock: &mut DockAreaState, f: &mut dyn FnMut(DormantRoot, &mut PanelState)) {
    f(DormantRoot::Center, &mut dock.center);
    map_dock_panels(&mut dock.left_dock, |s| f(DormantRoot::Left, s));
    map_dock_panels(&mut dock.right_dock, |s| f(DormantRoot::Right, s));
    map_dock_panels(&mut dock.bottom_dock, |s| f(DormantRoot::Bottom, s));
}

impl SavedWindowLayout {
    /// Store file paths inside `root` as project-relative. Dormant tabs are
    /// rewritten with the rest; restore data stays opaque and is not touched.
    pub fn relativize(&mut self, root: &Path) {
        edit_roots(&mut self.dock, &mut |_, state| relativize(state, root));
        // The zoom target names its panel by file like the tree does.
        if let Some(zoomed) = &mut self.dock.zoomed {
            relativize(&mut zoomed.panel, root);
        }
        for popout in &mut self.popouts {
            relativize(&mut popout.dock, root);
        }
        for leaf in &mut self.dormant {
            relativize(&mut leaf.state, root);
            if let Some(AnchorKey::File(file)) = &mut leaf.anchor {
                *file = relative_path(file, root);
            }
        }
    }

    /// Make stored file paths absolute under `root`.
    pub fn absolutize(&mut self, root: &Path) {
        edit_roots(&mut self.dock, &mut |_, state| absolutize(state, root));
        if let Some(zoomed) = &mut self.dock.zoomed {
            absolutize(&mut zoomed.panel, root);
        }
        for popout in &mut self.popouts {
            absolutize(&mut popout.dock, root);
        }
        for leaf in &mut self.dormant {
            absolutize(&mut leaf.state, root);
            if let Some(AnchorKey::File(file)) = &mut leaf.anchor {
                *file = absolute_path(file, root);
            }
        }
    }

    /// Offer every dormant tab again: put each back into its root's tree so it
    /// is resolved with everything else, and clear the list (whatever still
    /// cannot be resolved becomes dormant again). Call after [`Self::absolutize`].
    pub fn reinsert_dormant(&mut self) {
        let leaves = std::mem::take(&mut self.dormant);
        edit_roots(&mut self.dock, &mut |root, tree| {
            for leaf in leaves.iter().filter(|l| l.root == root) {
                if !dormant::reinsert(tree, leaf) {
                    tracing::debug!("layout: no tab group to offer a dormant tab to");
                }
            }
        });
    }
}

fn relative_path(file: &str, root: &Path) -> String {
    match Path::new(file).strip_prefix(root) {
        Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
        Err(_) => file.to_string(),
    }
}

fn absolute_path(file: &str, root: &Path) -> String {
    let path = Path::new(file);
    if path.is_relative() {
        root.join(path).to_string_lossy().into_owned()
    } else {
        file.to_string()
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use ui::dock::PanelInfo;

    pub fn tab_group(files: &[&str]) -> PanelState {
        let mut group = PanelState {
            panel_name: "TabPanel".into(),
            info: PanelInfo::tabs(0),
            ..Default::default()
        };
        for file in files {
            group.add_child(
                PanelState {
                    panel_name: "Editor".into(),
                    ..Default::default()
                }
                .with_file(*file),
            );
        }
        group
    }

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("pulsar-layout-{name}-{}", std::process::id()))
    }

    #[test]
    fn project_paths_round_trip_through_relative_form() {
        let root = Path::new("/proj");
        let mut state = tab_group(&["/proj/scripts/a.rs", "/elsewhere/b.rs"]);

        relativize(&mut state, root);
        assert_eq!(state.children[0].file(), Some("scripts/a.rs"));
        // Outside the project stays absolute.
        assert_eq!(state.children[1].file(), Some("/elsewhere/b.rs"));

        absolutize(&mut state, root);
        assert_eq!(
            Path::new(state.children[0].file().unwrap()),
            root.join("scripts/a.rs")
        );
        assert_eq!(state.children[1].file(), Some("/elsewhere/b.rs"));
    }

    fn zoomed_layout(file: &str) -> SavedWindowLayout {
        SavedWindowLayout {
            dock: DockAreaState {
                center: tab_group(&["/proj/a.rs"]),
                zoomed: Some(ui::dock::ZoomedState {
                    panel: PanelState {
                        panel_name: "Editor".into(),
                        ..Default::default()
                    }
                    .with_file(file),
                    placement: ui::dock::DockPlacement::Center,
                    path: vec![0],
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn zoom_target_path_is_rewritten_with_the_tree() {
        let root = Path::new("/proj");
        let mut layout = zoomed_layout("/proj/scripts/a.rs");

        layout.relativize(root);
        let zoomed = layout.dock.zoomed.as_ref().unwrap();
        assert_eq!(zoomed.panel.file(), Some("scripts/a.rs"));
        assert_eq!(zoomed.path, vec![0], "tree position is untouched");

        layout.absolutize(root);
        let zoomed = layout.dock.zoomed.as_ref().unwrap();
        assert_eq!(
            Path::new(zoomed.panel.file().unwrap()),
            root.join("scripts/a.rs")
        );

        // A file outside the project stays as it is both ways.
        let mut outside = zoomed_layout("/elsewhere/b.rs");
        outside.relativize(root);
        assert_eq!(
            outside.dock.zoomed.as_ref().unwrap().panel.file(),
            Some("/elsewhere/b.rs")
        );
    }

    #[test]
    fn zoom_and_chrome_survive_a_write_and_read() {
        let dir = temp("zoom-chrome");
        let path = layout_path(&dir);
        let mut layout = zoomed_layout("scripts/a.rs");
        layout.chrome = SavedChrome {
            file_manager_docked: true,
            file_manager_height: 512.0,
        };

        write_slot(&path, PRIMARY_SLOT, &layout).unwrap();
        let read = read_slot(&path, PRIMARY_SLOT).expect("layout reads back");
        assert_eq!(read.dock.zoomed, layout.dock.zoomed);
        assert_eq!(read.chrome, layout.chrome);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_chrome_and_no_zoom_write_nothing() {
        let value = serde_json::to_value(SavedWindowLayout::default()).unwrap();
        assert!(value.get("chrome").is_none());
        assert!(value["dock"].get("zoomed").is_none());
    }

    #[test]
    fn layout_survives_a_write_and_read() {
        let dir = temp("rw");
        let path = layout_path(&dir);
        let layout = SavedWindowLayout {
            dock: DockAreaState {
                center: tab_group(&["a.rs", "b.rs"]),
                ..Default::default()
            },
            ..Default::default()
        };

        write_slot(&path, PRIMARY_SLOT, &layout).unwrap();
        let read = read_slot(&path, PRIMARY_SLOT).expect("layout reads back");
        assert_eq!(read.dock.center, layout.dock.center);
        assert!(
            !path.with_extension("json.tmp").exists(),
            "temp file renamed away"
        );
        let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["version"], 2);
        assert!(file["windows"]["0"].is_object());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tile_bounds_and_stacking_survive_a_write_and_read() {
        use gpui::{point, px, size, Bounds};
        use ui::dock::TileMeta;

        let metas = vec![
            TileMeta {
                bounds: Bounds {
                    origin: point(px(10.), px(20.)),
                    size: size(px(300.), px(200.)),
                },
                z_index: 1,
            },
            TileMeta {
                bounds: Bounds {
                    origin: point(px(50.), px(60.)),
                    size: size(px(400.), px(250.)),
                },
                z_index: 0,
            },
        ];
        let tiles = PanelState {
            panel_name: "Tiles".into(),
            children: vec![tab_group(&["a.rs"]), tab_group(&["b.rs"])],
            info: PanelInfo::tiles(metas.clone()),
        };
        let dir = temp("tiles");
        let path = layout_path(&dir);
        let layout = SavedWindowLayout {
            dock: DockAreaState {
                center: tiles,
                ..Default::default()
            },
            ..Default::default()
        };

        write_slot(&path, PRIMARY_SLOT, &layout).unwrap();
        let read = read_slot(&path, PRIMARY_SLOT).unwrap();
        assert_eq!(read.dock.center.info, PanelInfo::tiles(metas));
        assert_eq!(read.dock.center.children.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sidebar_state_survives_a_write_and_older_files_read_without_it() {
        use super::super::super::nav_sidebar::model::{SavedGroup, TabKey};
        let dir = temp("sidebar");
        let path = layout_path(&dir);
        let layout = SavedWindowLayout {
            sidebar: SavedSidebar {
                pinned: vec![
                    TabKey::File("Content/Hero.class".into()),
                    TabKey::Panel("Level Editor".into()),
                ],
                groups: vec![SavedGroup {
                    name: "Combat".into(),
                    members: vec![TabKey::File("Content/Sword.class".into())],
                    collapsed: false,
                }],
                collapsed_groups: vec!["Blueprint Editor".into()],
                expanded_folders: vec!["Content/Maps".into()],
                panes: {
                    use super::super::super::nav_sidebar::panes::{PaneKind, PaneStack};
                    let mut panes = PaneStack::default();
                    panes.toggle(PaneKind::Content);
                    panes
                },
            },
            ..Default::default()
        };
        write_slot(&path, PRIMARY_SLOT, &layout).unwrap();
        assert_eq!(
            read_slot(&path, PRIMARY_SLOT).unwrap().sidebar,
            layout.sidebar
        );

        // A file written before the sidebar existed has no `sidebar` key.
        let mut older = serde_json::to_value(&layout).unwrap();
        older.as_object_mut().unwrap().remove("sidebar");
        let file = serde_json::json!({ "version": 2, "windows": { "0": older } });
        std::fs::write(&path, file.to_string()).unwrap();
        assert!(read_slot(&path, PRIMARY_SLOT)
            .expect("older layout")
            .sidebar
            .is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_layout_of_an_unknown_version_is_ignored() {
        let dir = temp("version");
        let path = layout_path(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        for version in [0, LAYOUT_VERSION + 1] {
            let file = serde_json::json!({ "version": version, "windows": { "0": {
                "dock": serde_json::to_value(DockAreaState::default()).unwrap() } } });
            std::fs::write(&path, file.to_string()).unwrap();
            assert!(read_slot(&path, PRIMARY_SLOT).is_none());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_v1_file_migrates_to_slot_zero() {
        let dir = temp("v1");
        let path = layout_path(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let dock = DockAreaState {
            center: tab_group(&["a.rs"]),
            ..Default::default()
        };
        let v1 = serde_json::json!({ "version": 1, "dock": dock });
        std::fs::write(&path, v1.to_string()).unwrap();

        let read = read_slot(&path, PRIMARY_SLOT).expect("v1 migrates");
        assert_eq!(read.dock.center, dock.center);
        assert!(read.popouts.is_empty() && read.dormant.is_empty());

        // The next write upgrades the file to v2.
        write_slot(&path, PRIMARY_SLOT, &read).unwrap();
        let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["version"], 2);
        assert!(file.get("dock").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writing_a_slot_preserves_the_others_verbatim() {
        let dir = temp("slots");
        let path = layout_path(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let other = serde_json::json!({ "mystery": [1, 2, 3], "dock": null });
        let file = serde_json::json!({ "version": 2, "windows": { "1": other.clone() } });
        std::fs::write(&path, file.to_string()).unwrap();

        write_slot(&path, PRIMARY_SLOT, &SavedWindowLayout::default()).unwrap();

        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after["windows"]["1"], other);
        assert!(after["windows"]["0"].is_object());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dormant_tabs_round_trip_and_are_offered_again() {
        let root = Path::new("/proj");
        let mut layout = SavedWindowLayout {
            dock: DockAreaState {
                center: tab_group(&["/proj/a.rs"]),
                ..Default::default()
            },
            dormant: vec![DormantLeaf {
                root: DormantRoot::Center,
                anchor: Some(AnchorKey::File("/proj/a.rs".into())),
                state: PanelState::default().with_file("/proj/thing.xyz"),
            }],
            ..Default::default()
        };

        layout.relativize(root);
        assert_eq!(layout.dormant[0].state.file(), Some("thing.xyz"));
        assert_eq!(
            layout.dormant[0].anchor,
            Some(AnchorKey::File("a.rs".into()))
        );

        let dir = temp("dormant");
        let path = layout_path(&dir);
        write_slot(&path, PRIMARY_SLOT, &layout).unwrap();
        let mut read = read_slot(&path, PRIMARY_SLOT).unwrap();
        assert_eq!(read.dormant, layout.dormant);
        let _ = std::fs::remove_dir_all(&dir);

        read.absolutize(root);
        read.reinsert_dormant();
        assert!(read.dormant.is_empty());
        let group = &read.dock.center.children;
        assert_eq!(group.len(), 2);
        assert_eq!(Path::new(group[1].file().unwrap()), root.join("thing.xyz"));
    }

    #[test]
    fn a_slot_with_a_bad_dormant_entry_still_reads() {
        let mut slot = serde_json::to_value(SavedWindowLayout::default()).unwrap();
        slot["dormant"] = serde_json::json!([{ "root": "nope" }]);
        let layout: SavedWindowLayout = serde_json::from_value(slot).unwrap();
        assert!(layout.dormant.is_empty());
    }

    #[test]
    fn rewriting_dock_panels_round_trips_a_side_dock() {
        // The side docks go through `map_dock_panels`; check it reaches inside.
        let json = serde_json::json!({
            "panel": serde_json::to_value(tab_group(&["/proj/x.rs"])).unwrap(),
            "placement": "left",
            "size": 420.0,
            "open": true,
        });
        let mut dock = Some(serde_json::from_value::<DockState>(json).unwrap());
        map_dock_panels(&mut dock, |s| relativize(s, Path::new("/proj")));
        let dock = dock.unwrap();
        assert_eq!(dock.panel().children[0].file(), Some("x.rs"));
        assert!(dock.is_open());
    }

    fn popout(files: &[&str], x: f32) -> SavedPopout {
        use gpui::{point, px, size, Bounds, WindowBounds};
        SavedPopout {
            bounds: SavedWindow::capture(
                WindowBounds::Windowed(Bounds {
                    origin: point(px(x), px(40.)),
                    size: size(px(800.), px(600.)),
                }),
                None,
            ),
            dock: tab_group(files),
        }
    }

    #[test]
    fn popouts_round_trip_with_their_paths_rewritten() {
        let root = Path::new("/proj");
        let mut layout = SavedWindowLayout {
            popouts: vec![
                popout(&["/proj/a.rs", "/elsewhere/b.rs"], 10.),
                popout(&["/proj/c.rs"], 900.),
            ],
            ..Default::default()
        };
        let original = layout.clone();

        layout.relativize(root);
        assert_eq!(layout.popouts[0].dock.children[0].file(), Some("a.rs"));
        assert_eq!(
            layout.popouts[0].dock.children[1].file(),
            Some("/elsewhere/b.rs")
        );
        assert_eq!(layout.popouts[1].dock.children[0].file(), Some("c.rs"));

        let dir = temp("popouts");
        let path = layout_path(&dir);
        write_slot(&path, PRIMARY_SLOT, &layout).unwrap();
        let mut read = read_slot(&path, PRIMARY_SLOT).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read.popouts, layout.popouts);

        read.absolutize(root);
        assert_eq!(
            Path::new(read.popouts[1].dock.children[0].file().unwrap()),
            root.join("c.rs")
        );
        assert_eq!(read.popouts[0].bounds, original.popouts[0].bounds);
    }

    #[test]
    fn a_bad_popout_entry_is_skipped_on_its_own() {
        let mut slot = serde_json::to_value(SavedWindowLayout {
            popouts: vec![popout(&["a.rs"], 10.), popout(&["b.rs"], 20.)],
            ..Default::default()
        })
        .unwrap();
        let list = slot["popouts"].as_array_mut().unwrap();
        list.insert(1, serde_json::json!({ "bounds": "nonsense" }));
        list.push(serde_json::json!(42));

        let layout: SavedWindowLayout = serde_json::from_value(slot).unwrap();
        assert_eq!(layout.popouts.len(), 2);
        assert_eq!(layout.popouts[0].dock.children[0].file(), Some("a.rs"));
        assert_eq!(layout.popouts[1].dock.children[0].file(), Some("b.rs"));
    }
}
