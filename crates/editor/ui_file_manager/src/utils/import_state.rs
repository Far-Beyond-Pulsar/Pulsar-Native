//! Per-file import actions and the out-of-date badge (#1102).
//!
//! The drawer keeps the [`asset_import::FileImportState`] of each file in the
//! open folder, computed off the UI thread (it hashes linked sources) and
//! recomputed when the folder, its files or the import database change. The
//! context menu offers Reimport / Unlink on linked sources and their native
//! assets, and Import… on importable files with no link, including ones the
//! user earlier chose not to import.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use asset_import::FileImportState;
use gpui::*;
use ui::notification::Notification;
use ui::ContextModal as _;

use crate::components::FileManagerDrawer;
use crate::utils::actions::{ImportSource, ReimportSource, UnlinkSource};
use crate::utils::types::FileItem;

/// The import states of the open folder's files.
#[derive(Default)]
pub struct FolderImportStates {
    /// What the states were computed for: folder, the drawer's content
    /// revision (bumped by file changes) and the import database revision.
    key: Option<(PathBuf, u64, u64)>,
    states: HashMap<PathBuf, FileImportState>,
    _refresh: Option<Task<()>>,
}

impl FileManagerDrawer {
    /// The import state of `path`, if it is in the open folder and has one.
    pub(crate) fn import_state(&self, path: &Path) -> Option<&FileImportState> {
        self.import_states.states.get(path)
    }

    /// Recompute the open folder's import states in the background when the
    /// folder, its files or the import database changed. The previous states
    /// stay visible until the new ones arrive.
    pub(crate) fn refresh_import_states(
        &mut self,
        folder: &Path,
        items: &[FileItem],
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.project_path.clone() else {
            return;
        };
        if engine_fs::is_cloud_path(&root) {
            return;
        }
        let key = (
            folder.to_path_buf(),
            self.content_revision,
            asset_import::db_revision(),
        );
        if self.import_states.key.as_ref() == Some(&key) {
            return;
        }
        self.import_states.key = Some(key.clone());
        let files: Vec<PathBuf> = items
            .iter()
            .filter(|item| !item.is_folder && !item.is_ghost)
            .map(|item| item.path.clone())
            .collect();
        let computed = cx
            .background_executor()
            .spawn(async move { asset_import::import_states(&root, &files) });
        self.import_states._refresh = Some(cx.spawn(async move |this, cx| {
            let states = computed.await;
            let _ = this.update(cx, |drawer, cx| {
                if drawer.import_states.key.as_ref() == Some(&key) {
                    drawer.import_states.states = states;
                    cx.notify();
                }
            });
        }));
    }

    /// The files an import action applies to: its own path, or else the
    /// selection.
    fn import_action_targets(&self, item_path: &str) -> Vec<PathBuf> {
        if item_path.is_empty() {
            self.selected_items.iter().cloned().collect()
        } else {
            vec![PathBuf::from(item_path)]
        }
    }

    /// The linked sources (record keys) of `targets`, deduplicated: a source
    /// and its asset name the same link.
    fn linked_sources_of(&self, root: &Path, targets: &[PathBuf]) -> Vec<String> {
        let mut sources: Vec<String> = targets
            .iter()
            .filter_map(|path| self.import_state(path)?.linked_source(root, path))
            .collect();
        sources.sort();
        sources.dedup();
        sources
    }
}

pub(crate) fn handle_reimport_source(
    d: &mut FileManagerDrawer,
    action: &ReimportSource,
    window: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
) {
    let Some(root) = d.project_path.clone() else {
        return;
    };
    let targets = d.import_action_targets(&action.item_path);
    let sources = d.linked_sources_of(&root, &targets);
    let started = sources
        .iter()
        .filter(|source| asset_import::submit_reimport(root.clone(), source).is_some())
        .count();
    if started > 0 {
        window.push_notification(
            Notification::info(format!("Reimporting {started} file(s) — see Tasks")),
            cx,
        );
    }
}

