//! `.level` file save/load, straight between the world and the file format.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use engine_fs::virtual_fs;
use pulsar_class::ClassRegistry;
use pulsar_scenedb::World;

use super::classes::{self, project_registry};
use super::{ComponentInstance, ObjectId, SceneObjectData};

use engine_backend::scene::SceneWorldExt;

use super::components::{get_components, replace_components};
use super::objects::{add_object, clear, get_all_objects};
use super::{LevelEditorCameraState, LevelEditorFileState, LevelFile, LevelMetadata};
use crate::world_settings_data::WorldSettingsData;

/// Serialize the scene to a JSON level file.
pub fn save_to_file<P: AsRef<Path>>(world: &World, path: P) -> Result<(), String> {
    save_to_file_with_editor_camera(world, path, None)
}

/// Serialize the scene to a JSON level file, optionally persisting editor camera state.
pub fn save_to_file_with_editor_camera<P: AsRef<Path>>(
    world: &World,
    path: P,
    editor_camera: Option<LevelEditorCameraState>,
) -> Result<(), String> {
    let settings = read_existing_world_settings(path.as_ref());
    save_with_classes_and_settings(world, path, editor_camera, settings, &project_registry())
}

/// Save a level with an explicit snapshot of its world settings.
pub fn save_to_file_with_settings<P: AsRef<Path>>(
    world: &World,
    path: P,
    editor_camera: Option<LevelEditorCameraState>,
    world_settings: WorldSettingsData,
) -> Result<(), String> {
    save_with_classes_and_settings(
        world,
        path,
        editor_camera,
        world_settings,
        &project_registry(),
    )
}

/// The objects and component lists a save writes.
///
/// Placed classes are saved by reference (#921): a class root keeps its
/// `ClassInstance` with overrides recomputed against the current class
/// defaults (only the differences), plus any component the user added;
/// the components built from class slots and the generated child objects
/// are left out, since loading rebuilds them from the class. User objects
/// parented under a generated child are saved under its nearest saved
/// ancestor.
pub(crate) fn level_contents(
    world: &World,
    registry: &ClassRegistry,
) -> (
    Vec<SceneObjectData>,
    HashMap<ObjectId, Vec<ComponentInstance>>,
) {
    let all = get_all_objects(world);
    let generated: HashSet<ObjectId> = all
        .iter()
        .filter(|obj| {
            classes::is_generated_child(world, &obj.id)
                || super::objects::is_instance_member(world, &obj.id)
        })
        .map(|obj| obj.id.clone())
        .collect();
    let parent_of: HashMap<ObjectId, Option<ObjectId>> = all
        .iter()
        .map(|obj| (obj.id.clone(), obj.parent.clone()))
        .collect();
    let mut objects = Vec::with_capacity(all.len());
    let mut components = HashMap::new();
    for mut obj in all {
        if generated.contains(&obj.id) {
            continue;
        }
        while let Some(parent) = obj.parent.clone().filter(|p| generated.contains(p)) {
            obj.parent = parent_of.get(&parent).cloned().flatten();
        }
        let mut list = get_components(world, &obj.id);
        if classes::is_class_root(world, &obj.id) {
            let overrides = classes::current_overrides(world, &obj.id, registry);
            list.retain(|c| c.data.get(pulsar_class::SLOT_ID_KEY).is_none());
            for component in &mut list {
                if component.class_name == pulsar_class::CLASS_INSTANCE {
                    if let Some(instance) = &overrides {
                        component.data = instance.to_value();
                    }
                    // Only the class reference and overrides are data here.
                    if let Some(map) = component.data.as_object_mut() {
                        map.retain(|key, _| !key.starts_with("__"));
                    }
                }
            }
            // The persisted list is authoritative; don't duplicate the
            // class's component data inline.
            obj.component_instances = None;
            obj.props.remove("script_asset");
        }
        components.insert(obj.id.clone(), list);
        objects.push(obj);
    }
    (objects, components)
}

/// [`save_to_file_with_editor_camera`] with an explicit class registry.
pub(crate) fn save_with_classes<P: AsRef<Path>>(
    world: &World,
    path: P,
    editor_camera: Option<LevelEditorCameraState>,
    registry: &ClassRegistry,
) -> Result<(), String> {
    let settings = read_existing_world_settings(path.as_ref());
    save_with_classes_and_settings(world, path, editor_camera, settings, registry)
}

