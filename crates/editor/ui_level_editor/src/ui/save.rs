//! Saving the level without blocking the editor (Pulsar-Native#967).
//!
//! A save used to run entirely on the UI thread with the scene locked: scan
//! the project's class directories, serialize every component, read and parse
//! the existing level file, pretty-print and write it. The editor froze for
//! all of it, and the render thread waited on the scene lock.
//!
//! Now every step runs on a background thread:
//!
//! 1. scan the class registry (disk, no lock);
//! 2. snapshot the world ([`level_io::snapshot_level`]) under a *read* lock on
//!    the shared scene only -- the renderer keeps reading, and edits wait just
//!    for the snapshot, never for disk;
//! 3. write the snapshot ([`level_io::write_level`]), no lock at all.
//!
//! The unsaved-changes flag is cleared when the snapshot is taken, so an edit
//! made while the file is being written marks the level dirty again, and it is
//! set back if the write fails. Saves are numbered: a write whose snapshot is
//! older than one already written is dropped, so a slow save can never land on
//! top of a newer one.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use gpui::{App, Window};
use rust_i18n::t;
use ui::ContextModal as _;

use crate::scene_edit::{classes, level_io, LevelEditorCameraState};
use crate::{request_thumbnail_capture, LevelEditorState};

type StateArc = Arc<parking_lot::RwLock<LevelEditorState>>;

/// Numbers handed to saves, in the order they were started.
static NEXT_SAVE: AtomicU64 = AtomicU64::new(1);

/// Serializes writes; per file, the number of the newest snapshot written.
static LAST_WRITTEN: std::sync::LazyLock<parking_lot::Mutex<HashMap<PathBuf, u64>>> =
    std::sync::LazyLock::new(Default::default);

/// What happened to one save.
pub(crate) enum SaveOutcome {
    Saved,
    /// A newer save was already written; this one's older snapshot was dropped.
    Superseded,
}

/// Whether a save is the level itself or a copy elsewhere.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaveKind {
    /// Save / Save As: the level now lives at the path, and its unsaved
    /// changes are saved.
    Level,
    /// A copy (e.g. Save as Default): the level's own path and unsaved state
    /// are left alone.
    Copy,
}

/// Save the level to `path` in the background. On success the level's path
/// becomes `path` (so this is also Save As) and a thumbnail is requested;
/// failures are shown as a notification in `window`.
pub(crate) fn save_level(
    state: StateArc,
    path: PathBuf,
    editor_camera: Option<LevelEditorCameraState>,
    window: &mut Window,
    cx: &mut App,
) {
    save_in_background(
        state,
        path,
        editor_camera,
        SaveKind::Level,
        window,
        cx,
        |_, _, _| {},
    );
}

/// Save in the background, then call `then(result)` on the UI thread (e.g.
/// for a success notification; failures are already notified).
pub(crate) fn save_in_background(
    state: StateArc,
    path: PathBuf,
    editor_camera: Option<LevelEditorCameraState>,
    kind: SaveKind,
    window: &mut Window,
    cx: &mut App,
    then: impl FnOnce(&Result<SaveOutcome, String>, &mut Window, &mut App) + 'static,
) {
    let number = NEXT_SAVE.fetch_add(1, Ordering::Relaxed);
    window
        .spawn(cx, async move |cx| {
            let work_state = state.clone();
            let work_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    save_blocking(&work_state, &work_path, editor_camera, kind, number)
                })
                .await;

            match &result {
                Ok(SaveOutcome::Saved) if kind == SaveKind::Level => {
                    state.write().scene.current_scene = Some(path.clone());
                    request_thumbnail_capture(&state);
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!("Save of {} failed: {error}", path.display());
                    if kind == SaveKind::Level {
                        state.write().scene.has_unsaved_changes = true;
                    }
                }
            }
            cx.update(|window, cx| {
                if let Err(error) = &result {
                    window.push_notification(
                        ui::notification::Notification::error(
                            t!("Notification.Title.SaveScene").to_string(),
                        )
                        .message(
                            t!("Notification.Message.SaveFailed", error => error.to_string())
                                .to_string(),
                        ),
                        cx,
                    );
                }
                then(&result, window, cx);
                window.refresh();
            })
            .ok();
        })
        .detach();
}

