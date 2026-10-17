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
//! A level save first folds old voxel edits into their journals' bases and
//! keeps the renderer's chunks of the edits with them, both off the scene
//! lock but for the moment each is put in.
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
    let (scene, settings, foliage_sets, voxel_chunk_saves) = {
        let mut state = state.write();
        if kind == SaveKind::Level {
            state.scene.has_unsaved_changes = false;
        }
        (
            state.scene.shared_scene(),
            state.scene.world_settings.clone(),
            state.editor.terrain.foliage_sets.clone(),
            state.scene.voxel_chunk_saves.clone(),
        )
    };
    if kind == SaveKind::Level {
        compact_voxel_journals(&scene);
        if let Some(saves) = &voxel_chunk_saves {
            keep_voxel_chunks(&scene, saves);
        }
    }
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

/// Fold old voxel edits into their journals' bases, so the level saves
/// what they left rather than every brush (`VoxelEditJournal::compact`).
/// The bases are built off the scene lock; a journal that changed meanwhile
/// still takes its base when it starts with it.
fn compact_voxel_journals(scene: &engine_backend::scene::SharedScene) {
    use helio_component::voxel_world;
    profiling::profile_scope!("level_editor::save::compact_voxel_edits");
    let pending = voxel_world::journals_to_compact(&scene.read().world);
    for (entity, recipe, edits) in pending {
        match voxel_world::compact_journal(recipe, &edits, voxel_world::LISTED_EDITS) {
            Ok(Some(base)) => {
                let mut scene = scene.write();
                let terrain = scene
                    .world
                    .get_mut::<helio_component::VoxelTerrainComponent>(entity);
                if let Some(mut terrain) = terrain {
                    terrain.edits.compact(base);
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!("Voxel edits kept unfolded: {error}"),
        }
    }
}

/// How long a save waits for the renderer's voxel chunks (a few frames
/// when the viewport draws; none when it does not).
const VOXEL_CHUNKS_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Keep the renderer's chunks of the voxel terrain's edits with its journal
/// (`VoxelEditJournal::keep_chunks`), so loading the level copies what the
/// edits made instead of generating it from the terrain and every edit.
/// Asked for only when a terrain with edits keeps none for them; kept only
/// while the journal is the one they were made for. Deflated here, off the
/// render thread and the scene lock.
fn keep_voxel_chunks(
    scene: &engine_backend::scene::SharedScene,
    saves: &engine_backend::subsystems::render::VoxelChunkSaves,
) {
    use helio_component::VoxelTerrainComponent;
    profiling::profile_scope!("level_editor::save::keep_voxel_chunks");
    let wanted = scene
        .read()
        .world
        .query::<&VoxelTerrainComponent>()
        .into_iter()
        .any(|(_, c)| !c.edits.is_empty() && c.edits.kept_chunks().is_none());
    if !wanted {
        return;
    }
    let Some(saved) = saves.kept_chunks(VOXEL_CHUNKS_WAIT) else {
        return;
    };
    if saved.chunks.is_empty() {
        return;
    }
    let kept = helio_voxel_data::VoxelKeptChunks::of(&saved.edits, saved.chunks.to_bytes().into());
    let entity = pulsar_scenedb::Entity::from_bits(saved.id.entity_bits);
    let mut scene = scene.write();
    let unchanged = scene
        .world
        .get::<VoxelTerrainComponent>(entity)
        .is_some_and(|terrain| terrain.edits == saved.edits);
    if unchanged {
        if let Some(mut terrain) = scene.world.get_mut::<VoxelTerrainComponent>(entity) {
            terrain.edits.keep_chunks(kept);
        }
    }
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

    /// A level save keeps the viewport renderer's chunks of the voxel
    /// terrain's edits with the terrain's journal (asked while the renderer
    /// draws), and the level loads with them.
    #[test]
    fn a_level_save_keeps_the_renderers_voxel_chunks() {
        use crate::scene_edit::{components, objects, ObjectType, SceneObjectData, Transform};
        use engine_backend::scene::SceneWorldExt as _;
        use engine_backend::subsystems::render::{EditorCameraState, HelioRenderer};
        use helio_component::{voxel_world, VoxelTerrainComponent, VoxelTerrainLayersComponent};
        use helio_voxel_data::{VoxelBrushEdit, VoxelBrushOp, VoxelBrushShape};
        const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
            eprintln!("skipping: no GPU adapter available");
            return;
        };
        let features = adapter.features() - wgpu::Features::EXPERIMENTAL_RAY_QUERY;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: helio::required_wgpu_features(features),
            required_limits: helio::required_wgpu_limits(adapter.limits()),
            experimental_features: helio::required_experimental_features(features),
            ..Default::default()
        }))
        .unwrap();

        // A flat terrain dug where the camera looks.
        let state = Arc::new(parking_lot::RwLock::new(LevelEditorState::new()));
        let terrain = {
            let state = state.read();
            let mut world = state.scene.world_mut();
            let id = objects::add_object(
                &mut world,
                SceneObjectData {
                    id: String::new(),
                    name: "Terrain".into(),
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
                None,
            );
            components::add_component(
                &mut world,
                &id,
                "VoxelTerrainComponent".into(),
                serde_json::to_value(VoxelTerrainComponent::plane(512.0)).unwrap(),
            );
            components::add_component(
                &mut world,
                &id,
                "VoxelTerrainLayersComponent".into(),
                serde_json::to_value(VoxelTerrainLayersComponent::flat(2.0)).unwrap(),
            );
            let owner = world.entity_for(&id).unwrap();
            let terrain = engine_backend::scene::attachments::enabled_components_of::<
                VoxelTerrainComponent,
            >(&world, owner)[0]
                .0;
            let edits = (0..64)
                .map(|k| VoxelBrushEdit {
                    center: [
                        f64::from(k % 8) * 3.0 - 10.0,
                        2.0,
                        f64::from(k / 8) * 3.0 - 30.0,
                    ],
                    radius: 2.5,
                    shape: VoxelBrushShape::Sphere,
                    op: VoxelBrushOp::Remove,
                    material: 0,
                    height: 0.0,
                })
                .collect();
            voxel_world::append_edits(&mut world, terrain, edits).unwrap();
            terrain
        };

        // The editor's viewport renderer, settled on the dig.
        let scene = state.read().scene.shared_scene();
        let mut renderer = HelioRenderer::new(scene.clone());
        renderer.set_editor_camera_state(EditorCameraState {
            position: [0.0, 14.0, 0.0],
            yaw: 0.0,
            pitch: -0.5,
        });
        state.write().scene.voxel_chunk_saves = Some(renderer.editor_mailbox().voxel_chunk_saves());
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 320,
                height: 180,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let mut frame = || {
            let rendered = renderer
                .render_frame(&device, &queue, &view, 320, 180, FORMAT)
                .is_some();
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rendered
        };
        for _ in 0..4000 {
            if !frame() {
                break;
            }
        }

        // Saved on another thread while the viewport keeps drawing.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dug.level");
        let n = NEXT_SAVE.fetch_add(1, Ordering::Relaxed);
        let saving = {
            let (state, path) = (state.clone(), path.clone());
            std::thread::spawn(move || save_blocking(&state, &path, None, SaveKind::Level, n))
        };
        while !saving.is_finished() {
            frame();
        }
        assert!(matches!(saving.join().unwrap(), Ok(SaveOutcome::Saved)));
        let saved = scene
            .read()
            .world
            .get::<VoxelTerrainComponent>(terrain)
            .unwrap()
            .edits
            .clone();
        let kept = saved
            .kept_chunks()
            .expect("the save kept the renderer's chunks");
        eprintln!("SAVE_KEPT {} KiB of chunks", kept.bytes.len() / 1024);

        let mut loaded = engine_backend::scene::new_scene();
        engine_backend::scene::RuntimeLevel::load_into(&path, &mut loaded.world).unwrap();
        let journals: Vec<_> = loaded
            .world
            .query::<&VoxelTerrainComponent>()
            .into_iter()
            .map(|(_, terrain)| terrain.edits.clone())
            .collect();
        assert_eq!(journals.len(), 1);
        assert_eq!(journals[0], saved);
        assert_eq!(journals[0].kept_chunks(), Some(kept));
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