pub(crate) fn save_with_classes_and_settings<P: AsRef<Path>>(
    world: &World,
    path: P,
    editor_camera: Option<LevelEditorCameraState>,
    world_settings: WorldSettingsData,
    registry: &ClassRegistry,
) -> Result<(), String> {
    write_level(
        snapshot_level(world, registry, editor_camera, world_settings),
        path.as_ref(),
    )
}

fn read_existing_world_settings(path: &Path) -> WorldSettingsData {
    virtual_fs::read_file(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value.get("world_settings").cloned())
        .and_then(|settings| serde_json::from_value(settings).ok())
        .unwrap_or_default()
}

/// Everything a save needs from the world, captured in one pass.
///
/// Saving is split so the world is only needed for this snapshot: taking it
/// is pure in-memory work under the scene lock, while [`write_level`] (reading
/// the existing file, serializing, writing) needs no world and can run on any
/// thread with the lock released. See `ui::save`.
pub struct LevelSnapshot {
    objects: Vec<SceneObjectData>,
    components: HashMap<ObjectId, Vec<ComponentInstance>>,
    /// Resolved class name of each placed class instance, keyed by object id,
    /// for deciding which legacy blueprint bindings still need carrying over.
    class_names: HashMap<ObjectId, String>,
    editor_camera: Option<LevelEditorCameraState>,
    world_settings: WorldSettingsData,
    /// None for world-only saves, which preserve the existing palette.
    pub foliage_sets: Option<crate::state::foliage_sets::FoliageSetLibrary>,
}

/// Capture what [`write_level`] writes. `registry` should be the project's
/// classes (`classes::project_registry`, which scans the disk -- take it
/// before locking the world).
pub fn snapshot_level(
    world: &World,
    registry: &ClassRegistry,
    editor_camera: Option<LevelEditorCameraState>,
    world_settings: WorldSettingsData,
) -> LevelSnapshot {
    profiling::profile_scope!("scene_edit::snapshot_level");
    let (objects, components) = level_contents(world, registry);
    let class_names = objects
        .iter()
        .filter_map(|obj| {
            let instance = classes::class_instance(world, &obj.id)?;
            let name = registry
                .resolve(&instance)
                .map(|entry| entry.name.clone())
                .unwrap_or(instance.class_name);
            Some((obj.id.clone(), name))
        })
        .collect();
    LevelSnapshot {
        objects,
        components,
        class_names,
        editor_camera,
        world_settings,
        foliage_sets: None,
    }
}

/// Write a snapshot to `path`. Needs no world or lock.
pub fn write_level(snapshot: LevelSnapshot, path: &Path) -> Result<(), String> {
    profiling::profile_scope!("scene_edit::write_level");
    let LevelSnapshot {
        objects,
        components,
        class_names,
        editor_camera,
        world_settings,
        foliage_sets,
    } = snapshot;
    if let Some(parent_dir) = path.parent() {
        virtual_fs::create_dir_all(parent_dir)
            .map_err(|e| format!("Failed to create directory: {e}"))?;
    }
    let now = chrono::Utc::now().to_rfc3339();
    // Read the existing file once: its editor camera is preserved when no fresh
    // camera state was supplied, and its #650 blueprint-binding section always
    // rides along (the editor cannot author it yet, but a re-save must never
    // destroy it).
    let existing_file = virtual_fs::read_file(path)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|json: String| serde_json::from_str::<LevelFile>(&json).ok());
    let mut editor = existing_file.as_ref().and_then(|file| file.editor.clone());
    if let Some(camera) = editor_camera {
        editor.get_or_insert_with(Default::default).camera = Some(camera);
    }
    if let Some(foliage_sets) = foliage_sets {
        editor.get_or_insert_with(Default::default).foliage_sets = foliage_sets;
    }
    // Legacy `blueprint_bindings` are no longer written: loading migrates
    // them to `ClassInstance`. Only entries the migration could not express
    // (a second class bound to one object) are carried over, so a re-save
    // never destroys them.
    let preserved_bindings = existing_file
        .map(|file| unmigrated_bindings(&class_names, file.blueprint_bindings))
        .unwrap_or_default();
    let level_file = LevelFile {
        version: engine_backend::scene::hierarchy_transform::PARENT_RELATIVE_FILE_VERSION.into(),
        objects,
        components,
        blueprint_bindings: preserved_bindings,
        metadata: LevelMetadata {
            created: now.clone(),
            modified: now,
            editor_version: env!("CARGO_PKG_VERSION").into(),
        },
        world_settings,
        editor,
    };
    let json = serde_json::to_string_pretty(&level_file)
        .map_err(|e| format!("Failed to serialize: {e}"))?;
    virtual_fs::write_file(path, json.as_bytes())
        .map_err(|e| format!("Failed to write file: {e}"))?;

    tracing::info!("Scene saved to: {}", path.display());
    Ok(())
}