pub(crate) fn handle_import_source(
    d: &mut FileManagerDrawer,
    action: &ImportSource,
    window: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
) {
    let Some(root) = d.project_path.clone() else {
        return;
    };
    let targets: Vec<PathBuf> = d
        .import_action_targets(&action.item_path)
        .into_iter()
        .filter(|path| {
            matches!(
                d.import_state(path),
                Some(FileImportState::Unlinked | FileImportState::Ignored)
            )
        })
        .collect();
    if targets.is_empty() {
        return;
    }
    // Importing a file the user earlier declined undoes that choice.
    if let Err(error) = asset_import::unignore_sources(&root, &targets) {
        window.push_notification(Notification::error(format!("Import failed: {error}")), cx);
        return;
    }
    let mut by_ext: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for path in targets {
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        by_ext.entry(ext).or_default().push(path);
    }
    crate::configurator::offer_import(root, by_ext, cx);
    cx.notify();
}

pub(crate) fn handle_unlink_source(
    d: &mut FileManagerDrawer,
    action: &UnlinkSource,
    window: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
) {
    let Some(root) = d.project_path.clone() else {
        return;
    };
    let targets = d.import_action_targets(&action.item_path);
    for source in d.linked_sources_of(&root, &targets) {
        if let Err(error) = asset_import::unlink_source(&root, &source) {
            window.push_notification(
                Notification::error(format!("Could not unlink {source}: {error}")),
                cx,
            );
        }
    }
    cx.notify();
}

#[cfg(test)]
mod tests {
    use asset_import::{FileImportState, ImportDb, ImportRecord, LinkStatus};
    use gpui::{
        point, px, size, AppContext as _, Bounds, TestAppContext, VisualTestContext, WindowBounds,
        WindowOptions,
    };

    use super::handle_unlink_source;
    use crate::components::FileManagerDrawer;
    use crate::utils::actions::UnlinkSource;

