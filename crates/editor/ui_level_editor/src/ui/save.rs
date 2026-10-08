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
    let scene = {
        let mut state = state.write();
        if kind == SaveKind::Level {
            state.scene.has_unsaved_changes = false;
        }
        state.scene.shared_scene()
    };
    let snapshot = {
        let scene = scene.read();
        let settings = state.read().scene.world_settings.clone();
        level_io::snapshot_level(&scene.world, &registry, editor_camera, settings)
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