/// Bindings of `bindings` not represented by a `ClassInstance` of the same
/// class on the bound object.
///
/// `class_names` is each placed instance's resolved class name
/// ([`LevelSnapshot::class_names`]).
fn unmigrated_bindings(
    class_names: &HashMap<ObjectId, String>,
    mut bindings: pulsar_scene::BlueprintBindings,
) -> pulsar_scene::BlueprintBindings {
    for (stable_id, entries) in bindings.iter_mut() {
        let Some(class_name) = class_names.get(stable_id.as_str()) else {
            // Object gone, or never migrated: nothing represents it.
            continue;
        };
        entries.retain(|binding| &binding.class_name != class_name);
    }
    bindings.retain(|_, entries| !entries.is_empty());
    bindings
}

/// Report component data of a registered class that does not decode even
/// after the load migrations. The editor keeps it as an unresolved payload
/// (so saving loses nothing and it can be fixed by hand); the runtime
/// refuses such a level, so it is a warning here, never silent.
fn report_invalid_known_data(world: &World, path: &Path) {
    for (instance, (meta, unresolved)) in world.query::<(
        &engine_backend::scene::attachments::ComponentMeta,
        &engine_backend::scene::attachments::UnresolvedComponent,
    )>() {
        if pulsar_world_registry::component_id_for_class(&meta.class_name).is_none() {
            continue;
        }
        let object = engine_backend::scene::attachments::owner_of(world, instance)
            .and_then(|owner| world.stable_id_of(owner))
            .map(|id| id.to_string())
            .unwrap_or_default();
        tracing::warn!(
            object = %object,
            class = %meta.class_name,
            path = %path.display(),
            "Component data is invalid for its class and was kept unresolved (the runtime will refuse this level): {}",
            unresolved.reason
        );
    }
}

/// Say what the load-time record migrations changed (`pulsar_class::records`).
pub(crate) fn log_record_migrations(
    records: &pulsar_class::records::RecordMigrations,
    path: &Path,
) {
    for id in &records.material_overrides {
        tracing::info!(object = %id, path = %path.display(), "Folded MaterialOverrideComponent into the mesh's material slots");
    }
    for (id, class) in &records.nested {
        tracing::info!(object = %id, class = %class, path = %path.display(), "Rewrote flat component data to the class's shape");
    }
    for id in &records.mesh_asset_props {
        tracing::info!(object = %id, path = %path.display(), "Turned props.mesh_asset into a StaticMeshComponent");
    }
    for (id, keys) in &records.stripped_props {
        tracing::info!(object = %id, path = %path.display(), ?keys, "Removed component copies from object props");
    }
    for (id, class, fields) in &records.retired_fields {
        tracing::info!(object = %id, class = %class, path = %path.display(), ?fields, "Removed retired component fields");
    }
}

/// Load a scene from a JSON level file (replaces the current scene).
pub fn load_from_file<P: AsRef<Path>>(world: &mut World, path: P) -> Result<(), String> {
    load_from_file_with_editor_camera(world, path).map(|_| ())
}

/// Load a scene from a JSON level file and return any persisted editor camera state.
pub fn load_from_file_with_editor_camera<P: AsRef<Path>>(
    world: &mut World,
    path: P,
) -> Result<Option<LevelEditorCameraState>, String> {
    load_from_file_with_editor_camera_and_settings(world, path).map(|(camera, _)| camera)
}