/// Save now on the calling thread, in order with every other save. For
/// callers already off the UI thread (the AI tools); the editor uses
/// [`save_level`].
pub(crate) fn save_now(
    state: &StateArc,
    path: &std::path::Path,
    editor_camera: Option<LevelEditorCameraState>,
) -> Result<SaveOutcome, String> {
    let number = NEXT_SAVE.fetch_add(1, Ordering::Relaxed);
    let result = save_blocking(state, path, editor_camera, SaveKind::Level, number);
    if result.is_err() {
        state.write().scene.has_unsaved_changes = true;
    }
    result
}

/// The whole save, on the calling thread (a background one in the editor).
fn save_blocking(
    state: &StateArc,
    path: &std::path::Path,
    editor_camera: Option<LevelEditorCameraState>,
    kind: SaveKind,
    number: u64,
) -> Result<SaveOutcome, String> {
    profiling::profile_scope!("level_editor::save");
    // Disk scan: no lock held.
    let registry = classes::project_registry();

    // Snapshot under a read lock on the shared scene only (the editor-state
    // lock is released straight away), so the renderer keeps going. A level
    // save counts as saved from this point: later edits dirty it again.
    let (scene, settings, foliage_sets) = {
        let mut state = state.write();
        if kind == SaveKind::Level {
            state.scene.has_unsaved_changes = false;
        }
        (
            state.scene.shared_scene(),
            state.scene.world_settings.clone(),
            state.editor.terrain.foliage_sets.clone(),
        )
    };
    let snapshot = {
        let scene = scene.read();
        let mut snapshot =
            level_io::snapshot_level(&scene.world, &registry, editor_camera, settings);
        snapshot.foliage_sets = Some(foliage_sets);
        snapshot
    };

    // Write in save order; a snapshot older than one already written to the
    // same file is stale.
    let mut last_written = LAST_WRITTEN.lock();
    if last_written
        .get(path)
        .is_some_and(|&newest| number < newest)
    {
        return Ok(SaveOutcome::Superseded);
    }
    level_io::write_level(snapshot, path)?;
    last_written.insert(path.to_path_buf(), number);
    Ok(SaveOutcome::Saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{execute_command, SceneCommand};
    use crate::scene_edit::{ObjectType, SceneObjectData, Transform};

    fn state_with_object(name: &str) -> StateArc {
        let state = Arc::new(parking_lot::RwLock::new(LevelEditorState::new()));
        execute_command(
            &mut state.write(),
            SceneCommand::AddObject {
                data: SceneObjectData {
                    id: String::new(),
                    name: name.into(),
                    object_type: ObjectType::Empty,
                    transform: Transform::default(),
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    scene_path: String::new(),
                    props: Default::default(),
                    component_instances: None,
                },
                parent_id: None,
            },
        );
        state
    }

    #[test]
    fn foliage_palette_round_trips_with_selection_and_placement() {
        use crate::state::foliage_sets::FoliageSelection;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foliage.level");
        let state = Arc::new(parking_lot::RwLock::new(LevelEditorState::new()));
        state.write().edit_terrain(|terrain| {
            let library = &mut terrain.foliage_sets;
            let set = library.add_set();
            library.set_mut(set).unwrap().name = "Meadow".into();
            let oak = library
                .add_member(set, "assets/trees/oak.glb".into())
                .unwrap();
            let bush = library
                .add_member(set, "assets/plants/bush.fbx".into())
                .unwrap();
            library.member_mut(set, bush).unwrap().enabled = false;
            library
                .member_mut(set, oak)
                .unwrap()
                .placement
                .set_density(27.0);
            library
                .member_mut(set, oak)
                .unwrap()
                .placement
                .set_scale_min(0.7);
            library.selection = Some(FoliageSelection::Member(set, oak));
            let disabled = library.add_set();
            library.set_mut(disabled).unwrap().enabled = false;
            library.set_mut(disabled).unwrap().expanded = false;
            library.selection = Some(FoliageSelection::Member(set, oak));
        });
        assert!(state.read().scene.has_unsaved_changes);
        let expected = state.read().editor.terrain.foliage_sets.clone();
        save_now(&state, &path, None).unwrap();
        assert!(!state.read().scene.has_unsaved_changes);

        let restored = LevelEditorState::new();
        let (editor, _) =
            level_io::load_from_file_with_editor_state(&mut restored.scene.world_mut(), &path)
                .unwrap();
        assert_eq!(editor.foliage_sets, expected);
        assert_eq!(editor.foliage_sets.paintable_members().count(), 1);
        let mut library = editor.foliage_sets;
        let added = library.add_set();
        assert!(!expected.sets.iter().any(|set| set.id == added));

        // World-only saves must preserve authoring state even with a fresh camera.
        level_io::save_to_file_with_editor_camera(
            &restored.scene.world(),
            &path,
            Some(LevelEditorCameraState {
                position: [1.0, 2.0, 3.0],
                yaw: 0.0,
                pitch: 0.0,
            }),
        )
        .unwrap();
        let file: crate::scene_edit::LevelFile =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(file.editor.unwrap().foliage_sets, expected);

        // Saving an empty palette explicitly removes the old one.
        state
            .write()
            .edit_terrain(|terrain| terrain.foliage_sets = Default::default());
        assert!(state.read().scene.has_unsaved_changes);
        save_now(&state, &path, None).unwrap();
        let (editor, _) =
            level_io::load_from_file_with_editor_state(&mut restored.scene.world_mut(), &path)
                .unwrap();
        assert!(editor.foliage_sets.sets.is_empty());
    }

    #[test]
    fn old_levels_default_to_an_empty_foliage_palette() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.level");
        let state = LevelEditorState::new();
        level_io::save_to_file(&state.scene.world(), &path).unwrap();
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        json["editor"] = serde_json::json!({ "camera": { "position": [0.0, 0.0, 0.0], "yaw": 0.0, "pitch": 0.0 } });
        std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        let (editor, _) =
            level_io::load_from_file_with_editor_state(&mut state.scene.world_mut(), &path)
                .unwrap();
        assert!(editor.foliage_sets.sets.is_empty());
        assert!(editor.camera.is_some());
    }

    #[test]
    fn save_writes_the_level_and_clears_the_dirty_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.level");
        let state = state_with_object("Crate");
        assert!(state.read().scene.has_unsaved_changes);

        let n = NEXT_SAVE.fetch_add(1, Ordering::Relaxed);
        assert!(matches!(
            save_blocking(&state, &path, None, SaveKind::Level, n),
            Ok(SaveOutcome::Saved)
        ));
        assert!(!state.read().scene.has_unsaved_changes);
        assert!(std::fs::read_to_string(&path).unwrap().contains("Crate"));
    }

    #[test]
    fn an_older_snapshot_never_overwrites_a_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.level");
        let older = NEXT_SAVE.fetch_add(1, Ordering::Relaxed);
        let newer = NEXT_SAVE.fetch_add(1, Ordering::Relaxed);

        assert!(matches!(
            save_blocking(
                &state_with_object("Newer"),
                &path,
                None,
                SaveKind::Level,
                newer
            ),
            Ok(SaveOutcome::Saved)
        ));
        assert!(matches!(
            save_blocking(
                &state_with_object("Older"),
                &path,
                None,
                SaveKind::Level,
                older
            ),
            Ok(SaveOutcome::Superseded)
        ));
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("Newer") && !written.contains("Older"));
    }
}