    /// A project holding a linked source and its native asset, the source
    /// edited since the import, plus an importable file with no link.
    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("rock.fbx"), b"imported bytes").unwrap();
        std::fs::write(root.join("rock.mesh"), b"native").unwrap();
        std::fs::write(root.join("tree.obj"), b"not imported").unwrap();
        let (source_hash, source_size) =
            asset_import::db::hash_file(&root.join("rock.fbx")).unwrap();
        let mut db = ImportDb::default();
        db.upsert(ImportRecord {
            source: "rock.fbx".into(),
            source_hash,
            source_size,
            asset: "rock.mesh".into(),
            importer: "mesh".into(),
            options: None,
            outputs: Vec::new(),
        });
        db.save(root).unwrap();
        std::fs::write(root.join("rock.fbx"), b"edited bytes!").unwrap();
        dir
    }

    /// The drawer badges an out-of-date source and its asset, and the
    /// Unlink action drops the link while keeping both files (#1102).
    #[gpui::test]
    fn the_drawer_badges_stale_links_and_unlinks_from_the_menu(cx: &mut TestAppContext) {
        walk(cx, None);
    }

    /// The same steps with real fonts, saving a PNG of the badges and of the
    /// linked source's menu, when `PULSAR_IMPORT_SHOTS` names a folder and a
    /// GPU adapter is available.
    #[test]
    fn import_action_screenshots() {
        let Some(dir) = std::env::var_os("PULSAR_IMPORT_SHOTS").map(std::path::PathBuf::from)
        else {
            return;
        };
        let mut cx = TestAppContext::with_real_text_system();
        walk(&mut cx, Some(dir));
    }

    fn walk(cx: &mut TestAppContext, shots: Option<std::path::PathBuf>) {
        let dir = project();
        let root = dir.path().to_path_buf();
        cx.update(|cx| {
            // As the engine's startup does; the drawer's import scan opens
            // the import configurator (for tree.obj) as its own window.
            cx.set_global(window_manager::WindowManager::new());
            cx.set_global(window_manager::WindowRegistry::new());
            ui::init(cx);
            ui::themes::init(cx);
        });
        let mut drawer = None;
        let window = cx.update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1000.), px(600.)),
                ))),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| FileManagerDrawer::new(Some(root.clone()), window, cx));
                drawer = Some(view.clone());
                cx.new(|cx| ui::Root::new(view.into(), window, cx))
            })
            .expect("window")
        });
        let drawer = drawer.unwrap();
        let handle: gpui::AnyWindowHandle = window.into();
        let headless = shots
            .as_ref()
            .and_then(|_| gpui::headless::HeadlessWindow::attach(handle, cx));
        let save = |cx: &mut TestAppContext, name: &str| {
            let (Some(headless), Some(dir)) = (&headless, &shots) else {
                return;
            };
            for _ in 0..3 {
                headless.draw(cx, true);
            }
            let _ = headless.settle(cx);
            let (width, height) = headless.size();
            std::fs::create_dir_all(dir).unwrap();
            image::RgbaImage::from_raw(width, height, headless.presented())
                .expect("frame size")
                .save(dir.join(format!("{name}.png")))
                .unwrap();
        };
        let mut cx = VisualTestContext::from_window(handle, cx);
        let draw = |cx: &mut VisualTestContext| {
            for _ in 0..3 {
                drawer.update(cx, |_, cx| cx.notify());
                cx.update(|window, cx| window.draw(cx).clear());
                cx.run_until_parked();
            }
        };
        drawer.update(&mut cx, |drawer, cx| {
            drawer.selected_folder = Some(root.clone());
            cx.notify();
        });
        draw(&mut cx);

        let state = |cx: &mut VisualTestContext, name: &str| {
            let path = root.join(name);
            drawer.read_with(cx, |drawer, _| drawer.import_state(&path).cloned())
        };
        assert_eq!(
            state(&mut cx, "rock.fbx"),
            Some(FileImportState::LinkedSource {
                status: LinkStatus::OutOfDate
            })
        );
        assert_eq!(
            state(&mut cx, "rock.mesh"),
            Some(FileImportState::LinkedAsset {
                source: "rock.fbx".into(),
                status: LinkStatus::OutOfDate
            })
        );
        assert_eq!(state(&mut cx, "tree.obj"), Some(FileImportState::Unlinked));
        assert!(
            cx.debug_bounds("out-of-date-rock.fbx").is_some(),
            "the source is badged"
        );
        assert!(
            cx.debug_bounds("out-of-date-rock.mesh").is_some(),
            "so is its asset"
        );
        assert!(cx.debug_bounds("out-of-date-tree.obj").is_none());
        save(&mut cx, "1-out-of-date-badges");

        // The linked source's menu offers Reimport and Unlink.
        let badge = cx.debug_bounds("out-of-date-rock.fbx").unwrap();
        for event in [true, false] {
            let (button, position) = (gpui::MouseButton::Right, badge.center());
            if event {
                cx.simulate_event(gpui::MouseDownEvent {
                    button,
                    position,
                    modifiers: gpui::Modifiers::none(),
                    click_count: 1,
                    first_mouse: false,
                });
            } else {
                cx.simulate_event(gpui::MouseUpEvent {
                    button,
                    position,
                    modifiers: gpui::Modifiers::none(),
                    click_count: 1,
                });
            }
        }
        draw(&mut cx);
        save(&mut cx, "2-linked-source-menu");
        cx.simulate_keystrokes("escape");
        draw(&mut cx);

        // Unlink from the asset's menu: the record goes, both files stay.
        let asset = root.join("rock.mesh");
        drawer.update_in(&mut cx, |drawer, window, cx| {
            handle_unlink_source(
                drawer,
                &UnlinkSource {
                    item_path: asset.to_string_lossy().into_owned(),
                },
                window,
                cx,
            )
        });
        draw(&mut cx);
        assert!(ImportDb::load(&root).records.is_empty());
        assert!(root.join("rock.fbx").exists() && asset.exists());
        assert_eq!(state(&mut cx, "rock.fbx"), Some(FileImportState::Unlinked));
        assert_eq!(
            state(&mut cx, "rock.mesh"),
            None,
            "no longer a linked asset"
        );

        cx.update(|window, _| window.remove_window());
        cx.run_until_parked();
    }
}