/// Load a scene and return its persisted editor camera and world settings.
pub fn load_from_file_with_editor_camera_and_settings<P: AsRef<Path>>(
    world: &mut World,
    path: P,
) -> Result<(Option<LevelEditorCameraState>, WorldSettingsData), String> {
    load_with_classes_and_settings(world, path, &project_registry())
}

/// Load all per-level authoring state as well as the world.
pub fn load_from_file_with_editor_state<P: AsRef<Path>>(
    world: &mut World,
    path: P,
) -> Result<(LevelEditorFileState, WorldSettingsData), String> {
    load_with_classes_and_editor_state(world, path, &project_registry())
}

/// [`load_from_file_with_editor_camera`] with an explicit class registry.
///
/// Old class references are migrated first (`ScriptComponent` class paths
/// and `blueprint_bindings` become `ClassInstance`), then every placed class
/// is rebuilt from its current definition with its overrides applied.
pub(crate) fn load_with_classes<P: AsRef<Path>>(
    world: &mut World,
    path: P,
    registry: &ClassRegistry,
) -> Result<Option<LevelEditorCameraState>, String> {
    load_with_classes_and_settings(world, path, registry).map(|(camera, _)| camera)
}

pub(crate) fn load_with_classes_and_settings<P: AsRef<Path>>(
    world: &mut World,
    path: P,
    registry: &ClassRegistry,
) -> Result<(Option<LevelEditorCameraState>, WorldSettingsData), String> {
    load_with_classes_and_editor_state(world, path, registry)
        .map(|(editor, settings)| (editor.camera, settings))
}

fn load_with_classes_and_editor_state<P: AsRef<Path>>(
    world: &mut World,
    path: P,
    registry: &ClassRegistry,
) -> Result<(LevelEditorFileState, WorldSettingsData), String> {
    load_with_classes_and_editor_state_progress(world, path, registry, &mut |_| {})
}

fn load_with_classes_and_editor_state_progress<P: AsRef<Path>>(
    world: &mut World,
    path: P,
    registry: &ClassRegistry,
    progress: &mut impl FnMut(f32),
) -> Result<(LevelEditorFileState, WorldSettingsData), String> {
    profiling::profile_scope!("scene_edit::load_from_file");
    let mut prepared = prepare_level_for_hydration_with_registry(path, registry)?;
    let total_work = prepared.total_work();
    let mut completed_work = 0usize;
    progress(0.02);
    clear(world);
    for obj in std::mem::take(&mut prepared.objects) {
        let parent = obj.parent.clone();
        add_object(world, obj, parent);
        completed_work += 1;
        progress(0.05 + 0.78 * completed_work as f32 / total_work as f32);
    }
    convert_legacy_transforms(world, &prepared.version);
    for (object_id, components) in std::mem::take(&mut prepared.components) {
        replace_components(world, &object_id, &components);
        completed_work += components.len();
        progress(0.05 + 0.78 * completed_work as f32 / total_work as f32);
    }
    progress(0.86);
    finish_prepared_hydration_with_registry(world, &prepared, registry);
    engine_backend::scene::resolve_level_assets(world);
    progress(0.98);
    tracing::info!(
        "Scene loaded from: {} (version: {})",
        prepared.path.display(),
        prepared.version
    );
    Ok((prepared.editor, prepared.world_settings))
}

/// Parsed level data, ready to be applied in short SceneDB write batches.
/// This is file data, not a second scene owner.
pub(crate) struct PreparedLevel {
    pub(crate) path: PathBuf,
    pub(crate) version: String,
    pub(crate) objects: Vec<SceneObjectData>,
    pub(crate) components: HashMap<ObjectId, Vec<ComponentInstance>>,
    pub(crate) editor: LevelEditorFileState,
    pub(crate) world_settings: WorldSettingsData,
    registry: ClassRegistry,
}

impl PreparedLevel {
    pub(crate) fn total_work(&self) -> usize {
        (self.objects.len()
            + self
                .components
                .values()
                .map(|instances| instances.len())
                .sum::<usize>())
        .max(1)
    }
}

/// Read and migrate level data without holding the shared SceneDB lock.
pub(crate) fn prepare_level_for_hydration<P: AsRef<Path>>(
    path: P,
) -> Result<PreparedLevel, String> {
    prepare_level_for_hydration_with_registry(path, &project_registry())
}

