//! Saving the level without blocking the editor.
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
//! A level save first runs the plugins' save hooks
//! ([`crate::extend::SaveHook`]: the voxel worlds fold old edits into their
//! journals and keep the renderer's chunks of the edits with them), off the
//! scene lock but for the moment each is put in, and writes the plugins'
//! editor data ([`crate::extend::EditorData`]).
//!
//! The unsaved-changes flag is cleared when the snapshot is taken, so an edit
//! made while the file is being written marks the level dirty again, and it is
//! set back if the write fails. Saves are numbered: a write whose snapshot is
//! older than one already written is dropped, so a slow save can never land on
//! top of a newer one.
//!
//! The work runs as a tracked editor task ("Save level", category `Save`), so
//! the Tasks window shows it and its log. The write itself is staged beside the
//! level and renamed over it (`virtual_fs::write_file_atomically`): nothing
//! reads a half-written level and a crash keeps the old file. Writes to one
//! path never overlap and are coalesced ([`write_coalesced`]): while one is on
//! disk, the newest waiting snapshot replaces any older waiting one. The
//! editor's own writes are recognisable ([`is_own_write`]), so a reload path
//! can ignore them.

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

/// Per file, the number of the newest snapshot written.
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
            let (done, finished) = smol::channel::bounded(1);
            editor_task_queue::global().submit(
                editor_task_queue::TaskDescription::new(
                    "Save level",
                    "Save",
                    editor_task_queue::TaskDuration::Short,
                ),
                move |task| {
                    task.set_phase("Saving level");
                    let result =
                        save_blocking(&work_state, &work_path, editor_camera, kind, number);
                    let status = result.as_ref().map(|_| ()).map_err(Clone::clone);
                    let _ = done.try_send(result);
                    status
                },
            );
            let result = finished
                .recv()
                .await
                .unwrap_or_else(|_| Err("The save task ended without a result".into()));

            match &result {
                Ok(SaveOutcome::Saved) if kind == SaveKind::Level => {
                    state
                        .write()
                        .scene
                        .set_current_scene(Some(path.clone()), true);
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
    let (scene, settings, editor_data) = {
        let mut state = state.write();
        // While Play runs the world is the game's: spawned objects, or
        // another level a script loaded. Saving it would write that over the
        // open level.
        if state.scene.is_play_mode() {
            return Err("Stop Play before saving: the world is the running game's".into());
        }
        if kind == SaveKind::Level {
            state.scene.has_unsaved_changes = false;
        }
        let editor_data: std::collections::BTreeMap<_, _> = crate::extend::editor_data()
            .filter_map(|data| Some((data.key.to_string(), (data.save)(&state)?)))
            .collect();
        (
            state.scene.shared_scene(),
            state.scene.world_settings.clone(),
            editor_data,
        )
    };
    if kind == SaveKind::Level {
        crate::extend::run_save_hooks(&scene);
    }
    let snapshot = {
        let scene = scene.read();
        let mut snapshot =
            level_io::snapshot_level(&scene.world, &registry, editor_camera, settings);
        snapshot.editor_data = Some(editor_data);
        snapshot
    };

    write_coalesced(path, snapshot, number)
}

type Waiter = std::sync::mpsc::Sender<Result<(), String>>;

/// A snapshot waiting for the write in flight on its path to finish.
struct Pending {
    snapshot: level_io::LevelSnapshot,
    number: u64,
    /// Saves covered by this snapshot, waiting for its outcome.
    waiters: Vec<Waiter>,
}

#[derive(Default)]
struct PathWriter {
    pending: Option<Pending>,
}

/// Paths with a write in flight. A path is present exactly while one thread
/// is writing it; that thread also writes whatever waits in `pending`.
static WRITERS: std::sync::LazyLock<parking_lot::Mutex<HashMap<PathBuf, PathWriter>>> =
    std::sync::LazyLock::new(Default::default);

/// Size and modification time of the level as the editor last wrote it.
static OWN_STAMPS: std::sync::LazyLock<parking_lot::Mutex<HashMap<PathBuf, FileStamp>>> =
    std::sync::LazyLock::new(Default::default);

#[derive(Clone, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

fn stamp_of(path: &std::path::Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(FileStamp {
        len: meta.len(),
        modified: meta.modified().ok(),
    })
}

/// Whether the level at `path` is, as far as can be told, what the editor
/// itself last wrote (or is writing right now), so a reload on a file change
/// should be skipped: the scene already holds those contents.
pub(crate) fn is_own_write(path: &std::path::Path) -> bool {
    if WRITERS.lock().contains_key(path) {
        return true;
    }
    match (OWN_STAMPS.lock().get(path), stamp_of(path)) {
        (Some(own), Some(now)) => *own == now,
        _ => false,
    }
}

/// Write `snapshot` to `path`, never concurrently with another write to it.
///
/// If a write to `path` is in flight, the snapshot waits: a newer one replaces
/// an older waiting one (latest wins) and this call returns the outcome of the
/// write that ends up covering it. A snapshot older than one already written
/// is stale and dropped ([`SaveOutcome::Superseded`]).
fn write_coalesced(
    path: &std::path::Path,
    snapshot: level_io::LevelSnapshot,
    number: u64,
) -> Result<SaveOutcome, String> {
    let mut waiters: Vec<Waiter> = Vec::new();
    {
        let mut writers = WRITERS.lock();
        match writers.get_mut(path) {
            Some(writer) => {
                if writer.pending.as_ref().is_some_and(|p| p.number > number) {
                    return Ok(SaveOutcome::Superseded);
                }
                let (tx, rx) = std::sync::mpsc::channel();
                let mut covered = writer
                    .pending
                    .take()
                    .map(|older| older.waiters)
                    .unwrap_or_default();
                covered.push(tx);
                writer.pending = Some(Pending {
                    snapshot,
                    number,
                    waiters: covered,
                });
                drop(writers);
                // The writing thread finishes the work; this one only waits
                // (it is a task worker, never the UI thread).
                return match rx.recv() {
                    Ok(Ok(())) => Ok(SaveOutcome::Saved),
                    Ok(Err(error)) => Err(error),
                    Err(_) => Err("The level writer ended without a result".into()),
                };
            }
            None => {
                writers.insert(path.to_path_buf(), PathWriter::default());
            }
        }
    }

    let mut current = Some((snapshot, number));
    let mut own_result = None;
    while let Some((snapshot, number)) = current.take() {
        let result = write_one(path, snapshot, number);
        for waiter in waiters.drain(..) {
            let _ = waiter.send(result.as_ref().map(|_| ()).map_err(Clone::clone));
        }
        own_result.get_or_insert(result);
        let mut writers = WRITERS.lock();
        match writers.get_mut(path).and_then(|w| w.pending.take()) {
            Some(next) => {
                waiters = next.waiters;
                current = Some((next.snapshot, next.number));
            }
            None => {
                writers.remove(path);
            }
        }
    }
    own_result.expect("the first snapshot is always written")
}

/// Write one snapshot, unless a newer one already reached the file.
fn write_one(
    path: &std::path::Path,
    snapshot: level_io::LevelSnapshot,
    number: u64,
) -> Result<SaveOutcome, String> {
    if LAST_WRITTEN
        .lock()
        .get(path)
        .is_some_and(|&newest| number < newest)
    {
        return Ok(SaveOutcome::Superseded);
    }
    level_io::write_level(snapshot, path)?;
    LAST_WRITTEN.lock().insert(path.to_path_buf(), number);
    if let Some(stamp) = stamp_of(path) {
        OWN_STAMPS.lock().insert(path.to_path_buf(), stamp);
    }
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

    fn snapshot_of(state: &StateArc) -> level_io::LevelSnapshot {
        let scene = state.read().scene.shared_scene();
        let scene = scene.read();
        level_io::snapshot_level(
            &scene.world,
            &classes::project_registry(),
            None,
            Default::default(),
        )
    }

    fn only_level_files(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn concurrent_saves_to_one_path_never_overlap_and_leave_a_whole_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.level");
        let states: Vec<_> = (0..8)
            .map(|i| state_with_object(&format!("Obj{i}")))
            .collect();
        let threads: Vec<_> = states
            .into_iter()
            .map(|state| {
                let path = path.clone();
                std::thread::spawn(move || save_now(&state, &path, None))
            })
            .collect();
        for thread in threads {
            thread.join().unwrap().expect("every save succeeds");
        }
        // Whole, parseable JSON from exactly one of the saves; no staging
        // file left; nothing still marked in flight.
        let text = std::fs::read_to_string(&path).unwrap();
        serde_json::from_str::<serde_json::Value>(&text).expect("a whole level");
        assert_eq!(only_level_files(dir.path()), vec!["c.level".to_string()]);
        assert!(!WRITERS.lock().contains_key(&path));
        assert!(is_own_write(&path));
    }

    #[test]
    fn a_save_arriving_during_a_write_is_coalesced_and_latest_wins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.level");
        // Pretend a write is in flight on this path.
        WRITERS.lock().insert(path.clone(), PathWriter::default());
        let mut numbers = Vec::new();
        let mut waiting = Vec::new();
        for name in ["First", "Second", "Third"] {
            let n = NEXT_SAVE.fetch_add(1, Ordering::Relaxed);
            numbers.push(n);
            let snapshot = snapshot_of(&state_with_object(name));
            let thread_path = path.clone();
            waiting.push(std::thread::spawn(move || {
                write_coalesced(&thread_path, snapshot, n)
            }));
            // Let it register as the pending one before the next arrives.
            while !WRITERS
                .lock()
                .get(&path)
                .and_then(|w| w.pending.as_ref())
                .is_some_and(|p| p.number == n)
            {
                std::thread::yield_now();
            }
        }
        assert!(!path.exists(), "nothing is written while one is in flight");

        // The in-flight write ends and hands over, as write_coalesced does.
        let next = WRITERS
            .lock()
            .get_mut(&path)
            .unwrap()
            .pending
            .take()
            .unwrap();
        let result = write_one(&path, next.snapshot, next.number);
        assert!(matches!(result, Ok(SaveOutcome::Saved)));
        for waiter in next.waiters {
            let _ = waiter.send(Ok(()));
        }
        WRITERS.lock().remove(&path);
        for thread in waiting {
            assert!(matches!(thread.join().unwrap(), Ok(SaveOutcome::Saved)));
        }
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("Third"));
        assert!(!written.contains("First") && !written.contains("Second"));
    }

    // Windows refuses to replace a read-only file; Unix renames over it.
    #[cfg(windows)]
    #[test]
    fn a_failed_write_keeps_the_old_file_and_leaves_no_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e.level");
        let n = NEXT_SAVE.fetch_add(1, Ordering::Relaxed);
        save_blocking(&state_with_object("Old"), &path, None, SaveKind::Level, n).unwrap();
        // A read-only level cannot be replaced: the write fails after staging.
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();

        let n = NEXT_SAVE.fetch_add(1, Ordering::Relaxed);
        let result = save_blocking(&state_with_object("New"), &path, None, SaveKind::Level, n);

        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&path, permissions).unwrap();
        assert!(result.is_err(), "replacing a read-only level fails");
        let kept = std::fs::read_to_string(&path).unwrap();
        assert!(kept.contains("Old") && !kept.contains("New"));
        assert_eq!(only_level_files(dir.path()), vec!["e.level".to_string()]);
        assert!(!WRITERS.lock().contains_key(&path), "the path is released");
    }

    /// A level save keeps the viewport renderer's chunks of the voxel
    /// terrain's edits with the terrain's journal (asked while the renderer
    /// draws), and the level loads with them.
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