fn prepare_level_for_hydration_with_registry<P: AsRef<Path>>(
    path: P,
    registry: &ClassRegistry,
) -> Result<PreparedLevel, String> {
    let bytes =
        virtual_fs::read_file(path.as_ref()).map_err(|e| format!("Failed to read file: {e}"))?;
    prepare_level_bytes_for_hydration(bytes, path, registry)
}

/// Prepare an untitled scene on the loader worker, without a temporary file.
pub(crate) fn prepare_new_level_for_hydration(
    embedded: Option<Vec<u8>>,
) -> Result<PreparedLevel, String> {
    match embedded {
        Some(bytes) => prepare_level_bytes_for_hydration(
            bytes,
            Path::new("Untitled.level"),
            &project_registry(),
        ),
        None => Ok(PreparedLevel {
            path: PathBuf::from("Untitled.level"),
            version: "2.0".into(),
            objects: Vec::new(),
            components: HashMap::new(),
            editor: LevelEditorFileState::default(),
            world_settings: WorldSettingsData::default(),
            registry: ClassRegistry::default(),
        }),
    }
}

fn prepare_level_bytes_for_hydration<P: AsRef<Path>>(
    bytes: Vec<u8>,
    path: P,
    registry: &ClassRegistry,
) -> Result<PreparedLevel, String> {
    let json = String::from_utf8(bytes).map_err(|e| format!("File is not valid UTF-8: {e}"))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&json).map_err(|e| format!("Failed to parse JSON: {e}"))?;
    let report = pulsar_class::migrate::migrate_level_value(&mut value, registry);
    if report.changed() {
        tracing::info!(
            "Migrated {} ScriptComponent(s) and {} binding(s) to ClassInstance in {}",
            report.script_components.len(),
            report.bindings.len(),
            path.as_ref().display()
        );
    }
    log_record_migrations(&report.records, path.as_ref());
    let level_file: LevelFile =
        serde_json::from_value(value).map_err(|e| format!("Failed to parse JSON: {e}"))?;
    if !level_file.version.starts_with("2.") && !level_file.version.starts_with("1.") {
        return Err(format!(
            "Unsupported scene version: {}. Expected 1.x or 2.x",
            level_file.version
        ));
    }
    Ok(PreparedLevel {
        path: path.as_ref().to_path_buf(),
        version: level_file.version,
        objects: level_file.objects,
        components: level_file.components,
        editor: level_file.editor.unwrap_or_default(),
        world_settings: level_file.world_settings,
        registry: registry.clone(),
    })
}

pub(crate) fn clear_for_hydration(world: &mut World) {
    clear(world);
}

pub(crate) fn add_hydrated_object(world: &mut World, obj: SceneObjectData) {
    let parent = obj.parent.clone();
    add_object(world, obj, parent);
}

pub(crate) fn convert_legacy_transforms(world: &mut World, version: &str) {
    // A scene saved before parent-relative transforms holds world-space values
    // for parented objects: make them local, keeping every object in place.
    if engine_backend::scene::hierarchy_transform::file_has_world_space_transforms(version) {
        engine_backend::scene::hierarchy_transform::convert_all_legacy_world_transforms(world);
    }
}

pub(crate) fn add_hydrated_components(
    world: &mut World,
    object_id: &str,
    components: &[ComponentInstance],
) {
    replace_components(world, object_id, components);
}

pub(crate) fn finish_prepared_hydration(world: &mut World, prepared: &PreparedLevel) {
    // The interactive loader calls this with the shared scene write-locked.
    // Never wait for asset workers here: every UI reader would wait with us.
    // Its caller pumps completed assets between unlocked async waits instead.
    finish_prepared_hydration_with_registry(world, prepared, &prepared.registry);
}

fn finish_prepared_hydration_with_registry(
    world: &mut World,
    prepared: &PreparedLevel,
    registry: &ClassRegistry,
) {
    report_invalid_known_data(world, &prepared.path);
    // Everything that defines a space (rigid frames, sublevel placements) gets its
    // slot (`.agents/FRAMES.md`).
    engine_backend::scene::frames::sync_spaces(world);
    // Objects that show another level get its contents.
    engine_backend::scene::sublevel_display::sync_level_instances(world);

    let unresolved = classes::rebuild_all_instances(world, registry);
    if !unresolved.is_empty() {
        tracing::warn!(
            "{} placed class instance(s) reference classes missing from this project: {:?}",
            unresolved.len(),
            unresolved
        );
    }
    // One sky and one directional light per level; extras are kept but
    // disabled, logged.
    engine_backend::scene::level_rules::enforce_on_load(world);
}

#[cfg(test)]
mod interactive_hydration_tests {
    use super::*;
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn finalization_releases_the_scene_while_an_asset_loader_is_blocked() {
        const KIND: &str = "editor-finalization-nonblocking-regression";
        let (release, wait) = mpsc::channel();
        let wait = Mutex::new(wait);
        let (started, start) = mpsc::channel();
        pulsar_scenedb::register_loader(KIND, move |_| {
            let _ = started.send(());
            wait.lock().unwrap().recv().map_err(|e| e.to_string())?;
            Ok(Arc::new(()) as pulsar_scenedb::AssetPayload)
        });

        let scene = Arc::new(parking_lot::RwLock::new(engine_backend::scene::new_scene()));
        {
            let mut store = scene.write();
            store.world.reload_asset(pulsar_scenedb::AssetId::declare(KIND, "held"));
            store.world.pump_assets();
        }
        start.recv_timeout(Duration::from_secs(5)).expect("loader starts");

        let loading_scene = scene.clone();
        let (finished, finish) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let prepared = PreparedLevel {
                path: PathBuf::from("nonblocking.level"),
                version: "2.0".into(),
                objects: Vec::new(),
                components: HashMap::new(),
                editor: LevelEditorFileState::default(),
                world_settings: WorldSettingsData::default(),
                registry: ClassRegistry::default(),
            };
            {
                let mut store = loading_scene.write();
                finish_prepared_hydration_with_registry(
                    &mut store.world,
                    &prepared,
                    &ClassRegistry::default(),
                );
            }
            finished.send(()).unwrap();
        });

        // Release the loader even on regression, so a failing test does not
        // leave a worker waiting for the full production asset timeout.
        let returned_before_asset = finish.recv_timeout(Duration::from_secs(2)).is_ok();
        let readable_while_loading = scene
            .try_read()
            .is_some_and(|store| store.world.asset_work_pending());
        let _ = release.send(());
        worker.join().unwrap();
        assert!(returned_before_asset, "finalization waited for an asset worker");
        assert!(readable_while_loading, "the UI could not read the loading scene");
    }
}

#[cfg(test)]
mod voxel_example_tests {
    use super::*;

    #[test]
    fn voxel_planet_example_loads_as_a_scenedb_backend_source() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../assets/examples/voxel_planet/scene/default.level");
        let mut world = World::new();
        let camera = load_from_file_with_editor_camera(&mut world, path)
            .expect("example level loads")
            .expect("example camera is present");
        // Over the foothills at the north pole, facing a range.
        assert_eq!(camera.position, [0.0, 6_371_631.0, 0.0]);

        let (entries, errors) = engine_backend::scene::voxel_frame::project_voxel_entries(&world);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].generator.as_ref().unwrap().version, 1);
        // The Earth preset (seed 75: foothills at the pole, a range 23 km away).
        let stacks: Vec<_> = engine_backend::scene::attachments::enabled_components::<
            helio_component::VoxelTerrainLayersComponent,
        >(&world)
        .map(|(_, _, c)| c.stack.clone())
        .collect();
        assert_eq!(stacks, [helio_component::VoxelTerrainStack::earth()]);
        // Graded like an outdoor scene: one unbound volume tone maps it.
        let volumes: Vec<_> = engine_backend::scene::attachments::enabled_components::<
            helio_component::PostProcessVolumeComponent,
        >(&world)
        .map(|(_, _, v)| v.clone())
        .collect();
        // The planet's air, centred on the planet's object.
        let air: Vec<_> = engine_backend::scene::attachments::enabled_components::<
            helio_component::AtmosphereComponent,
        >(&world)
        .map(|(_, _, a)| a.placement)
        .collect();
        assert_eq!(air, [helio_component::AtmospherePlacement::PlanetAtOwner]);
        assert_eq!(volumes.len(), 1);
        assert!(
            volumes[0].unbound
                && volumes[0].settings.tonemap_operator == helio_component::TonemapOperator::AgxPunchy
        );
    }
}
